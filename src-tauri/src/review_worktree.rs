//! Read-only index/worktree comparisons. No index refresh/write or object insertion.
use crate::{git_diff, review_types::ReviewFile};
use anyhow::{bail, Context, Result};
use git2::{Diff, DiffOptions, ObjectType, Oid, Repository, StatusOptions};
use std::{collections::BTreeSet, path::Path};

const MAX_FINGERPRINT_BYTES: u64 = 256 * 1024 * 1024;
const MAX_FILES: usize = 10_000;

pub fn fingerprint(repo: &Repository) -> Result<String> {
    let root = repo.workdir().context("A working tree is required")?;
    let mut opts = StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false)
        .update_index(false);
    let statuses = repo.statuses(Some(&mut opts))?;
    if statuses.len() > MAX_FILES {
        bail!("Current changes exceed the 10,000-file review limit");
    }
    let mut paths = BTreeSet::new();
    let mut parts = Vec::new();
    for entry in statuses.iter() {
        parts.push(format!("status:{}", entry.status().bits()));
        for delta in [entry.head_to_index(), entry.index_to_workdir()]
            .into_iter()
            .flatten()
        {
            for file in [delta.old_file(), delta.new_file()] {
                if let Some(path) = file.path() {
                    paths.insert(path.to_path_buf());
                }
            }
        }
    }
    let index = repo.path().join("index");
    parts.push(if index.exists() {
        Oid::hash_file(ObjectType::Blob, index)?.to_string()
    } else {
        "no-index".into()
    });
    let mut bytes = 0u64;
    for path in paths {
        let full = root.join(&path);
        parts.push(path.to_string_lossy().into_owned());
        match std::fs::symlink_metadata(&full) {
            Ok(meta) if meta.file_type().is_symlink() => parts.push(format!(
                "symlink:{}",
                std::fs::read_link(&full)?.to_string_lossy()
            )),
            Ok(meta) if meta.is_file() => {
                bytes = bytes.saturating_add(meta.len());
                if bytes > MAX_FINGERPRINT_BYTES {
                    bail!("Current changes exceed the 256 MiB review snapshot limit");
                }
                parts.push(format!(
                    "{}:{}:{}",
                    meta.len(),
                    meta.permissions().readonly(),
                    Oid::hash_file(ObjectType::Blob, &full)?
                ));
            }
            Ok(_) => {
                // Submodule contents are never read recursively. Its recorded HEAD is evidence.
                let sub = Repository::open(&full)
                    .ok()
                    .and_then(|r| r.head().ok()?.target());
                parts.push(format!("directory:{sub:?}"));
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => parts.push("absent".into()),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(Oid::hash_object(ObjectType::Blob, &serde_json::to_vec(&parts)?)?.to_string())
}

pub fn diff<'a>(
    repo: &'a Repository,
    group: &str,
    context: u32,
    force_text: bool,
) -> Result<Diff<'a>> {
    let index = repo.index()?;
    if index.has_conflicts() {
        bail!("The index contains conflicts. Resolve them before opening a worktree review.");
    }
    let mut opts = DiffOptions::new();
    opts.context_lines(context.min(25))
        .include_typechange(true)
        .force_text(force_text);
    let mut diff = if group == "staged" {
        let tree = repo.head().ok().and_then(|head| head.peel_to_tree().ok());
        repo.diff_tree_to_index(tree.as_ref(), Some(&index), Some(&mut opts))?
    } else {
        opts.include_untracked(true)
            .recurse_untracked_dirs(true)
            .show_untracked_content(true);
        repo.diff_index_to_workdir(Some(&index), Some(&mut opts))?
    };
    // Only staged renames: unstaged renames must not consume untracked files and hide their group.
    if group == "staged" {
        diff.find_similar(Some(git2::DiffFindOptions::new().renames(true)))?;
    }
    Ok(diff)
}

pub fn files(repo: &Repository) -> Result<Vec<ReviewFile>> {
    let mut result = Vec::new();
    for group in ["staged", "unstaged"] {
        let diff = diff(repo, group, 3, false)?;
        let files = git_diff::describe_diff(repo, &diff)?;
        for (file, delta) in files.into_iter().zip(diff.deltas()) {
            let group = if delta.status() == git2::Delta::Untracked {
                "untracked"
            } else {
                group
            };
            result.push(ReviewFile {
                id: format!("{group}:{}", file.path),
                group: group.into(),
                file,
            });
        }
    }
    if result.len() > MAX_FILES {
        bail!("Current changes exceed the 10,000-file review limit");
    }
    Ok(result)
}

pub fn patch(
    repo: &Repository,
    group: &str,
    path: &str,
    context: u32,
    head: Option<&str>,
    fingerprint: &str,
) -> Result<git_diff::FilePatch> {
    let diff = diff(repo, group, context, true)?;
    let index_ref = format!("index@{fingerprint}");
    let work_ref = format!("worktree@{fingerprint}");
    git_diff::patch_for_path(
        &diff,
        path,
        if group == "staged" {
            head.unwrap_or("empty tree")
        } else {
            &index_ref
        },
        if group == "staged" {
            &index_ref
        } else {
            &work_ref
        },
    )
}

pub fn identity(path: &Path) -> Result<String> {
    crate::git::repo_storage_identity(path)
}
