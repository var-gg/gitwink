// Read-only git access via `git2`.
//
// HARD RULE: every call here must be read-only. No commits, fetches, pushes,
// merges, or any operation that mutates a repo's state on disk.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use git2::{BranchType, Oid, Repository, Sort};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;

// Timeline rows cross the Rust -> JSON -> JS boundary in batches. Commit
// messages are normally tiny, but hostile/imported repositories can contain
// megabyte-scale payloads; cap each row so a 1,000-row file-history response
// cannot balloon into tens or hundreds of MiB.
const MAX_DIRECT_MESSAGE_BYTES: usize = 8 * 1024;
const MAX_DIRECT_SUMMARY_BYTES: usize = 1024;

fn bounded_text(value: &str, max_bytes: usize, marker: &str) -> String {
    if value.len() <= max_bytes {
        return value.to_string();
    }
    let budget = max_bytes.saturating_sub(marker.len());
    let mut end = budget.min(value.len());
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = value[..end].to_string();
    out.push_str(marker);
    out
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitSummary {
    pub repo_path: String,
    pub repo_name: String,
    pub hash: String,
    pub short_hash: String,
    pub summary: String,
    pub author: String,
    pub email: String,
    /// Unix seconds.
    pub timestamp: i64,
    /// Branch-name hint for commits NOT reachable from the currently checked-out
    /// branch. None means "this commit is on the branch you're already on" (or
    /// HEAD is detached, in which case we suppress all labels to avoid noise).
    pub branch_label: Option<String>,
    /// True if this commit has more than one parent (merge commit).
    pub is_merge: bool,
    /// True if any tag points at this commit.
    pub is_tagged: bool,
    /// Parent commit SHAs in order (first parent, then merge parents).
    /// Needed by the DAG lane drawer in single-repo mode.
    pub parents: Vec<String>,
    /// Full commit message (summary + body). Surfaced in the inline
    /// expansion. Empty string if libgit2 couldn't decode it.
    pub message: String,
    /// Remote-tracking ref shorthand (e.g. "origin/main") that points at
    /// this exact commit. Read from local refs (git.rs never fetches; refs
    /// are refreshed by the user's IDE/CLI or the optional auto-fetch in
    /// `fetch.rs`). None when no remote ref points here. Kept separate from
    /// `branch_label` because remote tip identity is "this commit IS the
    /// tip of origin/X", not "this commit is somewhere on origin/X".
    pub remote_tip_label: Option<String>,
    /// If multiple remote refs point at this same commit (e.g. origin/main
    /// and origin/release), this is the count of additional ones beyond
    /// `remote_tip_label`. UI renders `+N` after the badge.
    pub remote_tip_extra_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFile {
    pub path: String,
    pub old_path: Option<String>,
    pub insertions: usize,
    pub deletions: usize,
    /// "modified" | "new" | "renamed" | "deleted" | "typechange"
    pub status: String,
    /// True if libgit2 (or our extension heuristic) thinks this file is
    /// binary — patch text won't be useful, image preview may be.
    pub is_binary: bool,
    /// Byte size on disk in the parent commit (None for newly-added files).
    pub old_size: Option<u64>,
    /// Byte size in this commit (None for deleted files).
    pub new_size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFileBlobs {
    pub old_base64: Option<String>,
    pub new_base64: Option<String>,
    /// Hint so the frontend can pick the right MIME type.
    pub extension: String,
    /// True if either side is a Git LFS pointer (the actual blob lives in
    /// LFS storage; gitwink doesn't fetch LFS in v0.1).
    pub is_lfs: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfo {
    /// Display name. For local refs this is the branch shorthand (e.g.
    /// `"main"`); for remote-tracking refs it includes the remote prefix
    /// (e.g. `"origin/main"`).
    pub name: String,
    /// Fully qualified ref name (`refs/heads/<n>` or `refs/remotes/<n>`).
    /// Used as the wire identifier when filtering — disambiguates local
    /// branches whose shorthand happens to collide with a remote ref
    /// shorthand and vice versa.
    pub ref_name: String,
    /// `"local"` or `"remote"`. Frontend uses this to group the two in
    /// the BranchChip dropdown without re-parsing `ref_name`.
    pub kind: String,
    pub tip_hash: String,
    pub is_head: bool,
    /// Reachable-commit count, or `None` when not yet computed. The cheap
    /// `list_branches` path leaves this null (counting is a per-branch
    /// history walk); the frontend fills it in lazily for visible rows via
    /// `count_branch_commits`.
    pub commit_count: Option<u32>,
    /// Unix seconds of the tip commit, for "recent activity" sort.
    pub last_activity: i64,
}

/// Snapshot of the current branch's relation to its upstream remote-tracking
/// ref. Computed from local files only — the counts reflect the last fetch
/// (the user's IDE/CLI, or the optional auto-fetch in `fetch.rs`), not a live
/// remote query.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpstreamStatus {
    /// Local branch name (the currently checked-out branch).
    pub local_branch: String,
    /// Upstream ref shorthand, e.g. "origin/main".
    pub upstream: String,
    /// Commits on local that aren't on upstream. Capped at 99.
    pub ahead: u32,
    /// Commits on upstream that aren't on local. Capped at 99.
    pub behind: u32,
    /// True when the count was clamped at 99.
    pub ahead_capped: bool,
    pub behind_capped: bool,
    /// Unix seconds of `<gitdir>/FETCH_HEAD` mtime, when present. Useful for
    /// a "last fetch was N days ago" hint in the UI. None if FETCH_HEAD
    /// doesn't exist yet (e.g. clone with no follow-up fetch).
    pub last_fetch_unix: Option<i64>,
}

/// Extension classification. We override libgit2's is_binary heuristic for
/// well-known text formats (Unity YAML can include NUL bytes inside long
/// fields and gets misclassified) and for unambiguous binaries.
fn extension_lc(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default()
}

fn is_known_text_ext(ext: &str) -> bool {
    matches!(
        ext,
        // Unity assets, all YAML in Force Text mode (Unity default).
        "unity" | "prefab" | "asset" | "mat" | "anim" | "controller" | "meta"
        | "asmdef" | "asmref" | "uxml" | "uss" | "shader" | "cginc"
        // Generic text
        | "yaml" | "yml" | "json" | "json5" | "toml" | "xml" | "html" | "htm"
        | "md" | "markdown" | "txt" | "csv" | "tsv" | "ini" | "cfg" | "conf"
        // Code
        | "rs" | "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs"
        | "py" | "rb" | "go" | "java" | "kt" | "swift" | "c" | "h" | "cc"
        | "cpp" | "hpp" | "cs" | "fs" | "vb" | "scala" | "clj" | "lua" | "sh"
        | "bash" | "zsh" | "fish" | "ps1" | "psm1" | "bat" | "cmd"
        | "css" | "scss" | "sass" | "less"
        | "sql" | "graphql" | "gql" | "proto"
        | "dockerfile" | "gitignore" | "gitattributes" | "editorconfig"
    )
}

fn is_known_binary_ext(ext: &str) -> bool {
    matches!(
        ext,
        // Images (preview handled separately)
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "tiff" | "tif"
        // Audio / video
        | "mp3" | "mp4" | "wav" | "ogg" | "flac" | "m4a" | "mov" | "avi" | "webm" | "mkv"
        // Archives / packages
        | "zip" | "tar" | "gz" | "bz2" | "xz" | "rar" | "7z" | "jar" | "war"
        // Executables / libs
        | "exe" | "dll" | "so" | "dylib" | "a" | "o" | "obj" | "lib"
        // Documents / design
        | "pdf" | "psd" | "ai" | "sketch" | "fig"
        // 3D / Unity-shipped binaries
        | "fbx" | "blend" | "stl" | "ply" | "dae" | "max" | "ma" | "mb" | "abc"
        // Fonts
        | "ttf" | "otf" | "woff" | "woff2" | "eot"
        // Other
        | "db" | "sqlite" | "sqlite3" | "wasm"
    )
}

fn classify_binary(path: &str, git2_says_binary: bool) -> bool {
    let ext = extension_lc(path);
    if is_known_text_ext(&ext) {
        return false;
    }
    if is_known_binary_ext(&ext) {
        return true;
    }
    git2_says_binary
}

/// Return the changed file list for the given commit. Compares against the
/// first parent (root commit compares against an empty tree). Renames are
/// detected via libgit2's similarity heuristic.
pub fn changed_files(repo_path: &Path, commit_hash: &str) -> Result<Vec<ChangedFile>> {
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;
    let oid = git2::Oid::from_str(commit_hash).context("parse commit hash")?;
    let commit = repo.find_commit(oid).context("find commit")?;

    let new_tree = commit.tree().context("commit tree")?;
    let old_tree = if commit.parent_count() > 0 {
        Some(commit.parent(0)?.tree()?)
    } else {
        None
    };

    let mut diff = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), None)
        .context("diff trees")?;
    let mut find_opts = git2::DiffFindOptions::new();
    find_opts.renames(true).copies(false);
    diff.find_similar(Some(&mut find_opts)).ok();

    // Per-path line counts via a single print pass.
    let mut counts: std::collections::HashMap<String, (usize, usize)> =
        std::collections::HashMap::new();
    diff.print(git2::DiffFormat::Patch, |delta, _, line| {
        let p = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let entry = counts.entry(p).or_insert((0, 0));
        match line.origin() {
            '+' => entry.0 += 1,
            '-' => entry.1 += 1,
            _ => {}
        }
        true
    })
    .ok();

    let mut out: Vec<ChangedFile> = Vec::new();
    for delta in diff.deltas() {
        let new_path = delta
            .new_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();
        let old_path = delta
            .old_file()
            .path()
            .map(|p| p.to_string_lossy().into_owned());

        let status = match delta.status() {
            git2::Delta::Added => "new",
            git2::Delta::Deleted => "deleted",
            git2::Delta::Renamed => "renamed",
            git2::Delta::Copied => "copied",
            git2::Delta::Typechange => "typechange",
            _ => "modified",
        }
        .to_string();

        let display_path = if status == "deleted" {
            old_path.clone().unwrap_or_else(|| new_path.clone())
        } else {
            new_path.clone()
        };

        let (insertions, deletions) = counts
            .get(&display_path)
            .or_else(|| old_path.as_ref().and_then(|p| counts.get(p)))
            .copied()
            .unwrap_or((0, 0));

        let git2_binary = delta.new_file().is_binary() || delta.old_file().is_binary();
        let is_binary = classify_binary(&display_path, git2_binary);
        // delta.{new,old}_file().size() routinely returns 0 in git2 when the
        // blob wasn't loaded; read the ODB HEADER for the size — find_blob
        // would inflate the whole object (multi-MB for vendored bundles,
        // ×2 sides × thousands of files on a vendor-bump commit).
        let blob_size = |oid: git2::Oid| -> Option<u64> {
            repo.odb()
                .and_then(|odb| odb.read_header(oid))
                .map(|(size, _kind)| size as u64)
                .ok()
        };
        let new_size = if delta.new_file().exists() {
            blob_size(delta.new_file().id())
        } else {
            None
        };
        let old_size = if delta.old_file().exists() {
            blob_size(delta.old_file().id())
        } else {
            None
        };

        out.push(ChangedFile {
            path: display_path,
            old_path: if status == "renamed" { old_path } else { None },
            insertions,
            deletions,
            status,
            is_binary,
            old_size,
            new_size,
        });
    }

    Ok(out)
}

/// Load the raw bytes of a file at the given commit (new side) and at its
/// first parent (old side), returning both base64-encoded so the frontend
/// can render them as data: URLs. Limits each side to MAX_BLOB_BYTES to keep
/// the IPC payload reasonable.
pub fn commit_file_blobs(
    repo_path: &Path,
    commit_hash: &str,
    file_path: &str,
    old_path: Option<&str>,
) -> Result<CommitFileBlobs> {
    use base64::{engine::general_purpose, Engine as _};

    const MAX_BLOB_BYTES: usize = 4 * 1024 * 1024; // 4 MB per side

    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;
    let oid = git2::Oid::from_str(commit_hash).context("parse commit hash")?;
    let commit = repo.find_commit(oid).context("find commit")?;

    fn looks_like_lfs(bytes: &[u8]) -> bool {
        bytes.starts_with(b"version https://git-lfs.github.com/spec/v1")
    }

    /// Pull `oid sha256:<hex>` out of an LFS pointer text body.
    fn parse_lfs_oid(bytes: &[u8]) -> Option<String> {
        let text = std::str::from_utf8(bytes).ok()?;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("oid sha256:") {
                let oid = rest.trim();
                if oid.len() == 64 && oid.chars().all(|c| c.is_ascii_hexdigit()) {
                    return Some(oid.to_string());
                }
            }
        }
        None
    }

    /// Look up the actual LFS object bytes in the local cache
    /// (`<gitdir>/lfs/objects/<2>/<2>/<oid>`). `git clone` of an LFS repo
    /// usually pulls these automatically; if missing, we hand back None
    /// and let the UI explain.
    ///
    /// For linked worktrees, `repo.path()` is the worktree-specific gitdir
    /// (`<main>/.git/worktrees/<name>/`) and LFS objects live under the
    /// main common dir. We resolve that via the `commondir` file git itself
    /// writes next to the worktree gitdir — this works on every git2
    /// version without needing a method that came and went in the API.
    fn load_lfs_object(repo: &Repository, oid_hex: &str) -> Option<Vec<u8>> {
        let rel: std::path::PathBuf = std::path::Path::new("lfs")
            .join("objects")
            .join(&oid_hex[0..2])
            .join(&oid_hex[2..4])
            .join(oid_hex);

        let gitdir = repo.path();
        if let Ok(bytes) = std::fs::read(gitdir.join(&rel)) {
            return Some(bytes);
        }

        let commondir_file = gitdir.join("commondir");
        if let Ok(text) = std::fs::read_to_string(&commondir_file) {
            // Path-aware trim: only strip line terminators. `trim()` would
            // also remove leading/trailing whitespace from the path itself.
            let raw = text.trim_end_matches(['\n', '\r']);
            let p = std::path::Path::new(raw);
            let common = if p.is_absolute() {
                p.to_path_buf()
            } else {
                gitdir.join(p)
            };
            if let Ok(bytes) = std::fs::read(common.join(&rel)) {
                return Some(bytes);
            }
        }

        None
    }

    fn load(
        repo: &Repository,
        tree: &git2::Tree<'_>,
        path: &str,
        max: usize,
    ) -> (Option<String>, bool) {
        let entry = match tree.get_path(std::path::Path::new(path)) {
            Ok(e) => e,
            Err(_) => return (None, false),
        };
        let blob = match repo.find_blob(entry.id()) {
            Ok(b) => b,
            Err(_) => return (None, false),
        };
        let content = blob.content();

        if looks_like_lfs(content) {
            // Try the local LFS cache; if the user has `git lfs pull`-ed
            // (which `git clone` does by default on LFS repos) the actual
            // bytes will be there.
            if let Some(oid_hex) = parse_lfs_oid(content) {
                if let Some(actual) = load_lfs_object(repo, &oid_hex) {
                    if actual.len() > max {
                        return (None, false);
                    }
                    return (
                        Some(base64::engine::general_purpose::STANDARD.encode(&actual)),
                        false,
                    );
                }
            }
            return (None, true);
        }

        if content.len() > max {
            return (None, false);
        }
        (
            Some(base64::engine::general_purpose::STANDARD.encode(content)),
            false,
        )
    }

    let new_tree = commit.tree()?;
    let (new_base64, new_is_lfs) = load(&repo, &new_tree, file_path, MAX_BLOB_BYTES);

    let (old_base64, old_is_lfs) = if commit.parent_count() > 0 {
        let parent_tree = commit.parent(0)?.tree()?;
        let probe = old_path.unwrap_or(file_path);
        load(&repo, &parent_tree, probe, MAX_BLOB_BYTES)
    } else {
        (None, false)
    };

    let extension = std::path::Path::new(file_path)
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    // Touch the engine constant so optimizer/linter doesn't see unused.
    let _ = general_purpose::STANDARD;

    Ok(CommitFileBlobs {
        old_base64,
        new_base64,
        extension,
        is_lfs: new_is_lfs || old_is_lfs,
    })
}

