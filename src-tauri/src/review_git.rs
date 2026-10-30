use crate::{git_diff, review_types::*, review_worktree};
use anyhow::{bail, Context, Result};
use git2::Repository;
use std::{
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn head(repo: &Repository) -> Option<String> {
    repo.head()
        .ok()
        .and_then(|r| r.peel_to_commit().ok())
        .map(|c| c.id().to_string())
}
pub fn branch(repo: &Repository) -> String {
    repo.head()
        .ok()
        .filter(|r| r.is_branch())
        .and_then(|r| r.name().map(str::to_owned))
        .unwrap_or_else(|| {
            if repo.head_detached().unwrap_or(false) {
                "detached HEAD".into()
            } else {
                repo.find_reference("HEAD")
                    .ok()
                    .and_then(|r| r.symbolic_target().map(str::to_owned))
                    .unwrap_or_else(|| "HEAD".into())
            }
        })
}
pub fn resolve(repo: &Repository, revision: &str) -> Result<String> {
    if revision.is_empty() || revision.len() > 512 {
        bail!("Enter a commit revision of at most 512 bytes");
    }
    Ok(repo
        .revparse_single(revision)?
        .peel_to_commit()?
        .id()
        .to_string())
}

pub fn snapshot(
    request: &ReviewRequest,
    baseline: Option<ReviewBaseline>,
) -> Result<ReviewSnapshot> {
    let path = Path::new(request.repo_path());
    let repo = Repository::open(path)?;
    let identity = review_worktree::identity(path)?;
    let live_head = head(&repo);
    let branch = branch(&repo);
    let baseline_status = match &baseline {
        None => "missing",
        Some(b) if b.identity != identity => "repository_changed",
        Some(b) if repo.find_commit(git2::Oid::from_str(&b.head)?).is_err() => "missing_commit",
        Some(b)
            if live_head.as_ref().is_none_or(|h| {
                h != &b.head
                    && !repo
                        .graph_descendant_of(
                            git2::Oid::from_str(h).unwrap(),
                            git2::Oid::from_str(&b.head).unwrap(),
                        )
                        .unwrap_or(false)
            }) =>
        {
            "diverged"
        }
        Some(_) => "ready",
    }
    .to_string();
    let (mode, base, selected_head, fingerprint, files) = match request {
        ReviewRequest::Worktree { .. } => {
            let before = review_worktree::fingerprint(&repo)?;
            let files = review_worktree::files(&repo)?;
            if before != review_worktree::fingerprint(&repo)? {
                bail!("Working tree changed while reading. Refresh the review.");
            }
            (
                "worktree",
                live_head.clone(),
                live_head.clone(),
                Some(before),
                files,
            )
        }
        _ => {
            let (base, selected_head) = match request {
                ReviewRequest::Range { base, head, .. } => {
                    (resolve(&repo, base)?, resolve(&repo, head)?)
                }
                _ => {
                    let h = live_head
                        .clone()
                        .context("No commits yet. Open current changes instead.")?;
                    let base = if baseline_status == "ready" {
                        baseline.as_ref().unwrap().head.clone()
                    } else {
                        h.clone()
                    };
                    (base, h)
                }
            };
            let diff = git_diff::tree_diff(&repo, Some(&base), &selected_head, 3, false)?;
            let files = git_diff::describe_diff(&repo, &diff)?
                .into_iter()
                .map(|file| ReviewFile {
                    id: format!("range:{}", file.path),
                    group: "range".into(),
                    file,
                })
                .collect::<Vec<_>>();
            if files.len() > 10_000 {
                bail!("Range exceeds the 10,000-file review limit");
            }
            (
                if matches!(request, ReviewRequest::Range { .. }) {
                    "range"
                } else {
                    "sinceReviewed"
                },
                Some(base),
                Some(selected_head),
                None,
                files,
            )
        }
    };
    if head(&repo) != live_head
        || self::branch(&repo) != branch
        || review_worktree::identity(path)? != identity
    {
        bail!("Repository changed while reading. Refresh the review.");
    }
    let can_mark_reviewed = mode != "worktree"
        && selected_head == live_head
        && selected_head.is_some()
        && (mode == "range" || matches!(baseline_status.as_str(), "ready" | "missing"));
    Ok(ReviewSnapshot {
        id: crate::control::parser::new_request_id(),
        repo_path: request.repo_path().into(),
        repo_name: path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        branch,
        identity,
        mode: mode.into(),
        base,
        head: selected_head,
        live_head,
        baseline,
        baseline_status,
        fingerprint,
        created_at: SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        files,
        can_mark_reviewed,
    })
}

pub fn validate(snapshot: &ReviewSnapshot) -> Result<Repository> {
    let path = Path::new(&snapshot.repo_path);
    let repo = Repository::open(path)?;
    if review_worktree::identity(path)? != snapshot.identity
        || branch(&repo) != snapshot.branch
        || head(&repo) != snapshot.live_head
    {
        bail!("Repository or HEAD changed. Refresh the review.");
    }
    if let Some(expected) = &snapshot.fingerprint {
        if &review_worktree::fingerprint(&repo)? != expected {
            bail!("Working tree or index changed. Refresh the review.");
        }
    }
    Ok(repo)
}

pub fn patch(
    snapshot: &ReviewSnapshot,
    file_id: &str,
    context: u32,
) -> Result<git_diff::FilePatch> {
    let repo = validate(snapshot)?;
    let result = read_patch(&repo, snapshot, file_id, context)?;
    validate(snapshot)?;
    Ok(result)
}

// Callers must validate the same snapshot before and after the whole read batch.
pub(crate) fn read_patch(
    repo: &Repository,
    snapshot: &ReviewSnapshot,
    file_id: &str,
    context: u32,
) -> Result<git_diff::FilePatch> {
    let file = snapshot
        .files
        .iter()
        .find(|f| f.id == file_id)
        .context("File is not in this snapshot")?;
    if file.file.is_binary {
        bail!("Binary content is listed as metadata in work review");
    }
    let result = if let Some(fingerprint) = &snapshot.fingerprint {
        review_worktree::patch(
            &repo,
            &file.group,
            &file.file.path,
            context,
            snapshot.head.as_deref(),
            fingerprint,
        )?
    } else {
        git_diff::range_patch(
            &repo,
            snapshot.base.as_deref(),
            snapshot.head.as_deref().context("Missing head")?,
            &file.file.path,
            context,
        )?
    };
    Ok(result)
}
