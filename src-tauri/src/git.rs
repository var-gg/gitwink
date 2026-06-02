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
    /// this exact commit. Local file read — we never call `git fetch`.
    /// None when no remote ref points here. Kept separate from
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
    pub commit_count: usize,
    /// Unix seconds of the tip commit, for "recent activity" sort.
    pub last_activity: i64,
}

/// Snapshot of the current branch's relation to its upstream remote-tracking
/// ref. Computed from local files only — gitwink never calls `git fetch`, so
/// these counts reflect the user's last fetch, not the live remote.
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
        // blob wasn't loaded; fetch it explicitly.
        let new_size = if delta.new_file().exists() {
            repo.find_blob(delta.new_file().id())
                .map(|b| b.size() as u64)
                .ok()
        } else {
            None
        };
        let old_size = if delta.old_file().exists() {
            repo.find_blob(delta.old_file().id())
                .map(|b| b.size() as u64)
                .ok()
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

    let mut out = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        match line.origin() {
            'F' | 'H' => {
                // file or hunk header — content already includes its prefix
                out.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
            }
            origin => {
                out.push(origin);
                out.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
            }
        }
        true
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

fn ref_count_and_activity(repo: &Repository, tip: Oid, cap: usize) -> (usize, i64) {
    let mut count = 0usize;
    let mut last_activity: i64 = 0;
    if let Ok(mut rw) = repo.revwalk() {
        // libgit2: set_sorting MUST be called before any push/hide.
        let _ = rw.set_sorting(Sort::TIME);
        if rw.push(tip).is_ok() {
            for (i, oid_res) in rw.enumerate() {
                let Ok(oid) = oid_res else { continue };
                count = i + 1;
                if i == 0 {
                    if let Ok(c) = repo.find_commit(oid) {
                        last_activity = c.time().seconds();
                    }
                }
                if i >= cap {
                    count = cap + 1;
                    break;
                }
            }
        }
    }
    (count, last_activity)
}

/// Return every local branch AND every remote-tracking ref in the repo.
/// Remote refs come from `refs/remotes/*`, skipping any `*/HEAD` symbolic
/// alias. Both share the same `BranchInfo` shape, with `kind` set so the
/// frontend can render them in separate sections.
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

    let mut out: Vec<BranchInfo> = Vec::new();

    // Local heads first.
    if let Ok(refs) = repo.references_glob("refs/heads/*") {
        for r in refs.flatten() {
            let Some(name) = r.shorthand().map(|s| s.to_string()) else {
                continue;
            };
            let Some(tip) = r.target() else { continue };
            let (count, last_activity) =
                ref_count_and_activity(&repo, tip, LOCAL_REF_COUNT_CAP);
            out.push(BranchInfo {
                name: name.clone(),
                ref_name: format!("refs/heads/{name}"),
                kind: "local".to_string(),
                tip_hash: tip.to_string(),
                is_head: Some(&name) == head_branch.as_ref(),
                commit_count: count,
                last_activity,
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
            let (count, last_activity) =
                ref_count_and_activity(&repo, tip, REMOTE_REF_COUNT_CAP);
            out.push(BranchInfo {
                name: name.clone(),
                ref_name: format!("refs/remotes/{name}"),
                kind: "remote".to_string(),
                tip_hash: tip.to_string(),
                is_head: false,
                commit_count: count,
                last_activity,
            });
        }
    }

    // Sort by recency overall; UI groups by `kind` so this just controls
    // intra-group ordering.
    out.sort_by(|a, b| b.last_activity.cmp(&a.last_activity));
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
/// PURE READ. We only inspect refs the user/IDE already wrote with
/// `git fetch`/`pull`; gitwink never initiates network activity.
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

/// Max number of `refs/remotes/origin/*` tips we'll push into the timeline
/// revwalk per repo. Tips beyond this (sorted by tip-commit recency) are
/// dropped, which avoids unbounded revwalk roots on repos with hundreds of
/// stale remote branches. 64 covers every active project we've seen.
const REMOTE_TIP_ROOT_CAP: usize = 64;

/// A `refs/remotes/origin/<short>` ref pinned to a single commit. We don't
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

/// Read `refs/remotes/origin/*` from disk (NO network — these refs are
/// whatever the user/IDE/CI last wrote during `git fetch`). Excludes the
/// symbolic `origin/HEAD`. Filters by `since_unix_seconds` so abandoned
/// remote branches don't drag the timeline back. Returns at most `cap`
/// tips, sorted by tip-commit recency.
fn collect_origin_remote_tips(
    repo: &Repository,
    since_unix_seconds: i64,
    cap: usize,
) -> Vec<RemoteTip> {
    let mut out: Vec<RemoteTip> = Vec::new();
    let Ok(iter) = repo.references_glob("refs/remotes/origin/*") else {
        return out;
    };
    for r in iter.flatten() {
        let Some(label) = r.shorthand().map(str::to_string) else {
            continue;
        };
        // Symbolic ref like `origin/HEAD` resolves to another origin/<name>
        // — keep the named target only, drop the symbolic alias.
        if label == "origin/HEAD" || label.ends_with("/HEAD") {
            continue;
        }
        let Some(oid) = r.target() else { continue };
        let Ok(commit) = repo.find_commit(oid) else {
            continue;
        };
        let ts = commit.time().seconds();
        if since_unix_seconds > 0 && ts < since_unix_seconds {
            continue;
        }
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
    out.truncate(cap);
    out
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
/// `scan_cap` so a branch with deep history can't pin first-paint.
fn limited_reachable_membership(
    repo: &Repository,
    tip: Oid,
    targets: &HashSet<Oid>,
    scan_cap: usize,
) -> HashSet<Oid> {
    let mut found: HashSet<Oid> = HashSet::new();
    if targets.is_empty() {
        return found;
    }
    let Ok(mut rw) = repo.revwalk() else {
        return found;
    };
    // libgit2: set_sorting resets the walker, so it MUST run before push.
    // Doing it after push silently empties the walker.
    if rw.set_sorting(Sort::TIME).is_err() {
        return found;
    }
    if rw.push(tip).is_err() {
        return found;
    }

    for (i, oid_res) in rw.enumerate() {
        if i >= scan_cap || found.len() == targets.len() {
            break;
        }
        let Ok(oid) = oid_res else {
            continue;
        };
        if targets.contains(&oid) {
            found.insert(oid);
        }
    }
    found
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
) -> HashMap<Oid, Option<String>> {
    let mut labels: HashMap<Oid, Option<String>> = HashMap::new();
    if targets.is_empty() {
        return labels;
    }

    // Detached HEAD or no HEAD → suppress all labels (existing behaviour).
    let Some(head_name) = head_branch_name else {
        for &oid in targets {
            labels.insert(oid, None);
        }
        return labels;
    };

    if let Some(tip) = repo
        .head()
        .ok()
        .and_then(|h| h.peel_to_commit().ok())
        .map(|c| c.id())
    {
        let hits = limited_reachable_membership(repo, tip, targets, BRANCH_LABEL_SCAN_CAP);
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
        return labels;
    }

    if let Ok(refs) = repo.references_glob("refs/heads/*") {
        for r in refs.flatten() {
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
            let hits = limited_reachable_membership(repo, tip, &remaining, BRANCH_LABEL_SCAN_CAP);
            for oid in hits {
                labels.insert(oid, Some(name.clone()));
                remaining.remove(&oid);
            }
        }
    }

    labels
}

fn collect_tagged_oids(repo: &Repository) -> HashSet<Oid> {
    let mut set = HashSet::new();
    if let Ok(refs) = repo.references_glob("refs/tags/*") {
        for r in refs.flatten() {
            if let Ok(obj) = r.peel(git2::ObjectType::Commit) {
                set.insert(obj.id());
            }
        }
    }
    set
}

/// Walk the given branches (None = all local heads, like recent_commits) in
/// a single repository, deduping by SHA, returning newest-first commits with
/// parent SHAs populated.
pub fn repo_commits(
    repo_path: &Path,
    branches: Option<&[String]>,
    max_count: usize,
    since_unix_seconds: i64,
) -> Result<Vec<CommitSummary>> {
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;

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

    let tagged_oids = collect_tagged_oids(&repo);

    let mut revwalk = repo.revwalk().context("init revwalk")?;
    // libgit2 contract: set_sorting resets the walker, so it MUST run before
    // any push/hide. Calling it after pushes (as we did in 847ed51) silently
    // drops the pushed starting points.
    revwalk
        .set_sorting(Sort::TIME)
        .context("set revwalk sort")?;
    let mut pushed = 0usize;

    // Remote tips contribute both as revwalk roots (so remote-only commits
    // appear) AND as the source of per-row "origin/X" badges. In explicit
    // mode (user picked specific refs), only the remote refs they picked
    // count. In all-branches mode, auto-discover the recent origin/* tips
    // with the same window filter — this is the GitLens-killer feature.
    let mut remote_tips: Vec<RemoteTip> = Vec::new();

    // explicit_mode = the caller passed a list at all (Some, including the
    // empty slice). An empty explicit list is "the user deselected every
    // branch" and must yield zero rows, NOT fall back to all-mode.
    let explicit_mode = branches.is_some();
    let explicit_names: &[String] = branches.unwrap_or(&[]);

    if explicit_mode && explicit_names.is_empty() {
        return Ok(Vec::new());
    }

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
                        }
                    }
                }
            }
        }
    } else {
        // All-branches mode: every local head + recent origin/* tips.
        if let Ok(refs) = repo.references_glob("refs/heads/*") {
            for r in refs.flatten() {
                if let Some(oid) = r.target() {
                    if revwalk.push(oid).is_ok() {
                        pushed += 1;
                    }
                }
            }
        }
        remote_tips = collect_origin_remote_tips(
            &repo,
            since_unix_seconds,
            REMOTE_TIP_ROOT_CAP,
        );
        for tip in &remote_tips {
            let _ = revwalk.push(tip.oid);
        }
    }

    let remote_badges = build_remote_badge_map(&remote_tips);

    if pushed == 0 && remote_tips.is_empty() {
        // Explicit mode with all refs stale/invalid: return empty rather
        // than silently rerouting the user to HEAD history — they asked
        // for specific refs, so "nothing matches" is the honest answer.
        if explicit_mode {
            return Ok(Vec::new());
        }
        let Ok(head) = repo.head() else {
            return Ok(Vec::new());
        };
        let Ok(head_commit) = head.peel_to_commit() else {
            return Ok(Vec::new());
        };
        revwalk.push(head_commit.id()).context("push HEAD")?;
    }

    // Pass 1: collect just the rows we'll return. Cheap — bounded by
    // max_count and the since cutoff.
    let mut raw: Vec<(Oid, git2::Commit<'_>)> = Vec::with_capacity(max_count);
    for oid in revwalk {
        if raw.len() >= max_count {
            break;
        }
        let oid = match oid {
            Ok(o) => o,
            Err(_) => continue,
        };
        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if commit.time().seconds() < since_unix_seconds {
            break;
        }
        raw.push((oid, commit));
    }

    // Pass 2: branch labels, computed only against the OIDs we'll actually
    // return — bounded per-branch and short-circuits when all targets resolve.
    let targets: HashSet<Oid> = raw.iter().map(|(oid, _)| *oid).collect();
    let labels = compute_branch_labels(&repo, head_branch_name.as_deref(), &targets);

    let mut out: Vec<CommitSummary> = Vec::with_capacity(raw.len());
    for (oid, commit) in raw {
        let hash = oid.to_string();
        let short_hash: String = hash.chars().take(7).collect();
        let summary = commit.summary().unwrap_or("").to_string();
        let author = commit.author();
        let parents: Vec<String> = commit.parent_ids().map(|p| p.to_string()).collect();
        let message = commit.message().unwrap_or("").to_string();
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

    Ok(out)
}

/// Walk every local head and return up to `max_count` recent commits whose
/// author time is at or after `since_unix_seconds`. Empty / detached repos
/// return an empty vector rather than erroring.
pub fn recent_commits(
    repo_path: &Path,
    max_count: usize,
    since_unix_seconds: i64,
) -> Result<Vec<CommitSummary>> {
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

    let tagged_oids = collect_tagged_oids(&repo);

    // Remote tracking refs (origin/*) are local-file reads — see comment in
    // `repo_commits` for the rationale. We always include them here because
    // `recent_commits` is the all-branches view by definition.
    let remote_tips =
        collect_origin_remote_tips(&repo, since_unix_seconds, REMOTE_TIP_ROOT_CAP);
    let remote_badges = build_remote_badge_map(&remote_tips);

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
    if let Ok(refs) = repo.references_glob("refs/heads/*") {
        for r in refs.flatten() {
            if let Some(oid) = r.target() {
                if revwalk.push(oid).is_ok() {
                    pushed += 1;
                }
            }
        }
    }
    // Add remote tip OIDs as extra roots — surfaces commits pushed to
    // origin/<branch> that have no local head.
    for tip in &remote_tips {
        let _ = revwalk.push(tip.oid);
    }
    // Fall back to HEAD if for some reason no heads were enumerable
    // (newly-init'd repo, detached HEAD, etc.) AND no remote tips.
    if pushed == 0 && remote_tips.is_empty() {
        let head = match repo.head() {
            Ok(h) => h,
            Err(_) => return Ok(Vec::new()),
        };
        let head_commit = match head.peel_to_commit() {
            Ok(c) => c,
            Err(_) => return Ok(Vec::new()),
        };
        revwalk.push(head_commit.id()).context("push HEAD")?;
    }

    // Pass 1: collect the output rows first. Bounded by max_count and the
    // since cutoff (TIME sort is descending, so once we cross the cutoff
    // older commits won't reappear later).
    let mut raw: Vec<(Oid, git2::Commit<'_>)> = Vec::with_capacity(max_count);
    for oid in revwalk {
        if raw.len() >= max_count {
            break;
        }
        let oid = match oid {
            Ok(o) => o,
            Err(_) => continue,
        };
        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if commit.time().seconds() < since_unix_seconds {
            break;
        }
        raw.push((oid, commit));
    }

    // Pass 2: branch labels for just the OIDs we'll return. Used to be a
    // O(branches × history) prefetch — now bounded per-branch and exits
    // early once every target is resolved.
    let targets: HashSet<Oid> = raw.iter().map(|(oid, _)| *oid).collect();
    let labels = compute_branch_labels(&repo, head_branch_name.as_deref(), &targets);

    let mut out: Vec<CommitSummary> = Vec::with_capacity(raw.len());
    for (oid, commit) in raw {
        let hash = oid.to_string();
        let short_hash: String = hash.chars().take(7).collect();
        let summary = commit.summary().unwrap_or("").to_string();
        let author = commit.author();
        let parents: Vec<String> = commit.parent_ids().map(|p| p.to_string()).collect();
        let message = commit.message().unwrap_or("").to_string();
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

    Ok(out)
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
            commits.iter().map(|c| c.summary.as_str()).collect::<Vec<_>>(),
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
}