/// Unified diff text (git's standard patch output) for a single file in a
/// commit, against that commit's first parent. Root commit compares against
/// the empty tree.
pub fn file_diff(
    repo_path: &Path,
    commit_hash: &str,
    file_path: &str,
    context_lines: u32,
) -> Result<String> {
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;
    let oid = git2::Oid::from_str(commit_hash).context("parse commit hash")?;
    let commit = repo.find_commit(oid).context("find commit")?;

    let new_tree = commit.tree()?;
    let old_tree = if commit.parent_count() > 0 {
        Some(commit.parent(0)?.tree()?)
    } else {
        None
    };

    let mut opts = git2::DiffOptions::new();
    opts.pathspec(file_path);
    // The pathspec must match the LITERAL path. Default pathspec semantics
    // are fnmatch, so a bracketed filename like pages/[id].tsx is a character
    // class that also matches pages/i.tsx etc. — pulling other files' hunks
    // into this file's diff (and poisoning the diff cache with it).
    opts.disable_pathspec_match(true);
    // Caller-controlled: 3 for the default hunk view, larger to expand
    // context, very large (≥ file length) for a whole-file diff. git merges
    // hunks as the context grows, so the same patch output renders a full
    // file with add/delete tinting intact — no separate viewer needed.
    opts.context_lines(context_lines);
    // Force text patch generation. libgit2's binary heuristic misfires on
    // big Unity YAML scenes (NUL bytes in long fields) and would otherwise
    // emit "Binary files differ" for files that are perfectly diffable.
    // Frontend only calls file_diff for paths it has already classified as
    // text, so forcing text here is safe.
    opts.force_text(true);
    let mut diff = repo
        .diff_tree_to_tree(old_tree.as_ref(), Some(&new_tree), Some(&mut opts))
        .context("diff trees")?;

    let mut find_opts = git2::DiffFindOptions::new();
    find_opts.renames(true);
    diff.find_similar(Some(&mut find_opts)).ok();

    // Cap the emitted patch. "Full" context (context_lines ≫ file length) on
    // a huge generated/lock/minified file would otherwise produce a multi-MB
    // string that floods the IPC channel + the frontend parser/DOM. The
    // frontend disables "Full" for large files; this is the backend's own net.
    const MAX_DIFF_BYTES: usize = 5 * 1024 * 1024;
    let mut out = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        if out.len() >= MAX_DIFF_BYTES {
            return false; // already at the cap
        }
        let origin = line.origin();
        let is_header = matches!(origin, 'F' | 'H');
        // Non-header lines carry their +/-/space prefix as the origin char.
        if !is_header {
            out.push(origin);
        }
        let content = std::str::from_utf8(line.content()).unwrap_or("");
        let budget = MAX_DIFF_BYTES.saturating_sub(out.len());
        if content.len() <= budget {
            out.push_str(content);
            true
        } else {
            // A single oversized line (minified/generated file): push a
            // UTF-8-safe prefix that fits the budget and stop, so one line
            // can't blow far past the cap.
            let mut end = budget;
            while end > 0 && !content.is_char_boundary(end) {
                end -= 1;
            }
            out.push_str(&content[..end]);
            false
        }
    })
    .ok();

    Ok(out)
}

/// Cheap revwalk-based commit count + tip activity time for a single ref
/// tip. The cap exists so a deep history can't block the branch-picker
/// render. Local refs are the user's working surface — give them the more
/// generous cap. Remote refs are mostly here so the user can focus on
/// them; the count is a secondary cue, so we cap much harder. With 100
/// remote refs on a monorepo this difference (100×500 vs 100×5_000) is
/// the difference between "branch list ready immediately" and "branch
/// chip lags for a second".
const LOCAL_REF_COUNT_CAP: usize = 5_000;
const REMOTE_REF_COUNT_CAP: usize = 500;

/// Count commits reachable from `tip`, capped. Bounds a single branch's
/// walk; `cap + 1` is returned as the "more than cap" sentinel so the UI can
/// render e.g. "5000+".
fn ref_commit_count(repo: &Repository, tip: Oid, cap: usize) -> u32 {
    let mut count = 0u32;
    if let Ok(mut rw) = repo.revwalk() {
        // libgit2: set_sorting MUST be called before any push/hide.
        let _ = rw.set_sorting(Sort::TIME);
        if rw.push(tip).is_ok() {
            for (i, oid_res) in rw.enumerate() {
                if oid_res.is_err() {
                    continue;
                }
                count = (i + 1) as u32;
                if i >= cap {
                    count = (cap + 1) as u32;
                    break;
                }
            }
        }
    }
    count
}

/// Return every local branch AND every remote-tracking ref in the repo.
/// Remote refs come from `refs/remotes/*`, skipping any `*/HEAD` symbolic
/// alias. Both share the same `BranchInfo` shape, with `kind` set so the
/// frontend can render them in separate sections.
///
/// CHEAP path: per branch we only peel the tip to its commit for the "last
/// activity" timestamp (O(1)). Reachable-commit COUNTS are deliberately
/// omitted (`commit_count: None`) — counting is a per-branch history walk,
/// so a repo with thousands of branches would otherwise pay
/// O(branches × history) just to open the chip. The frontend fills counts in
/// lazily for the rows actually on screen via `count_branch_commits`.
pub fn list_branches(repo_path: &Path) -> Result<Vec<BranchInfo>> {
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;

    let head_branch: Option<String> = if repo.head_detached().unwrap_or(true) {
        None
    } else {
        repo.head()
            .ok()
            .and_then(|h| h.shorthand().map(|s| s.to_string()))
    };

    // Tip → commit time. Cheap: loads one commit object, no walk.
    let tip_time = |tip: Oid| -> i64 {
        repo.find_commit(tip)
            .map(|c| c.time().seconds())
            .unwrap_or(0)
    };

    let mut out: Vec<BranchInfo> = Vec::new();

    // Local heads first.
    if let Ok(refs) = repo.references_glob("refs/heads/*") {
        for r in refs.flatten() {
            let Some(name) = r.shorthand().map(|s| s.to_string()) else {
                continue;
            };
            let Some(tip) = r.target() else { continue };
            out.push(BranchInfo {
                name: name.clone(),
                ref_name: format!("refs/heads/{name}"),
                kind: "local".to_string(),
                tip_hash: tip.to_string(),
                is_head: Some(&name) == head_branch.as_ref(),
                commit_count: None,
                last_activity: tip_time(tip),
            });
        }
    }

    // Remote-tracking refs (local file reads — no fetch). Skip the symbolic
    // `*/HEAD` alias and any ref the target isn't reachable for.
    if let Ok(refs) = repo.references_glob("refs/remotes/*") {
        for r in refs.flatten() {
            let Some(name) = r.shorthand().map(|s| s.to_string()) else {
                continue;
            };
            if name.ends_with("/HEAD") {
                continue;
            }
            let Some(tip) = r.target() else { continue };
            out.push(BranchInfo {
                name: name.clone(),
                ref_name: format!("refs/remotes/{name}"),
                kind: "remote".to_string(),
                tip_hash: tip.to_string(),
                is_head: false,
                commit_count: None,
                last_activity: tip_time(tip),
            });
        }
    }

    // Sort by recency overall; UI groups by `kind` so this just controls
    // intra-group ordering.
    out.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
    Ok(out)
}

