//! Read-only tree comparisons and bounded patch rendering.
use anyhow::{Context, Result};
use git2::Repository;
use serde::{Deserialize, Serialize};
use std::path::Path;

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

pub(crate) fn classify_binary(path: &str, git2_says_binary: bool) -> bool {
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

    describe_diff(&repo, &diff)
}

pub(crate) fn describe_diff(repo: &Repository, diff: &git2::Diff<'_>) -> Result<Vec<ChangedFile>> {
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

pub const MAX_DIFF_BYTES: usize = 5 * 1024 * 1024;

/// Exact provenance and completeness travel with the patch, including in caches.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePatch {
    pub text: String,
    pub truncated: bool,
    pub reason: Option<String>,
    pub old_path: String,
    pub new_path: String,
    pub old_revision: String,
    pub new_revision: String,
}

pub fn file_diff(repo_path: &Path, hash: &str, path: &str, context: u32) -> Result<FilePatch> {
    let repo = Repository::open(repo_path)?;
    let commit = repo.find_commit(git2::Oid::from_str(hash)?)?;
    let base = commit.parent_id(0).ok().map(|id| id.to_string());
    range_patch(&repo, base.as_deref(), hash, path, context)
}

pub(crate) fn tree_diff<'r>(
    repo: &'r Repository,
    base: Option<&str>,
    head: &str,
    context: u32,
    force_text: bool,
) -> Result<git2::Diff<'r>> {
    let old = base
        .map(|id| repo.find_commit(git2::Oid::from_str(id)?)?.tree())
        .transpose()?;
    let new = repo.find_commit(git2::Oid::from_str(head)?)?.tree()?;
    let mut opts = git2::DiffOptions::new();
    opts.context_lines(context.min(1_000_000))
        .include_typechange(true)
        .force_text(force_text);
    // Find renames on the entire tree BEFORE selecting the literal destination.
    let mut diff = repo.diff_tree_to_tree(old.as_ref(), Some(&new), Some(&mut opts))?;
    diff.find_similar(Some(
        git2::DiffFindOptions::new().renames(true).copies(false),
    ))?;
    Ok(diff)
}

pub(crate) fn range_patch(
    repo: &Repository,
    base: Option<&str>,
    head: &str,
    path: &str,
    context: u32,
) -> Result<FilePatch> {
    let diff = tree_diff(repo, base, head, context, true)?;
    patch_for_path(&diff, path, base.unwrap_or("empty tree"), head)
}

pub(crate) fn patch_for_path(
    diff: &git2::Diff<'_>,
    path: &str,
    old_revision: &str,
    new_revision: &str,
) -> Result<FilePatch> {
    let (index, delta) = diff
        .deltas()
        .enumerate()
        .find(|(_, d)| {
            let display = if d.status() == git2::Delta::Deleted {
                d.old_file()
            } else {
                d.new_file()
            };
            display.path() == Some(Path::new(path))
        })
        .context("File is not in this comparison")?;
    let mut result = FilePatch {
        text: String::new(),
        truncated: false,
        reason: None,
        old_path: delta
            .old_file()
            .path()
            .unwrap_or(Path::new(path))
            .to_string_lossy()
            .into_owned(),
        new_path: delta
            .new_file()
            .path()
            .unwrap_or(Path::new(path))
            .to_string_lossy()
            .into_owned(),
        old_revision: old_revision.into(),
        new_revision: new_revision.into(),
    };
    if let Some(mut patch) = git2::Patch::from_diff(diff, index)? {
        let printed = patch.print(&mut |_, _, line| {
            let prefix = match line.origin() {
                '+' | '-' | ' ' => Some(line.origin()),
                _ => None,
            };
            let content = String::from_utf8_lossy(line.content());
            let needed = content.len() + usize::from(prefix.is_some());
            if result.text.len() + needed > MAX_DIFF_BYTES {
                // Stop at a complete line; never present half a changed line as complete.
                result.truncated = true;
                result.reason = Some("patch_byte_limit".into());
                return false;
            }
            if let Some(prefix) = prefix {
                result.text.push(prefix);
            }
            result.text.push_str(&content);
            true
        });
        if !result.truncated {
            printed?;
        }
    }
    Ok(result)
}
