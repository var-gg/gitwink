use crate::{cache, discovery_orchestrator as catalog, review_git, review_store, review_types::*};
use anyhow::{bail, Context, Result};
use std::{collections::VecDeque, path::Path, sync::Mutex};
use tauri::{AppHandle, Manager};

#[derive(Default)]
pub struct ReviewSessions(Mutex<VecDeque<ReviewSnapshot>>);
impl ReviewSessions {
    fn get(&self, id: &str) -> Result<ReviewSnapshot> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .context("Review expired. Refresh to open a new snapshot.")
    }
    fn insert(&self, snapshot: ReviewSnapshot) {
        let mut sessions = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // File lists only; patch payloads are never retained in a global cache.
        sessions.push_back(snapshot);
        while sessions.len() > 8 {
            sessions.pop_front();
        }
    }
}

fn store(app: &AppHandle) -> Result<rusqlite::Connection> {
    let dir = app.path().app_data_dir()?;
    std::fs::create_dir_all(&dir)?;
    let conn = rusqlite::Connection::open(dir.join("review.sqlite"))?;
    review_store::initialize(&conn)?;
    Ok(conn)
}

fn check_active(app: &AppHandle, path: &str) -> Result<()> {
    let conn = cache::open(app)?;
    if catalog::repo_is_excluded(&conn, path)? {
        bail!("Repository is excluded from gitwink");
    }
    let fence = catalog::capture_active_repo_lifecycle_fence(&conn, path)?
        .context("Repository must be registered and active in gitwink")?;
    if !catalog::repo_lifecycle_fence_matches(&conn, &fence)? {
        bail!("Repository changed. Refresh the review.");
    }
    Ok(())
}

#[tauri::command]
pub async fn review_snapshot(
    app: AppHandle,
    request: ReviewRequest,
) -> Result<ReviewSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<_, String> {
        (|| -> Result<_> {
            check_active(&app, request.repo_path())?;
            let repo = git2::Repository::open(Path::new(request.repo_path()))?;
            let baseline = review_store::load(
                &store(&app)?,
                request.repo_path(),
                &review_git::branch(&repo),
            )?;
            let snapshot = review_git::snapshot(&request, baseline)?;
            check_active(&app, request.repo_path())?;
            app.state::<ReviewSessions>().insert(snapshot.clone());
            Ok(snapshot)
        })()
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn review_patch(
    app: AppHandle,
    snapshot_id: String,
    file_id: String,
    context: u32,
) -> Result<crate::git_diff::FilePatch, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<_, String> {
        (|| -> Result<_> {
            let snapshot = app.state::<ReviewSessions>().get(&snapshot_id)?;
            check_active(&app, &snapshot.repo_path)?;
            let patch = review_git::patch(&snapshot, &file_id, context.min(25))?;
            check_active(&app, &snapshot.repo_path)?;
            Ok(patch)
        })()
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn review_validate(app: AppHandle, snapshot_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        (|| -> Result<()> {
            let snapshot = app.state::<ReviewSessions>().get(&snapshot_id)?;
            check_active(&app, &snapshot.repo_path)?;
            review_git::validate(&snapshot)?;
            Ok(())
        })()
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn review_mark_reviewed(app: AppHandle, snapshot_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        (|| -> Result<()> {
            let snapshot = app.state::<ReviewSessions>().get(&snapshot_id)?;
            check_active(&app, &snapshot.repo_path)?;
            review_git::validate(&snapshot)?;
            if !snapshot.can_mark_reviewed {
                bail!("Only the inspected current HEAD can become the review baseline");
            }
            review_store::save(
                &mut store(&app)?,
                &snapshot.repo_path,
                &snapshot.branch,
                snapshot.baseline.as_ref(),
                &ReviewBaseline {
                    identity: snapshot.identity,
                    head: snapshot.head.context("No commit to mark reviewed")?,
                },
            )
        })()
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