/// Lazily-computed reachable-commit count for one branch ref. The frontend
/// requests these for the BranchChip rows currently on screen, so we never
/// walk a branch the user doesn't look at.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BranchCommitCount {
    pub ref_name: String,
    pub count: u32,
}

/// Count reachable commits for each given branch ref (local or
/// remote-tracking), capping per ref by kind. Unresolvable refs are skipped.
pub fn count_branch_commits(
    repo_path: &Path,
    ref_names: &[String],
) -> Result<Vec<BranchCommitCount>> {
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;
    let mut out = Vec::with_capacity(ref_names.len());
    for ref_name in ref_names {
        let cap = if ref_name.starts_with("refs/remotes/") {
            REMOTE_REF_COUNT_CAP
        } else {
            LOCAL_REF_COUNT_CAP
        };
        let Ok(reference) = repo.find_reference(ref_name) else {
            continue;
        };
        let Some(tip) = reference.target() else {
            continue;
        };
        out.push(BranchCommitCount {
            ref_name: ref_name.clone(),
            count: ref_commit_count(&repo, tip, cap),
        });
    }
    Ok(out)
}

/// Cap shown to the user when ahead/behind crosses 99. We clamp the integer
/// itself rather than displaying "99+" purely in CSS so the IPC payload
/// stays trivially small even on diverged branches.
const AHEAD_BEHIND_CAP: usize = 99;

/// Report how a given local branch relates to its upstream remote-tracking
/// ref. When `branch_name` is None, defaults to the currently checked-out
/// branch. Returns None for detached HEAD (no branch_name given), for an
/// unknown branch name, or when no upstream is configured AND no
/// `origin/<branch>` fallback exists.
///
/// PURE READ. We only inspect refs already written locally (by the user's
/// IDE/CLI or gitwink's optional auto-fetch); this function — and git.rs as a
/// whole — never reaches the network. The optional panel-open `git fetch`
/// lives in `fetch.rs`, outside this read-only module.
pub fn current_upstream_status(
    repo_path: &Path,
    branch_name: Option<&str>,
) -> Result<Option<UpstreamStatus>> {
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;

    // Resolve the local branch we're computing for.
    let (local_branch_name, local_oid) = match branch_name {
        Some(name) => {
            let Ok(branch) = repo.find_branch(name, BranchType::Local) else {
                return Ok(None);
            };
            let Some(oid) = branch.get().target() else {
                return Ok(None);
            };
            (name.to_string(), oid)
        }
        None => {
            if repo.head_detached().unwrap_or(false) {
                return Ok(None);
            }
            let head = match repo.head() {
                Ok(h) => h,
                Err(_) => return Ok(None),
            };
            let Some(name) = head.shorthand().map(|s| s.to_string()) else {
                return Ok(None);
            };
            let Some(oid) = head.target() else {
                return Ok(None);
            };
            (name, oid)
        }
    };

    // Resolve upstream: prefer the configured upstream (`branch.<n>.remote`
    // + `branch.<n>.merge`), fall back to `origin/<local_branch_name>` if
    // the user just clones without `set-upstream`.
    let (upstream_label, upstream_oid) = {
        let configured = repo
            .find_branch(&local_branch_name, BranchType::Local)
            .ok()
            .and_then(|b| b.upstream().ok())
            .and_then(|ub| {
                let name = ub.name().ok().flatten().map(|s| s.to_string())?;
                let oid = ub.get().target()?;
                Some((name, oid))
            });

        if let Some(pair) = configured {
            pair
        } else {
            let fallback_short = format!("origin/{local_branch_name}");
            let fallback_full = format!("refs/remotes/{fallback_short}");
            match repo
                .find_reference(&fallback_full)
                .ok()
                .and_then(|r| r.target().map(|oid| (fallback_short.clone(), oid)))
            {
                Some(pair) => pair,
                None => return Ok(None),
            }
        }
    };

    let (ahead_raw, behind_raw) = repo
        .graph_ahead_behind(local_oid, upstream_oid)
        .context("graph_ahead_behind")?;

    let ahead_capped = ahead_raw > AHEAD_BEHIND_CAP;
    let behind_capped = behind_raw > AHEAD_BEHIND_CAP;
    let ahead = ahead_raw.min(AHEAD_BEHIND_CAP) as u32;
    let behind = behind_raw.min(AHEAD_BEHIND_CAP) as u32;

    // FETCH_HEAD mtime lives in the *common* git dir for worktrees, so
    // resolve commondir manually (git2 0.19 doesn't expose Repository::commondir).
    // For a normal clone this is just repo.path().
    let gitdir = repo.path();
    let common = std::fs::read_to_string(gitdir.join("commondir"))
        .ok()
        .map(|text| {
            let raw = text.trim_end_matches(['\n', '\r']);
            let p = std::path::Path::new(raw);
            if p.is_absolute() {
                p.to_path_buf()
            } else {
                gitdir.join(p)
            }
        })
        .unwrap_or_else(|| gitdir.to_path_buf());

    let last_fetch_unix = std::fs::metadata(common.join("FETCH_HEAD"))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|st| st.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);

    Ok(Some(UpstreamStatus {
        local_branch: local_branch_name,
        upstream: upstream_label,
        ahead,
        behind,
        ahead_capped,
        behind_capped,
        last_fetch_unix,
    }))
}

/// Branch label sketch: how far back into a single branch's history we look
/// when answering "is this commit on this branch?". 5k commits comfortably
/// covers any normally-active branch; older commits get no label, which is
/// fine — the timeline is for recent work, and label is a hint, not truth.
const BRANCH_LABEL_SCAN_CAP: usize = 5_000;

/// How many consecutive sub-cutoff commits a TIME-sorted walk tolerates
/// before it stops. Committer dates are NOT monotone along ancestry
/// (`rebase --committer-date-is-author-date`, `git am` imports, skewed CI
/// clocks), and libgit2's GIT_SORT_TIME is a frontier priority queue — a
/// skew-OLD-dated child pops BEFORE its newer-dated ancestors are even
/// discovered. Breaking on first sight of an old commit therefore hides
/// live in-window history sitting behind it; since the walk result doubles
/// as the live set `cache::reconcile_repo_commits` trusts for DELETION,
/// that would be destructive, not just cosmetic. git's own `--since`
/// handling keeps walking with a slop for exactly this reason.
const CUTOFF_SLOP: usize = 32;

/// A `refs/remotes/<remote>/<short>` ref pinned to a single commit. We don't
/// walk this — it's just a tip OID we may use as a revwalk root and as a
/// per-commit badge.
#[derive(Clone)]
struct RemoteTip {
    /// Shorthand label, e.g. `"origin/main"`.
    label: String,
    oid: Oid,
    /// Tip commit time, used to sort + filter against the request window.
    last_activity: i64,
}

/// Badge metadata for a single commit: the primary remote label plus an
/// extra-count hint when multiple remote refs collapse onto the same OID.
#[derive(Clone)]
struct RemoteBadge {
    primary: String,
    extra_count: usize,
}

/// Read every remote-tracking ref from disk (NO network). The BranchChip calls
/// this scope “All branches”, so dropping non-origin remotes, old tips or a
/// 65th ref is a contract violation; the row cap bounds output, while revwalk
/// root insertion itself is cheap and deduplicated by libgit2.
fn collect_remote_tips_with_status(repo: &Repository) -> (Vec<RemoteTip>, bool) {
    let mut out: Vec<RemoteTip> = Vec::new();
    let mut had_errors = false;
    let Ok(iter) = repo.references_glob("refs/remotes/*") else {
        return (out, true);
    };
    for item in iter {
        let Ok(r) = item else {
            had_errors = true;
            continue;
        };
        let Some(label) = r.shorthand().map(str::to_string) else {
            continue;
        };
        // Symbolic ref like `origin/HEAD` resolves to another remote ref
        // — keep the named target only, drop the symbolic alias.
        if label.ends_with("/HEAD") {
            continue;
        }
        let Some(oid) = r.target() else { continue };
        let Ok(commit) = repo.find_commit(oid) else {
            had_errors = true;
            continue;
        };
        let ts = commit.time().seconds();
        out.push(RemoteTip {
            label,
            oid,
            last_activity: ts,
        });
    }
    out.sort_by(|a, b| {
        b.last_activity
            .cmp(&a.last_activity)
            .then_with(|| a.label.cmp(&b.label))
    });
    (out, had_errors)
}

/// Group remote tips by the commit OID they point at, so each commit gets
/// one `(primary, extra_count)` pair instead of N rows. Primary label is
/// chosen deterministically (sorted ascending) for stable UI.
fn build_remote_badge_map(tips: &[RemoteTip]) -> HashMap<Oid, RemoteBadge> {
    let mut grouped: HashMap<Oid, Vec<String>> = HashMap::new();
    for tip in tips {
        grouped.entry(tip.oid).or_default().push(tip.label.clone());
    }
    grouped
        .into_iter()
        .map(|(oid, mut labels)| {
            labels.sort();
            let primary = labels[0].clone();
            let extra_count = labels.len().saturating_sub(1);
            (
                oid,
                RemoteBadge {
                    primary,
                    extra_count,
                },
            )
        })
        .collect()
}

/// Walk `tip` and report which of `targets` it reaches, bounded by
/// `scan_cap` so a branch with deep history can't pin first-paint — and by
/// `floor_ts`: in a TIME-sorted walk, once commits are older than the oldest
/// target (all callers know it), no remaining target can still appear, so
/// the walk stops there (CUTOFF_SLOP tolerates clock skew). Without this
/// bound, ONE unreachable target (the normal state right after an auto-fetch
/// puts remote-only commits in the list) degenerated every branch's walk to
/// the full 5,000-pop cap on every panel summon. 0 disables the floor.
fn limited_reachable_membership_impl(
    repo: &Repository,
    tip: Oid,
    targets: &HashSet<Oid>,
    scan_cap: usize,
    floor_ts: i64,
    cancellation: Option<&CancellationToken>,
) -> Option<HashSet<Oid>> {
    let mut found: HashSet<Oid> = HashSet::new();
    if targets.is_empty() {
        return Some(found);
    }
    let Ok(mut rw) = repo.revwalk() else {
        return Some(found);
    };
    // libgit2: set_sorting resets the walker, so it MUST run before push.
    // Doing it after push silently empties the walker.
    if rw.set_sorting(Sort::TIME).is_err() {
        return Some(found);
    }
    if rw.push(tip).is_err() {
        return Some(found);
    }

    let mut slop = 0usize;
    for (i, oid_res) in rw.enumerate() {
        if cancellation.is_some_and(CancellationToken::is_cancelled) {
            return None;
        }
        if i >= scan_cap || found.len() == targets.len() {
            break;
        }
        let Ok(oid) = oid_res else {
            continue;
        };
        if targets.contains(&oid) {
            found.insert(oid);
            slop = 0;
            continue;
        }
        if floor_ts > 0 {
            if let Ok(c) = repo.find_commit(oid) {
                if c.time().seconds() < floor_ts {
                    slop += 1;
                    if slop >= CUTOFF_SLOP {
                        break;
                    }
                } else {
                    slop = 0;
                }
            }
        }
    }
    Some(found)
}

