use std::path::{Path, PathBuf};

use tauri::AppHandle;

use crate::{cache, discovery_orchestrator, git};

use super::model::{
    ControlError, ControlErrorCode, ControlIntent, ControlRequest, RepoLocator, ResolvedTarget,
    PROTOCOL_ID,
};

pub fn resolve(app: &AppHandle, request: &ControlRequest) -> Result<ResolvedTarget, ControlError> {
    if request.protocol != PROTOCOL_ID {
        return Err(ControlError::new(
            ControlErrorCode::InvalidArguments,
            format!("Unsupported control protocol `{}`", request.protocol),
        ));
    }

    let locator = match &request.intent {
        ControlIntent::ShowCommit { repo, .. }
        | ControlIntent::ShowFileHistory { repo, .. }
        | ControlIntent::ShowRange { repo, .. }
        | ControlIntent::ShowWorktree { repo } => repo,
    };
    let candidate = local_candidate(locator, &request.invocation_cwd)?;
    let canonical =
        discovery_orchestrator::exact_worktree_identity(&candidate).ok_or_else(|| {
            ControlError::new(
                ControlErrorCode::RepoUnavailable,
                format!(
                    "Could not open a Git working tree at `{}`",
                    candidate.display()
                ),
            )
        })?;

    let conn = cache::open(app).map_err(|error| internal("Open repository catalog", error))?;
    if discovery_orchestrator::repo_is_excluded(&conn, &canonical)
        .map_err(|error| internal("Check repository exclusion", error))?
    {
        return Err(ControlError::new(
            ControlErrorCode::RepoExcluded,
            "This repository is excluded in gitwink; restore it explicitly before opening it",
        ));
    }
    let fence = discovery_orchestrator::capture_active_repo_lifecycle_fence(&conn, &canonical)
        .map_err(|error| internal("Read repository lifecycle", error))?
        .ok_or_else(|| catalog_error(&conn, &canonical))?;
    let repo_path = fence.canonical_path().to_string();
    drop(conn);

    let target = match &request.intent {
        ControlIntent::ShowRange { base, head, .. } => {
            let repo =
                git2::Repository::open(&repo_path).map_err(|e| internal("Open repository", e))?;
            let base = crate::review_git::resolve(&repo, base).map_err(|e| {
                ControlError::new(ControlErrorCode::RevisionNotFound, e.to_string())
            })?;
            let head = crate::review_git::resolve(&repo, head).map_err(|e| {
                ControlError::new(ControlErrorCode::RevisionNotFound, e.to_string())
            })?;
            ResolvedTarget::Review {
                request: crate::review_types::ReviewRequest::Range {
                    repo_path: repo_path.clone(),
                    base,
                    head,
                },
            }
        }
        ControlIntent::ShowWorktree { .. } => ResolvedTarget::Review {
            request: crate::review_types::ReviewRequest::Worktree {
                repo_path: repo_path.clone(),
            },
        },
        ControlIntent::ShowCommit {
            revision,
            presentation,
            ..
        } => {
            let commit =
                git::resolve_commit_summary(Path::new(&repo_path), revision).map_err(|_| {
                    ControlError::new(
                        ControlErrorCode::RevisionNotFound,
                        format!("Revision `{revision}` was not found in this local repository"),
                    )
                })?;
            ResolvedTarget::Commit {
                repo_path: repo_path.clone(),
                commit,
                presentation: *presentation,
            }
        }
        ControlIntent::ShowFileHistory {
            file_path,
            presentation,
            ..
        } => ResolvedTarget::FileHistory {
            repo_path: repo_path.clone(),
            file_path: normalize_repo_relative_file(file_path)?,
            presentation: *presentation,
        },
    };

    let conn = cache::open(app).map_err(|error| internal("Reopen repository catalog", error))?;
    if !discovery_orchestrator::repo_lifecycle_fence_matches(&conn, &fence)
        .map_err(|error| internal("Verify repository lifecycle", error))?
    {
        return Err(ControlError::new(
            ControlErrorCode::RepositoryChanged,
            "Repository lifecycle changed while resolving the request; retry",
        ));
    }

    Ok(target)
}

fn local_candidate(locator: &RepoLocator, cwd: &str) -> Result<PathBuf, ControlError> {
    let RepoLocator::LocalPath { path } = locator;
    let raw = PathBuf::from(path);
    let candidate = if raw.is_absolute() {
        raw
    } else {
        Path::new(cwd).join(raw)
    };
    if candidate.as_os_str().is_empty() {
        return Err(ControlError::new(
            ControlErrorCode::InvalidArguments,
            "Repository path cannot be empty",
        ));
    }
    Ok(candidate)
}

pub(crate) fn normalize_repo_relative_file(raw: &str) -> Result<String, ControlError> {
    let value = raw.trim();
    if value.is_empty() || value.as_bytes().contains(&0) {
        return Err(invalid_file("File path cannot be empty"));
    }
    let normalized = if cfg!(windows) {
        value.replace('\\', "/")
    } else {
        value.to_string()
    };
    let bytes = normalized.as_bytes();
    if normalized.starts_with('/') || normalized.starts_with("//") || bytes.get(1) == Some(&b':') {
        return Err(invalid_file(
            "File path must be relative to the repository root",
        ));
    }

    let mut components = Vec::new();
    for component in normalized.split('/') {
        match component {
            "" => return Err(invalid_file("File path contains an empty component")),
            "." => continue,
            ".." => return Err(invalid_file("File path may not escape the repository root")),
            value => components.push(value),
        }
    }
    if components.is_empty() {
        return Err(invalid_file("File path must name a file"));
    }
    Ok(components.join("/"))
}

fn catalog_error(conn: &rusqlite::Connection, canonical: &str) -> ControlError {
    let missing = discovery_orchestrator::registered_missing_repo_identity(conn, canonical)
        .ok()
        .flatten()
        .is_some();
    let known = discovery_orchestrator::registered_repo_identity(conn, canonical)
        .ok()
        .flatten()
        .is_some();
    if missing || known {
        ControlError::new(
            ControlErrorCode::RepoUnavailable,
            "This repository is registered but is not currently active and readable",
        )
    } else {
        ControlError::new(
            ControlErrorCode::RepoNotRegistered,
            "This repository is not registered in gitwink; add it explicitly before opening it",
        )
    }
}

fn invalid_file(message: impl Into<String>) -> ControlError {
    ControlError::new(ControlErrorCode::InvalidFilePath, message)
}

fn internal(context: &str, error: impl std::fmt::Display) -> ControlError {
    ControlError::new(
        ControlErrorCode::InternalError,
        format!("{context} failed: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_clean_repo_relative_paths() {
        assert_eq!(
            normalize_repo_relative_file("src/./control/mod.rs").unwrap(),
            "src/control/mod.rs"
        );
    }

    #[test]
    fn rejects_absolute_parent_and_empty_components() {
        for value in ["/etc/passwd", "../secret", "src/../../secret", "a//b"] {
            assert!(normalize_repo_relative_file(value).is_err(), "{value}");
        }
    }
}