/// Compute branch labels for a small set of target commits.
///
/// Return shape:
///   - missing entry  → no branch within scan_cap contains this commit
///   - Some(None)     → reachable from HEAD; label is suppressed (spec)
///   - Some(Some(n))  → first non-current branch (in glob order) that contains it
///
/// This replaces the previous "build full HashSets of every commit on every
/// branch" approach, which was O(branches × history) regardless of how many
/// rows we were going to return.
fn compute_branch_labels(
    repo: &Repository,
    head_branch_name: Option<&str>,
    targets: &HashSet<Oid>,
    // Timestamp of the OLDEST target (0 = unknown): every membership walk
    // stops once it drops below this, instead of paying the full scan cap
    // per branch whenever any target is unreachable (see
    // limited_reachable_membership).
    floor_ts: i64,
) -> HashMap<Oid, Option<String>> {
    compute_branch_labels_impl(repo, head_branch_name, targets, floor_ts, None).unwrap_or_default()
}

fn compute_branch_labels_cancellable(
    repo: &Repository,
    head_branch_name: Option<&str>,
    targets: &HashSet<Oid>,
    floor_ts: i64,
    cancellation: &CancellationToken,
) -> Option<HashMap<Oid, Option<String>>> {
    compute_branch_labels_impl(
        repo,
        head_branch_name,
        targets,
        floor_ts,
        Some(cancellation),
    )
}

fn compute_branch_labels_impl(
    repo: &Repository,
    head_branch_name: Option<&str>,
    targets: &HashSet<Oid>,
    floor_ts: i64,
    cancellation: Option<&CancellationToken>,
) -> Option<HashMap<Oid, Option<String>>> {
    let mut labels: HashMap<Oid, Option<String>> = HashMap::new();
    if targets.is_empty() {
        return Some(labels);
    }

    // Detached HEAD or no HEAD → suppress all labels (existing behaviour).
    let Some(head_name) = head_branch_name else {
        for &oid in targets {
            if cancellation.is_some_and(CancellationToken::is_cancelled) {
                return None;
            }
            labels.insert(oid, None);
        }
        return Some(labels);
    };

    if let Some(tip) = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| c.id())
    {
        let hits = limited_reachable_membership_impl(
            repo,
            tip,
            targets,
            BRANCH_LABEL_SCAN_CAP,
            floor_ts,
            cancellation,
        )?;
        for oid in hits {
            labels.insert(oid, None);
        }
    }

    let mut remaining: HashSet<Oid> = targets
        .iter()
        .copied()
        .filter(|oid| !labels.contains_key(oid))
        .collect();

    if remaining.is_empty() {
        return Some(labels);
    }

    if let Ok(refs) = repo.references_glob("refs/heads/*") {
        for r in refs.flatten() {
            if cancellation.is_some_and(CancellationToken::is_cancelled) {
                return None;
            }
            if remaining.is_empty() {
                break;
            }
            let Some(name) = r.shorthand().map(|s| s.to_string()) else {
                continue;
            };
            if name.as_str() == head_name {
                continue;
            }
            let Some(tip) = r.target() else {
                continue;
            };
            let hits = limited_reachable_membership_impl(
                repo,
                tip,
                &remaining,
                BRANCH_LABEL_SCAN_CAP,
                floor_ts,
                cancellation,
            )?;
            for oid in hits {
                labels.insert(oid, Some(name.clone()));
                remaining.remove(&oid);
            }
        }
    }

    Some(labels)
}

fn collect_tagged_oids_with_status(repo: &Repository) -> (HashSet<Oid>, bool) {
    let mut set = HashSet::new();
    let mut had_errors = false;
    match repo.references_glob("refs/tags/*") {
        Ok(refs) => {
            for item in refs {
                match item {
                    Ok(reference) => match reference.peel(git2::ObjectType::Commit) {
                        Ok(object) => {
                            set.insert(object.id());
                        }
                        // Tags may validly point at blobs or trees. That is
                        // not a damaged history; only an unreadable target is
                        // an error. `Any` proves the non-commit object exists.
                        Err(_) if reference.peel(git2::ObjectType::Any).is_err() => {
                            had_errors = true
                        }
                        Err(_) => {}
                    },
                    Err(_) => had_errors = true,
                }
            }
        }
        Err(_) => had_errors = true,
    }
    (set, had_errors)
}

/// Cheap change probe for the single-repo view: the (ref, tip) pairs the
/// corresponding `repo_commits` walk would start from, sorted and joined.
/// If this string didn't move since the last pull, the walk would return
/// the same rows — the frontend uses it to skip re-shipping up to 2,000
/// full CommitSummary structs on a plain panel re-summon. Errs on the side
/// of extra fetches (it includes every remote-tracking tip),
/// never on staleness.
pub fn repo_refs_fingerprint(repo_path: &Path, branches: Option<&[String]>) -> Result<String> {
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;
    let mut parts: Vec<String> = Vec::new();

    // HEAD in BOTH arms: repo_commits pushes peeled HEAD as a walk root
    // (detached-HEAD commits — rebase stops, agent worktrees — must show),
    // and branch labels key off the current-branch NAME, so a plain
    // `git switch` must also change the fingerprint.
    if let Ok(head) = repo.head() {
        let name = head.shorthand().unwrap_or("HEAD").to_string();
        if let Ok(c) = head.peel_to_commit() {
            parts.push(format!("HEAD@{name}:{}", c.id()));
        }
    }
    // Tags in both arms: tag badges come from collect_tagged_oids
    // unconditionally, so a new/moved tag must refresh the view too.
    if let Ok(refs) = repo.references_glob("refs/tags/*") {
        for r in refs.flatten() {
            if let (Some(n), Some(oid)) = (r.name(), r.target()) {
                parts.push(format!("{n}:{oid}"));
            }
        }
    }

    match branches {
        Some(sel) => {
            // Resolve exactly like repo_commits' explicit mode: full ref
            // paths pass through, bare names mean refs/heads/{name}.
            for name in sel {
                let full = if name.starts_with("refs/") {
                    name.clone()
                } else {
                    format!("refs/heads/{name}")
                };
                if let Ok(r) = repo.find_reference(&full) {
                    if let Some(oid) = r.target() {
                        parts.push(format!("{full}:{oid}"));
                    }
                }
            }
        }
        None => {
            for glob in ["refs/heads/*", "refs/remotes/*"] {
                if let Ok(refs) = repo.references_glob(glob) {
                    for r in refs.flatten() {
                        if let (Some(n), Some(oid)) = (r.name(), r.target()) {
                            parts.push(format!("{n}:{oid}"));
                        }
                    }
                }
            }
        }
    }
    parts.sort();
    Ok(parts.join("|"))
}

#[derive(Debug)]
pub struct RepoCommitsScan {
    pub rows: Vec<CommitSummary>,
    pub incomplete: bool,
}

/// Walk the given branches (None = all repository refs) in a single
/// repository, deduping by SHA and reporting actual read uncertainty. Reaching
/// the caller's requested time boundary is normal coverage, not incompleteness.
pub fn repo_commits(
    repo_path: &Path,
    branches: Option<&[String]>,
    max_count: usize,
    since_unix_seconds: i64,
) -> Result<RepoCommitsScan> {
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;

    let repo_path_str = repo_path.to_string_lossy().into_owned();
    let repo_name = repo_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Some([]) is an explicit "no branches" selection. Return before tag,
    // HEAD or revwalk work; an empty filter must be both exact and free.
    let explicit_mode = branches.is_some();
    let explicit_names: &[String] = branches.unwrap_or(&[]);
    if explicit_mode && explicit_names.is_empty() {
        return Ok(RepoCommitsScan {
            rows: Vec::new(),
            incomplete: false,
        });
    }

    let detached = repo.head_detached().unwrap_or(true);
    let head_branch_name: Option<String> = if detached {
        None
    } else {
        repo.head()
            .ok()
            .and_then(|h| h.shorthand().map(|s| s.to_string()))
    };

    let (tagged_oids, tag_errors) = collect_tagged_oids_with_status(&repo);

    let mut revwalk = repo.revwalk().context("init revwalk")?;
    // libgit2 contract: set_sorting resets the walker, so it MUST run before
    // any push/hide. Calling it after pushes (as we did in 847ed51) silently
    // drops the pushed starting points.
    revwalk
        .set_sorting(Sort::TIME)
        .context("set revwalk sort")?;
    let mut pushed = 0usize;

    // Remote tips contribute both as revwalk roots (so remote-only commits
    // appear) AND as the source of per-row "remote/X" badges. In explicit
    // mode (user picked specific refs), only the remote refs they picked
    // count. In all-branches mode, auto-discover every remote-tracking tip.
    let mut remote_tips: Vec<RemoteTip> = Vec::new();

    // In explicit mode tags only decorate returned commits. An unrelated bad
    // tag cannot make the selected branch's commit history incomplete. In
    // all-ref mode tags are roots, so an unreadable tag may hide tag-only
    // commits and must be reported.
    let mut had_errors = !explicit_mode && tag_errors;

    if explicit_mode {
        // BranchChip selection — names may be either full ref paths
        // (`refs/heads/...` / `refs/remotes/...`) or bare shorthand for
        // local branches (callers from before the multi-ref change).
        for name in explicit_names {
            let full = if name.starts_with("refs/") {
                name.clone()
            } else {
                format!("refs/heads/{name}")
            };
            let Ok(reference) = repo.find_reference(&full) else {
                continue;
            };
            let Some(oid) = reference.target() else {
                continue;
            };
            if revwalk.push(oid).is_ok() {
                pushed += 1;
                // Picked-remote refs feed the badge map so users see
                // `origin/X` even when their selection is explicit.
                if full.starts_with("refs/remotes/") {
                    if let Some(short) = reference.shorthand() {
                        if let Ok(commit) = repo.find_commit(oid) {
                            remote_tips.push(RemoteTip {
                                label: short.to_string(),
                                oid,
                                last_activity: commit.time().seconds(),
                            });
                        } else {
                            had_errors = true;
                        }
                    }
                }
            } else {
                had_errors = true;
            }
        }
    } else {
        // All-branches mode: every local head + every remote-tracking tip.
        match repo.references_glob("refs/heads/*") {
            Ok(refs) => {
                for item in refs {
                    match item {
                        Ok(reference) => {
                            if let Some(oid) = reference.target() {
                                if revwalk.push(oid).is_ok() {
                                    pushed += 1;
                                } else {
                                    had_errors = true;
                                }
                            }
                        }
                        Err(_) => had_errors = true,
                    }
                }
            }
            Err(_) => had_errors = true,
        }
        let (tips, remote_errors) = collect_remote_tips_with_status(&repo);
        remote_tips = tips;
        had_errors |= remote_errors;
        for tip in &remote_tips {
            if revwalk.push(tip.oid).is_ok() {
                pushed += 1;
            } else {
                had_errors = true;
            }
        }
        for oid in &tagged_oids {
            if revwalk.push(*oid).is_ok() {
                pushed += 1;
            } else {
                had_errors = true;
            }
        }
        // Always include HEAD in all-branches mode — detached-HEAD commits
        // are reachable from no local head and no remote tip (see the same
        // block in recent_commits). Explicit mode deliberately omits this:
        // the user picked specific refs and HEAD wasn't one of them.
        if let Ok(head) = repo.head() {
            if let Ok(head_commit) = head.peel_to_commit() {
                if revwalk.push(head_commit.id()).is_ok() {
                    pushed += 1;
                } else {
                    had_errors = true;
                }
            }
        }
    }

    let remote_badges = build_remote_badge_map(&remote_tips);

    if pushed == 0 {
        // Explicit mode with all refs stale/invalid — and the all-branches
        // case where nothing at all was walkable (unborn HEAD, no refs):
        // "nothing matches" is the honest answer.
        return Ok(RepoCommitsScan {
            rows: Vec::new(),
            incomplete: had_errors,
        });
    }

    // Pass 1: collect just the rows we'll return. Cheap — bounded by
    // max_count and the since cutoff, with the same CUTOFF_SLOP tolerance
    // as recent_commits (a hard break on the first skew-old commit would
    // truncate the visible history behind it).
    let mut raw: Vec<(Oid, git2::Commit<'_>)> = Vec::with_capacity(max_count);
    let mut slop = 0usize;
    for oid in revwalk {
        if raw.len() >= max_count {
            break;
        }
        let oid = match oid {
            Ok(o) => o,
            Err(_) => {
                had_errors = true;
                continue;
            }
        };
        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => {
                had_errors = true;
                continue;
            }
        };
        if commit.time().seconds() < since_unix_seconds {
            slop += 1;
            if slop >= CUTOFF_SLOP {
                break;
            }
            continue;
        }
        slop = 0;
        raw.push((oid, commit));
    }

    // Pass 2: branch labels, computed only against the OIDs we'll actually
    // return — bounded per-branch, short-circuits when all targets resolve,
    // and floored at the oldest returned commit's time.
    let targets: HashSet<Oid> = raw.iter().map(|(oid, _)| *oid).collect();
    let label_floor = raw
        .iter()
        .map(|(_, c)| c.time().seconds())
        .min()
        .unwrap_or(0);
    let labels = compute_branch_labels(&repo, head_branch_name.as_deref(), &targets, label_floor);

    let mut out: Vec<CommitSummary> = Vec::with_capacity(raw.len());
    for (oid, commit) in raw {
        let hash = oid.to_string();
        let short_hash: String = hash.chars().take(7).collect();
        let summary = bounded_text(
            commit.summary().unwrap_or(""),
            MAX_DIRECT_SUMMARY_BYTES,
            "…",
        );
        let author = commit.author();
        let parents: Vec<String> = commit.parent_ids().map(|p| p.to_string()).collect();
        let message = bounded_text(
            commit.message().unwrap_or(""),
            MAX_DIRECT_MESSAGE_BYTES,
            "\n… [message truncated]",
        );
        let ts = commit.time().seconds();
        let branch_label = labels.get(&oid).cloned().flatten();
        let (remote_tip_label, remote_tip_extra_count) = match remote_badges.get(&oid) {
            Some(b) => (Some(b.primary.clone()), b.extra_count),
            None => (None, 0),
        };

        out.push(CommitSummary {
            repo_path: repo_path_str.clone(),
            repo_name: repo_name.clone(),
            hash,
            short_hash,
            summary,
            author: author.name().unwrap_or("").to_string(),
            email: author.email().unwrap_or("").to_string(),
            timestamp: ts,
            branch_label,
            is_merge: commit.parent_count() > 1,
            is_tagged: tagged_oids.contains(&oid),
            parents,
            message,
            remote_tip_label,
            remote_tip_extra_count,
        });
    }

    Ok(RepoCommitsScan {
        rows: out,
        incomplete: had_errors,
    })
}

/// Caps for `file_history`: stop at this many MATCHES (a file rarely changes
/// more than a few hundred times), and never examine more than this many
/// commits (bounds the pathological "changed twice in a 100k-commit history"
/// case). Off-UI, like every other walk.
const FILE_HISTORY_MAX_MATCHES: usize = 1000;
const FILE_HISTORY_SCAN_CAP: usize = 50_000;

/// Why a file-history walk stopped before exhausting every reachable commit.
/// The UI must never turn a bounded result into the false claim that no older
/// history exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileHistoryTruncation {
    MatchLimit,
    ScanLimit,
    Incomplete,
    ShallowBoundary,
}

/// File-history rows plus the coverage metadata needed to describe them
/// honestly. `truncated == None` is the only state that means the walk
/// exhausted all local heads, remote-tracking refs, tags and HEAD.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryResult {
    pub rows: Vec<CommitSummary>,
    pub commits_scanned: usize,
    pub truncated: Option<FileHistoryTruncation>,
    pub remote_refs_included: usize,
    pub follows_renames: bool,
    /// Backend-only retry signal. A root change is not merely generic
    /// incompleteness: the command layer can immediately retry once and avoid
    /// shipping a mixed snapshot.
    #[serde(skip)]
    pub snapshot_changed: bool,
}

/// Commits that changed `file_path` (repo-relative, '/'-separated), newest
/// first. A live, capped revwalk over local heads, remote-tracking refs, tags
/// and HEAD: for each commit
/// we compare the blob id at that path against the FIRST parent's — added,
/// modified, and deleted all count; the root commit counts if the file is
/// present. Bounds BOTH the matches returned and the commits examined, so a
/// rarely-touched file in a huge history still returns quickly. No rename
/// following yet — a path renamed in the past shows only its history under the
/// name asked for (v1). Coverage metadata makes both caps explicit.
#[cfg(test)]
pub fn file_history(repo_path: &Path, file_path: &str) -> Result<FileHistoryResult> {
    file_history_cancellable(repo_path, file_path, &CancellationToken::new())?
        .context("file history unexpectedly cancelled")
}

fn file_history_refs_fingerprint_in(repo: &Repository) -> Result<String> {
    let mut parts = Vec::new();
    for glob in ["refs/heads/*", "refs/remotes/*"] {
        let Ok(refs) = repo.references_glob(glob) else {
            parts.push(format!("{glob}:<unavailable>"));
            continue;
        };
        for item in refs {
            let Ok(reference) = item else {
                // The history walk itself skips unreadable individual refs.
                // Mirror that behavior instead of turning a coalescing hint
                // into a new user-visible failure mode.
                parts.push(format!("{glob}:<unreadable>"));
                continue;
            };
            if let (Some(name), Some(oid)) = (reference.name(), reference.target()) {
                parts.push(format!("{name}:{oid}"));
            }
        }
    }
    if let Ok(tags) = repo.references_glob("refs/tags/*") {
        for item in tags {
            let Ok(reference) = item else {
                parts.push("refs/tags/*:<unreadable>".to_string());
                continue;
            };
            let Some(name) = reference.name() else {
                continue;
            };
            // Include both the tag object and peeled commit. Moving or replacing
            // an annotated tag must invalidate even when one side happens to stay.
            let direct = reference
                .target()
                .map(|oid| oid.to_string())
                .unwrap_or_default();
            let peeled = reference
                .peel(git2::ObjectType::Commit)
                .ok()
                .map(|object| object.id().to_string())
                .unwrap_or_default();
            parts.push(format!("{name}:{direct}:{peeled}"));
        }
    } else {
        parts.push("refs/tags/*:<unavailable>".to_string());
    }
    if let Ok(head) = repo.head() {
        let name = head.name().unwrap_or("HEAD");
        if let Ok(commit) = head.peel_to_commit() {
            parts.push(format!("HEAD@{name}:{}", commit.id()));
        }
    }
    parts.sort_unstable();
    parts.dedup();
    Ok(parts.join("|"))
}

/// Cancellation-aware implementation used by the coalescing command layer.
/// The token is checked at every revwalk and branch-label iteration, so a
/// superseded 50k-commit request does not burn through the remainder of its
/// scan. `Ok(None)` is a normal cancellation outcome.
pub fn file_history_cancellable(
    repo_path: &Path,
    file_path: &str,
    cancellation: &CancellationToken,
) -> Result<Option<FileHistoryResult>> {
    if cancellation.is_cancelled() {
        return Ok(None);
    }
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;
    let roots_before = file_history_refs_fingerprint_in(&repo).ok();

    if cancellation.is_cancelled() {
        return Ok(None);
    }

    let repo_path_str = repo_path.to_string_lossy().into_owned();
    let repo_name = repo_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    let detached = repo.head_detached().unwrap_or(true);
    let head_branch_name: Option<String> = if detached {
        None
    } else {
        repo.head()
            .ok()
            .and_then(|h| h.shorthand().map(|s| s.to_string()))
    };
    let (tagged_oids, mut root_errors) = collect_tagged_oids_with_status(&repo);

    let mut revwalk = repo.revwalk().context("init revwalk")?;
    revwalk
        .set_sorting(Sort::TIME)
        .context("set revwalk sort")?;
    let mut pushed = 0usize;
    let mut remote_refs_included = 0usize;
    match repo.references_glob("refs/heads/*") {
        Ok(refs) => {
            for item in refs {
                if cancellation.is_cancelled() {
                    return Ok(None);
                }
                match item {
                    Ok(reference) => {
                        if let Some(oid) = reference.target() {
                            if revwalk.push(oid).is_ok() {
                                pushed += 1;
                            } else {
                                root_errors = true;
                            }
                        }
                    }
                    Err(_) => root_errors = true,
                }
            }
        }
        Err(_) => root_errors = true,
    }
    // A remote-only branch is still part of the repository history the user
    // can see in gitwink. These are local ref reads — no network access.
    match repo.references_glob("refs/remotes/*") {
        Ok(refs) => {
            for item in refs {
                if cancellation.is_cancelled() {
                    return Ok(None);
                }
                match item {
                    Ok(reference) => {
                        // Symbolic remote/HEAD-style refs have no direct target.
                        if let Some(oid) = reference.target() {
                            if revwalk.push(oid).is_ok() {
                                pushed += 1;
                                remote_refs_included += 1;
                            } else {
                                root_errors = true;
                            }
                        }
                    }
                    Err(_) => root_errors = true,
                }
            }
        }
        Err(_) => root_errors = true,
    }
    // Tag-only history must not disappear merely because its branch was
    // deleted. `tagged_oids` was already collected for row decoration.
    for oid in &tagged_oids {
        if cancellation.is_cancelled() {
            return Ok(None);
        }
        if revwalk.push(*oid).is_ok() {
            pushed += 1;
        } else {
            root_errors = true;
        }
    }
    if let Ok(head) = repo.head() {
        if let Ok(c) = head.peel_to_commit() {
            if revwalk.push(c.id()).is_ok() {
                pushed += 1;
            } else {
                root_errors = true;
            }
        }
    }
    if pushed == 0 {
        let snapshot_changed = roots_before.is_none()
            || file_history_refs_fingerprint_in(&repo).ok().as_ref() != roots_before.as_ref();
        return Ok(Some(FileHistoryResult {
            rows: Vec::new(),
            commits_scanned: 0,
            truncated: (root_errors || snapshot_changed)
                .then_some(FileHistoryTruncation::Incomplete),
            remote_refs_included,
            follows_renames: false,
            snapshot_changed,
        }));
    }

    let path = Path::new(file_path);
    // Blob id + mode at `path`, or None when absent. Mode is part of file
    // history: chmod/type-only commits retain the same OID but are real changes.
    let entry_identity = |commit: &git2::Commit<'_>,
                          cache: &mut HashMap<Oid, Option<(Oid, i32)>>|
     -> Result<Option<(Oid, i32)>> {
        if let Some(cached) = cache.get(&commit.id()).copied() {
            return Ok(cached);
        }
        let tree = commit.tree().context("read commit tree")?;
        let value = match tree.get_path(path) {
            Ok(entry) => Ok(Some((entry.id(), entry.filemode()))),
            Err(error) if error.code() == git2::ErrorCode::NotFound => Ok(None),
            Err(error) => Err(anyhow::Error::from(error)),
        }?;
        cache.insert(commit.id(), value);
        Ok(value)
    };

    let mut raw: Vec<(Oid, git2::Commit<'_>)> = Vec::new();
    let mut examined = 0usize;
    let mut truncated = None;
    let mut had_errors = root_errors;
    let mut entry_cache: HashMap<Oid, Option<(Oid, i32)>> = HashMap::new();
    for oid in revwalk {
        if cancellation.is_cancelled() {
            return Ok(None);
        }
        if examined >= FILE_HISTORY_SCAN_CAP {
            truncated = Some(FileHistoryTruncation::ScanLimit);
            break;
        }
        examined += 1;
        let oid = match oid {
            Ok(o) => o,
            Err(_) => {
                had_errors = true;
                continue;
            }
        };
        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => {
                had_errors = true;
                continue;
            }
        };
        if cancellation.is_cancelled() {
            return Ok(None);
        }
        let cur = match entry_identity(&commit, &mut entry_cache) {
            Ok(value) => value,
            Err(_) => {
                had_errors = true;
                continue;
            }
        };
        // Compare to the FIRST parent only (linear-history / --first-parent
        // semantics): a merge shows up iff it changed the path vs its first
        // parent. A root commit (no parent) counts iff the file exists in it.
        let parent_entry = if commit.parent_count() == 0 {
            None
        } else {
            let parent = match commit.parent(0) {
                Ok(parent) => parent,
                Err(_) => {
                    had_errors = true;
                    continue;
                }
            };
            match entry_identity(&parent, &mut entry_cache) {
                Ok(value) => value,
                Err(_) => {
                    had_errors = true;
                    continue;
                }
            }
        };
        if cur != parent_entry {
            // Prove an extra FILE match, rather than marking MatchLimit merely
            // because some unrelated older commit exists.
            if raw.len() >= FILE_HISTORY_MAX_MATCHES {
                truncated = Some(FileHistoryTruncation::MatchLimit);
                break;
            }
            raw.push((oid, commit));
        }
    }
    if cancellation.is_cancelled() {
        return Ok(None);
    }
    if truncated.is_none() {
        truncated = if had_errors {
            Some(FileHistoryTruncation::Incomplete)
        } else if repo.is_shallow() {
            Some(FileHistoryTruncation::ShallowBoundary)
        } else {
            None
        };
    }
    let targets: HashSet<Oid> = raw.iter().map(|(oid, _)| *oid).collect();
    let label_floor = raw
        .iter()
        .map(|(_, c)| c.time().seconds())
        .min()
        .unwrap_or(0);
    let Some(labels) = compute_branch_labels_cancellable(
        &repo,
        head_branch_name.as_deref(),
        &targets,
        label_floor,
        cancellation,
    ) else {
        return Ok(None);
    };

    let mut out: Vec<CommitSummary> = Vec::with_capacity(raw.len());
    for (oid, commit) in raw {
        if cancellation.is_cancelled() {
            return Ok(None);
        }
        let hash = oid.to_string();
        let short_hash: String = hash.chars().take(7).collect();
        let summary = bounded_text(
            commit.summary().unwrap_or(""),
            MAX_DIRECT_SUMMARY_BYTES,
            "…",
        );
        let author = commit.author();
        let parents: Vec<String> = commit.parent_ids().map(|p| p.to_string()).collect();
        let message = bounded_text(
            commit.message().unwrap_or(""),
            MAX_DIRECT_MESSAGE_BYTES,
            "\n… [message truncated]",
        );
        let ts = commit.time().seconds();
        let branch_label = labels.get(&oid).cloned().flatten();
        out.push(CommitSummary {
            repo_path: repo_path_str.clone(),
            repo_name: repo_name.clone(),
            hash,
            short_hash,
            summary,
            author: author.name().unwrap_or("").to_string(),
            email: author.email().unwrap_or("").to_string(),
            timestamp: ts,
            branch_label,
            is_merge: commit.parent_count() > 1,
            is_tagged: tagged_oids.contains(&oid),
            parents,
            message,
            // Remote-tip badges are about "is this the tip of origin/X" — not
            // meaningful for a file-history list, so we skip the extra work.
            remote_tip_label: None,
            remote_tip_extra_count: 0,
        });
    }
    // The final root proof comes after branch labels and row materialization,
    // not merely after revwalk EOF. This closes the window where refs moved
    // during the expensive decoration phase.
    let snapshot_changed = roots_before.is_none()
        || file_history_refs_fingerprint_in(&repo).ok().as_ref() != roots_before.as_ref();
    if snapshot_changed {
        truncated = Some(FileHistoryTruncation::Incomplete);
    }
    Ok(Some(FileHistoryResult {
        rows: out,
        commits_scanned: examined,
        truncated,
        remote_refs_included,
        follows_renames: false,
        snapshot_changed,
    }))
}

#[derive(Debug)]
pub struct RecentCommitsScan {
    pub rows: Vec<CommitSummary>,
    /// True only when revwalk naturally exhausted without any skipped object
    /// or heuristic cutoff. Destructive cache reconcile is forbidden unless
    /// this is true and the caller also proves roots stayed stable.
    pub exhaustive: bool,
    pub hit_row_cap: bool,
    pub hit_cutoff_heuristic: bool,
    pub had_errors: bool,
    pub shallow: bool,
}

impl RecentCommitsScan {
    pub fn can_reconcile(&self) -> bool {
        self.exhaustive
            && !self.hit_row_cap
            && !self.hit_cutoff_heuristic
            && !self.had_errors
            && !self.shallow
    }
}

/// Bounded recent walk with an explicit completeness contract. Timestamp
/// cutoff and row caps are presentation budgets, never deletion proof.
pub fn recent_commits_scan(
    repo_path: &Path,
    max_count: usize,
    since_unix_seconds: i64,
) -> Result<RecentCommitsScan> {
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;

    let repo_path_str = repo_path.to_string_lossy().into_owned();
    let repo_name = repo_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Identify what HEAD is on right now so we can suppress branch labels for
    // the user's current branch (the spec rule: "체크아웃된 브랜치는 생략").
    let detached = repo.head_detached().unwrap_or(true);
    let head_branch_name: Option<String> = if detached {
        None
    } else {
        repo.head()
            .ok()
            .and_then(|h| h.shorthand().map(|s| s.to_string()))
    };

    let (tagged_oids, tag_errors) = collect_tagged_oids_with_status(&repo);

    // Remote-tracking refs are local-file reads — see comment in
    // `repo_commits` for the rationale. We always include them here because
    // `recent_commits` is the all-branches view by definition.
    let (remote_tips, remote_errors) = collect_remote_tips_with_status(&repo);
    let remote_badges = build_remote_badge_map(&remote_tips);
    let mut had_errors = tag_errors || remote_errors;

    let mut revwalk = repo.revwalk().context("init revwalk")?;
    // libgit2 contract: set_sorting resets the walker, so it MUST run before
    // any push/hide. Calling it after pushes (as we did in 847ed51) silently
    // drops the pushed starting points.
    revwalk
        .set_sorting(Sort::TIME)
        .context("set revwalk sort")?;

    // Walk every local ref head, not just HEAD. The revwalk dedupes commits
    // that appear on multiple branches, so an agent that committed to a
    // feature branch (or to a branch the user isn't currently on) still
    // shows up in the timeline.
    let mut pushed = 0usize;
    match repo.references_glob("refs/heads/*") {
        Ok(refs) => {
            for item in refs {
                let Ok(r) = item else {
                    had_errors = true;
                    continue;
                };
                if let Some(oid) = r.target() {
                    if revwalk.push(oid).is_ok() {
                        pushed += 1;
                    } else {
                        had_errors = true;
                    }
                }
            }
        }
        Err(_) => had_errors = true,
    }
    // Add remote tip OIDs as extra roots — surfaces commits pushed to
    // a remote-tracking branch that have no local head.
    for tip in &remote_tips {
        if revwalk.push(tip.oid).is_ok() {
            pushed += 1;
        } else {
            had_errors = true;
        }
    }
    // Always include HEAD itself: commits made on a detached HEAD (agents
    // checking out a SHA / tag, bisect-style loops) are reachable from no
    // local branch and no remote tip, so without this root they would be
    // invisible to the timeline. The revwalk dedupes shared history, so
    // when HEAD sits on a branch the extra root costs nothing. This also
    // covers the old "no heads, no remote tips" fallback — an unborn HEAD
    // simply contributes no root and the walk yields nothing.
    if let Ok(head) = repo.head() {
        if let Ok(head_commit) = head.peel_to_commit() {
            if revwalk.push(head_commit.id()).is_ok() {
                pushed += 1;
            }
        }
    }
    // Tags are roots too: a commit reachable ONLY from a tag (its branch
    // was deleted after tagging — release tags routinely outlive branches)
    // is still part of the repo's truth. It must stay visible, and more
    // importantly it must be in the live set so the reconcile pass never
    // deletes its cached row as a "ghost". Old tags cost nothing extra:
    // sub-cutoff commits are excluded by the slop loop below.
    for oid in &tagged_oids {
        if revwalk.push(*oid).is_ok() {
            pushed += 1;
        } else {
            had_errors = true;
        }
    }
    if pushed == 0 {
        return Ok(RecentCommitsScan {
            rows: Vec::new(),
            exhaustive: !had_errors,
            hit_row_cap: false,
            hit_cutoff_heuristic: false,
            had_errors,
            shallow: repo.is_shallow(),
        });
    }

    // Pass 1: collect the output rows first. Bounded by max_count and the
    // since cutoff — with a CUTOFF_SLOP tolerance instead of a hard break,
    // because committer dates aren't monotone along ancestry and a hard
    // break on the first old commit would hide live in-window history
    // behind a skew-dated child (see CUTOFF_SLOP).
    let mut raw: Vec<(Oid, git2::Commit<'_>)> = Vec::with_capacity(max_count);
    let mut slop = 0usize;
    let mut hit_row_cap = false;
    let mut hit_cutoff_heuristic = false;
    let mut exhaustive = true;
    for oid in revwalk {
        if raw.len() >= max_count {
            hit_row_cap = true;
            exhaustive = false;
            break;
        }
        let oid = match oid {
            Ok(o) => o,
            Err(_) => {
                had_errors = true;
                continue;
            }
        };
        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => {
                had_errors = true;
                continue;
            }
        };
        if commit.time().seconds() < since_unix_seconds {
            slop += 1;
            if slop >= CUTOFF_SLOP {
                hit_cutoff_heuristic = true;
                exhaustive = false;
                break;
            }
            continue;
        }
        slop = 0;
        raw.push((oid, commit));
    }

    // Pass 2: branch labels for just the OIDs we'll return. Used to be a
    // O(branches × history) prefetch — now bounded per-branch and exits
    // early once every target is resolved.
    let targets: HashSet<Oid> = raw.iter().map(|(oid, _)| *oid).collect();
    let label_floor = raw
        .iter()
        .map(|(_, c)| c.time().seconds())
        .min()
        .unwrap_or(0);
    let labels = compute_branch_labels(&repo, head_branch_name.as_deref(), &targets, label_floor);

    let mut out: Vec<CommitSummary> = Vec::with_capacity(raw.len());
    for (oid, commit) in raw {
        let hash = oid.to_string();
        let short_hash: String = hash.chars().take(7).collect();
        let summary = bounded_text(
            commit.summary().unwrap_or(""),
            MAX_DIRECT_SUMMARY_BYTES,
            "…",
        );
        let author = commit.author();
        let parents: Vec<String> = commit.parent_ids().map(|p| p.to_string()).collect();
        let message = bounded_text(
            commit.message().unwrap_or(""),
            MAX_DIRECT_MESSAGE_BYTES,
            "\n… [message truncated]",
        );
        let ts = commit.time().seconds();
        let branch_label = labels.get(&oid).cloned().flatten();
        let (remote_tip_label, remote_tip_extra_count) = match remote_badges.get(&oid) {
            Some(b) => (Some(b.primary.clone()), b.extra_count),
            None => (None, 0),
        };

        out.push(CommitSummary {
            repo_path: repo_path_str.clone(),
            repo_name: repo_name.clone(),
            hash,
            short_hash,
            summary,
            author: author.name().unwrap_or("").to_string(),
            email: author.email().unwrap_or("").to_string(),
            timestamp: ts,
            branch_label,
            is_merge: commit.parent_count() > 1,
            is_tagged: tagged_oids.contains(&oid),
            parents,
            message,
            remote_tip_label,
            remote_tip_extra_count,
        });
    }

    Ok(RecentCommitsScan {
        rows: out,
        exhaustive: exhaustive && !had_errors,
        hit_row_cap,
        hit_cutoff_heuristic,
        had_errors,
        shallow: repo.is_shallow(),
    })
}

/// Compatibility wrapper for non-destructive callers. Mutation paths use
/// `recent_commits_scan` and inspect its completeness before reconciling.
pub fn recent_commits(
    repo_path: &Path,
    max_count: usize,
    since_unix_seconds: i64,
) -> Result<Vec<CommitSummary>> {
    Ok(recent_commits_scan(repo_path, max_count, since_unix_seconds)?.rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Signature, Time};
    use tempfile::TempDir;

    fn empty_tree(repo: &Repository) -> git2::Tree<'_> {
        let mut index = repo.index().unwrap();
        let tree_id = index.write_tree().unwrap();
        repo.find_tree(tree_id).unwrap()
    }

    fn commit_with_time(
        repo: &Repository,
        msg: &str,
        ts: i64,
        parents: &[&git2::Commit<'_>],
    ) -> git2::Oid {
        let sig = Signature::new("t", "t@e", &Time::new(ts, 0)).unwrap();
        let tree = empty_tree(repo);
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, parents)
            .unwrap()
    }

    fn write_loose_ref(repo: &Repository, ref_name: &str, oid: &str) {
        let path = repo.path().join(ref_name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("{oid}\n")).unwrap();
    }

    // A tree builder seeded from the first parent's tree (so unrelated files
    // persist across commits), for the file-history tests.
    fn builder_from_parent<'a>(
        repo: &'a Repository,
        parents: &[&git2::Commit<'_>],
    ) -> git2::TreeBuilder<'a> {
        match parents.first() {
            Some(p) => repo.treebuilder(Some(&p.tree().unwrap())).unwrap(),
            None => repo.treebuilder(None).unwrap(),
        }
    }

    fn commit_file(
        repo: &Repository,
        file: &str,
        content: &str,
        msg: &str,
        ts: i64,
        parents: &[&git2::Commit<'_>],
    ) -> git2::Oid {
        let sig = Signature::new("t", "t@e", &Time::new(ts, 0)).unwrap();
        let blob = repo.blob(content.as_bytes()).unwrap();
        let mut b = builder_from_parent(repo, parents);
        b.insert(file, blob, 0o100644).unwrap();
        let tree = repo.find_tree(b.write().unwrap()).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, parents)
            .unwrap()
    }

    fn commit_remove(
        repo: &Repository,
        file: &str,
        msg: &str,
        ts: i64,
        parents: &[&git2::Commit<'_>],
    ) -> git2::Oid {
        let sig = Signature::new("t", "t@e", &Time::new(ts, 0)).unwrap();
        let mut b = builder_from_parent(repo, parents);
        b.remove(file).unwrap();
        let tree = repo.find_tree(b.write().unwrap()).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, parents)
            .unwrap()
    }

    fn commit_file_mode(
        repo: &Repository,
        file: &str,
        mode: i32,
        msg: &str,
        ts: i64,
        parent: &git2::Commit<'_>,
    ) -> git2::Oid {
        let sig = Signature::new("t", "t@e", &Time::new(ts, 0)).unwrap();
        let parent_tree = parent.tree().unwrap();
        let blob = parent_tree.get_path(Path::new(file)).unwrap().id();
        let mut builder = repo.treebuilder(Some(&parent_tree)).unwrap();
        builder.insert(file, blob, mode).unwrap();
        let tree = repo.find_tree(builder.write().unwrap()).unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &[parent])
            .unwrap()
    }

    #[test]
    fn file_history_tracks_add_modify_delete_only() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let c1 = commit_file(&repo, "a.txt", "v1", "add a", 1_000, &[]);
        let c1c = repo.find_commit(c1).unwrap();
        let c2 = commit_file(&repo, "a.txt", "v2", "edit a", 2_000, &[&c1c]);
        let c2c = repo.find_commit(c2).unwrap();
        // Touches b.txt only — must NOT appear in a.txt's history.
        let c3 = commit_file(&repo, "b.txt", "x", "add b", 3_000, &[&c2c]);
        let c3c = repo.find_commit(c3).unwrap();
        let _c4 = commit_remove(&repo, "a.txt", "rm a", 4_000, &[&c3c]);

        let hist = file_history(dir.path(), "a.txt").unwrap();
        assert_eq!(hist.truncated, None);
        assert_eq!(
            hist.rows
                .iter()
                .map(|c| c.summary.as_str())
                .collect::<Vec<_>>(),
            vec!["rm a", "edit a", "add a"],
        );
        // b.txt's history is just its own add.
        let bhist = file_history(dir.path(), "b.txt").unwrap();
        assert_eq!(
            bhist
                .rows
                .iter()
                .map(|c| c.summary.as_str())
                .collect::<Vec<_>>(),
            vec!["add b"],
        );
    }

    #[test]
    fn file_history_unknown_path_is_empty() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let _ = commit_file(&repo, "a.txt", "v1", "add a", 1_000, &[]);
        assert!(file_history(dir.path(), "nope.txt")
            .unwrap()
            .rows
            .is_empty());
    }

    #[test]
    fn file_history_finds_changes_on_other_branches() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let base = commit_file(&repo, "a.txt", "v1", "base", 1_000, &[]);
        let basec = repo.find_commit(base).unwrap();
        // A side branch edits a.txt; HEAD stays on `base`.
        let side = commit_file(&repo, "a.txt", "v2", "side edit", 2_000, &[&basec]);
        repo.branch("feature", &repo.find_commit(side).unwrap(), false)
            .unwrap();
        // Walk covers all local heads, so the side edit is included.
        let hist = file_history(dir.path(), "a.txt").unwrap();
        let msgs: Vec<&str> = hist.rows.iter().map(|c| c.summary.as_str()).collect();
        assert!(msgs.contains(&"side edit"), "got {msgs:?}");
        assert!(msgs.contains(&"base"), "got {msgs:?}");
    }

    #[test]
    fn all_branches_and_fingerprint_include_non_origin_remote_tip() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let c1 = commit_file(&repo, "a.txt", "v1", "local", 1_000, &[]);
        let c1c = repo.find_commit(c1).unwrap();
        let sig = Signature::new("t", "t@e", &Time::new(2_000, 0)).unwrap();
        let blob = repo.blob(b"remote").unwrap();
        let mut builder = repo.treebuilder(Some(&c1c.tree().unwrap())).unwrap();
        builder.insert("a.txt", blob, 0o100644).unwrap();
        let tree = repo.find_tree(builder.write().unwrap()).unwrap();
        let remote_only = repo
            .commit(None, &sig, &sig, "upstream only", &tree, &[&c1c])
            .unwrap();
        repo.reference(
            "refs/remotes/upstream/main",
            remote_only,
            true,
            "test upstream",
        )
        .unwrap();

        let rows = repo_commits(dir.path(), None, 20, 0).unwrap().rows;
        assert_eq!(
            rows.first().map(|row| row.hash.clone()),
            Some(remote_only.to_string())
        );
        assert_eq!(
            rows.first().and_then(|row| row.remote_tip_label.as_deref()),
            Some("upstream/main")
        );

        let before = repo_refs_fingerprint(dir.path(), None).unwrap();
        repo.find_reference("refs/remotes/upstream/main")
            .unwrap()
            .set_target(c1, "rewind upstream")
            .unwrap();
        let after = repo_refs_fingerprint(dir.path(), None).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn repo_commits_time_boundary_is_normal_complete_window_coverage() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let mut parent = None;
        for index in 0..(CUTOFF_SLOP + 2) {
            let parents = parent
                .as_ref()
                .map(|commit| vec![commit])
                .unwrap_or_default();
            let oid =
                commit_with_time(&repo, &format!("old-{index}"), 100 + index as i64, &parents);
            parent = Some(repo.find_commit(oid).unwrap());
        }
        let recent = commit_with_time(&repo, "inside window", 20_000, &[parent.as_ref().unwrap()]);

        let scan = repo_commits(dir.path(), None, 20, 10_000).unwrap();
        assert_eq!(scan.rows.len(), 1);
        assert_eq!(scan.rows[0].hash, recent.to_string());
        assert!(
            !scan.incomplete,
            "reaching the requested time boundary is not a Git read failure"
        );
    }

    #[test]
    fn valid_non_commit_tag_does_not_claim_history_is_damaged() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let commit = commit_with_time(&repo, "commit", 1_000, &[]);
        let blob = repo.blob(b"release artifact").unwrap();
        repo.reference("refs/tags/artifact", blob, true, "tag blob")
            .unwrap();

        let direct = repo_commits(dir.path(), None, 20, 0).unwrap();
        assert_eq!(direct.rows[0].hash, commit.to_string());
        assert!(!direct.incomplete);
        let recent = recent_commits_scan(dir.path(), 20, 0).unwrap();
        assert!(!recent.had_errors);
    }

    #[test]
    fn explicit_branch_ignores_unrelated_broken_tag_but_all_refs_reports_it() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let commit = commit_with_time(&repo, "commit", 1_000, &[]);
        let selected = vec![repo.head().unwrap().name().unwrap().to_string()];
        write_loose_ref(
            &repo,
            "refs/tags/broken",
            "1111111111111111111111111111111111111111",
        );

        let explicit = repo_commits(dir.path(), Some(&selected), 20, 0).unwrap();
        assert_eq!(explicit.rows[0].hash, commit.to_string());
        assert!(
            !explicit.incomplete,
            "an unrelated tag is decoration outside the selected branch scope"
        );
        assert!(repo_commits(dir.path(), None, 20, 0).unwrap().incomplete);
    }

    #[test]
    fn broken_remote_ref_returns_partial_rows_and_blocks_reconcile() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let commit = commit_with_time(&repo, "commit", 1_000, &[]);
        write_loose_ref(
            &repo,
            "refs/remotes/origin/broken",
            "2222222222222222222222222222222222222222",
        );

        let direct = repo_commits(dir.path(), None, 20, 0).unwrap();
        assert!(direct.rows.iter().any(|row| row.hash == commit.to_string()));
        assert!(direct.incomplete);

        let recent = recent_commits_scan(dir.path(), 20, 0).unwrap();
        assert!(recent.rows.iter().any(|row| row.hash == commit.to_string()));
        assert!(recent.had_errors);
        assert!(!recent.can_reconcile());
    }

    #[test]
    fn file_history_includes_mode_only_changes() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let add = commit_file(&repo, "script.sh", "echo ok", "add", 1_000, &[]);
        let add_commit = repo.find_commit(add).unwrap();
        commit_file_mode(
            &repo,
            "script.sh",
            0o100755,
            "make executable",
            2_000,
            &add_commit,
        );

        let history = file_history(dir.path(), "script.sh").unwrap();
        assert_eq!(
            history
                .rows
                .iter()
                .map(|commit| commit.summary.as_str())
                .collect::<Vec<_>>(),
            vec!["make executable", "add"]
        );
    }

    #[test]
    fn file_history_precancel_is_a_normal_empty_outcome() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let missing = Path::new("definitely-does-not-exist");
        assert!(file_history_cancellable(missing, "a.txt", &cancellation)
            .unwrap()
            .is_none());
    }

    #[test]
    fn file_history_includes_remote_only_refs() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let base = commit_file(&repo, "a.txt", "v1", "base", 1_000, &[]);
        let basec = repo.find_commit(base).unwrap();

        // Create a commit object without moving HEAD, then expose it only via
        // a remote-tracking ref. No local branch reaches this change.
        let sig = Signature::new("t", "t@e", &Time::new(2_000, 0)).unwrap();
        let blob = repo.blob(b"v2").unwrap();
        let mut builder = builder_from_parent(&repo, &[&basec]);
        builder.insert("a.txt", blob, 0o100644).unwrap();
        let tree = repo.find_tree(builder.write().unwrap()).unwrap();
        let remote_only = repo
            .commit(None, &sig, &sig, "remote only edit", &tree, &[&basec])
            .unwrap();
        repo.reference(
            "refs/remotes/upstream/feature",
            remote_only,
            true,
            "test remote-only history",
        )
        .unwrap();

        let hist = file_history(dir.path(), "a.txt").unwrap();
        let msgs: Vec<&str> = hist.rows.iter().map(|c| c.summary.as_str()).collect();
        assert!(msgs.contains(&"remote only edit"), "got {msgs:?}");
        assert!(hist.remote_refs_included >= 1);
        assert_eq!(hist.truncated, None);
    }

    #[test]
    fn empty_repo_returns_empty() {
        let dir = TempDir::new().unwrap();
        let _ = Repository::init(dir.path()).unwrap();
        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        assert!(commits.is_empty());
    }

    #[test]
    fn returns_commits_newest_first() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let c1 = commit_with_time(&repo, "first", 1_000, &[]);
        let c1c = repo.find_commit(c1).unwrap();
        let c2 = commit_with_time(&repo, "second", 2_000, &[&c1c]);
        let c2c = repo.find_commit(c2).unwrap();
        let _ = commit_with_time(&repo, "third", 3_000, &[&c2c]);

        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        assert_eq!(
            commits
                .iter()
                .map(|c| c.summary.as_str())
                .collect::<Vec<_>>(),
            vec!["third", "second", "first"]
        );
        assert_eq!(commits[0].timestamp, 3_000);
    }

    #[test]
    fn caps_max_count() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let mut parent_oid = commit_with_time(&repo, "c0", 1_000, &[]);
        for i in 1..5 {
            let parent = repo.find_commit(parent_oid).unwrap();
            parent_oid = commit_with_time(&repo, &format!("c{i}"), 1_000 + i, &[&parent]);
        }

        let commits = recent_commits(dir.path(), 3, 0).unwrap();
        assert_eq!(commits.len(), 3);
    }

    #[test]
    fn cutoff_excludes_old_commits() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let old = commit_with_time(&repo, "old", 100, &[]);
        let oldc = repo.find_commit(old).unwrap();
        let _new = commit_with_time(&repo, "new", 2_000, &[&oldc]);

        let commits = recent_commits(dir.path(), 10, 1_000).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].summary, "new");
    }

    #[test]
    fn current_branch_commits_have_no_label() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let _ = commit_with_time(&repo, "on current", 1_000, &[]);
        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].branch_label, None);
    }

    #[test]
    fn other_branch_commits_get_label() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let main = commit_with_time(&repo, "shared", 1_000, &[]);
        let mainc = repo.find_commit(main).unwrap();

        repo.branch("dev", &mainc, false).unwrap();
        let sig = Signature::new("t", "t@e", &Time::new(2_000, 0)).unwrap();
        let tree = empty_tree(&repo);
        repo.commit(
            Some("refs/heads/dev"),
            &sig,
            &sig,
            "dev-only",
            &tree,
            &[&mainc],
        )
        .unwrap();

        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        let dev_commit = commits.iter().find(|c| c.summary == "dev-only").unwrap();
        let shared_commit = commits.iter().find(|c| c.summary == "shared").unwrap();
        assert_eq!(dev_commit.branch_label.as_deref(), Some("dev"));
        assert_eq!(shared_commit.branch_label, None);
    }

    #[test]
    fn detached_head_suppresses_all_labels() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let c1 = commit_with_time(&repo, "first", 1_000, &[]);
        let c1c = repo.find_commit(c1).unwrap();
        repo.branch("dev", &c1c, false).unwrap();
        let sig = Signature::new("t", "t@e", &Time::new(2_000, 0)).unwrap();
        let tree = empty_tree(&repo);
        repo.commit(
            Some("refs/heads/dev"),
            &sig,
            &sig,
            "dev-only",
            &tree,
            &[&c1c],
        )
        .unwrap();

        // Detach HEAD onto the first commit.
        repo.set_head_detached(c1).unwrap();

        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        assert!(
            commits.iter().all(|c| c.branch_label.is_none()),
            "detached HEAD must suppress all labels: {commits:?}"
        );
    }

    #[test]
    fn walks_all_local_branches() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        // Commit on the default branch (becomes "refs/heads/master" or
        // "refs/heads/main" depending on git config).
        let main1 = commit_with_time(&repo, "main-only", 1_000, &[]);
        let main1c = repo.find_commit(main1).unwrap();

        // Branch "dev" off main and put a unique commit on it.
        repo.branch("dev", &main1c, false).unwrap();
        let sig = Signature::new("t", "t@e", &Time::new(2_000, 0)).unwrap();
        let tree = empty_tree(&repo);
        repo.commit(
            Some("refs/heads/dev"),
            &sig,
            &sig,
            "dev-only",
            &tree,
            &[&main1c],
        )
        .unwrap();

        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        let summaries: Vec<&str> = commits.iter().map(|c| c.summary.as_str()).collect();
        assert!(summaries.contains(&"main-only"), "missing main-only");
        assert!(summaries.contains(&"dev-only"), "missing dev-only");
    }

    #[test]
    fn dedupes_commits_visible_from_multiple_branches() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        // Single commit reachable from main and dev.
        let shared = commit_with_time(&repo, "shared", 1_000, &[]);
        let sharedc = repo.find_commit(shared).unwrap();
        repo.branch("dev", &sharedc, false).unwrap();

        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        assert_eq!(commits.len(), 1, "shared commit should appear once");
    }

    #[test]
    fn populates_short_hash_and_repo_name() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let _ = commit_with_time(&repo, "only", 1_000, &[]);

        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].short_hash.len(), 7);
        assert!(commits[0].hash.starts_with(&commits[0].short_hash));
        assert_eq!(
            commits[0].repo_name,
            dir.path().file_name().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn file_diff_treats_pathspec_literally_not_fnmatch() {
        // "[id].tsx" as an fnmatch pattern is a character class matching
        // "i.tsx"/"d.tsx" — and NOT its own literal name. Without the
        // literal-pathspec pin, a commit touching both files rendered the
        // sibling's hunks under the bracketed file (and cached them).
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        let mk_tree = |bracket: &str, sibling: &str| {
            let ba = repo.blob(bracket.as_bytes()).unwrap();
            let bb = repo.blob(sibling.as_bytes()).unwrap();
            let mut b = repo.treebuilder(None).unwrap();
            b.insert("[id].tsx", ba, 0o100644).unwrap();
            b.insert("i.tsx", bb, 0o100644).unwrap();
            repo.find_tree(b.write().unwrap()).unwrap()
        };

        let sig1 = Signature::new("t", "t@e", &Time::new(1_000, 0)).unwrap();
        let t1 = mk_tree("bracket v1\n", "sibling v1\n");
        let c1 = repo
            .commit(Some("HEAD"), &sig1, &sig1, "seed", &t1, &[])
            .unwrap();
        let c1c = repo.find_commit(c1).unwrap();

        let sig2 = Signature::new("t", "t@e", &Time::new(2_000, 0)).unwrap();
        let t2 = mk_tree("bracket v2\n", "sibling v2\n");
        let c2 = repo
            .commit(Some("HEAD"), &sig2, &sig2, "edit both", &t2, &[&c1c])
            .unwrap();

        let patch = file_diff(dir.path(), &c2.to_string(), "[id].tsx", 3).unwrap();
        assert!(
            patch.contains("bracket v2"),
            "own hunks must be present:\n{patch}"
        );
        assert!(
            !patch.contains("sibling"),
            "sibling file's hunks leaked into the diff:\n{patch}"
        );
    }
}
/// Stable-enough local storage identity for the repository's common Git dir.
/// It is used only to reject automatic restore of a different repository that
/// was initialized later at the same worktree path; it never writes the repo.
pub fn repo_storage_identity(repo_path: &Path) -> Result<String> {
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;
    let gitdir = repo.path();
    let common = std::fs::read_to_string(gitdir.join("commondir"))
        .ok()
        .map(|text| {
            let raw = text.trim_end_matches(['\n', '\r']);
            let path = Path::new(raw);
            if path.is_absolute() {
                path.to_path_buf()
            } else {
                gitdir.join(path)
            }
        })
        .unwrap_or_else(|| gitdir.to_path_buf());
    let canonical = std::fs::canonicalize(&common).unwrap_or(common);
    let metadata = std::fs::metadata(&canonical)
        .with_context(|| format!("stat common git dir {}", canonical.display()))?;

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        return Ok(format!(
            "windows:{}:{}:{}",
            canonical.to_string_lossy().to_lowercase(),
            metadata.creation_time(),
            metadata.file_attributes()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        return Ok(format!(
            "unix:{}:{}:{}",
            canonical.display(),
            metadata.dev(),
            metadata.ino()
        ));
    }
    #[allow(unreachable_code)]
    Ok(format!("path:{}", canonical.display()))
}

pub fn repo_is_shallow(repo_path: &Path) -> Result<bool> {
    Ok(Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?
        .is_shallow())
}
