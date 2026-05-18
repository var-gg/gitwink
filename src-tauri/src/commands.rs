use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::{cache, discovery, git};

const MAX_COMMITS_PER_REPO: usize = 10;
const TIMELINE_WINDOW_DAYS: i64 = 7;
const TIMELINE_MAX_TOTAL: usize = 50;

#[tauri::command]
pub fn ping() -> &'static str {
    "pong"
}

#[tauri::command]
pub fn list_repos(app: AppHandle) -> Result<Vec<cache::Repo>, String> {
    let conn = cache::open(&app).map_err(|e| e.to_string())?;
    cache::list_repos(&conn).map_err(|e| e.to_string())
}

#[derive(Clone, Serialize)]
struct ScanProgress {
    root: String,
    found: usize,
}

#[derive(Clone, Serialize)]
struct ScanComplete {
    count: usize,
}

#[tauri::command]
pub async fn list_recent_commits_cached(
    app: AppHandle,
) -> Result<Vec<git::CommitSummary>, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<git::CommitSummary>, String> {
        let conn = cache::open(&app).map_err(|e| e.to_string())?;
        let cutoff = unix_now() - TIMELINE_WINDOW_DAYS * 86_400;
        cache::list_recent_commits(&conn, cutoff, TIMELINE_MAX_TOTAL).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn recent_commits(app: AppHandle) -> Result<Vec<git::CommitSummary>, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<git::CommitSummary>, String> {
        let conn = cache::open(&app).map_err(|e| e.to_string())?;
        let repos = cache::list_repos(&conn).map_err(|e| e.to_string())?;
        let cutoff = unix_now() - TIMELINE_WINDOW_DAYS * 86_400;

        let mut all: Vec<git::CommitSummary> = Vec::new();
        for repo in repos {
            let commits = git::recent_commits(
                Path::new(&repo.path),
                MAX_COMMITS_PER_REPO,
                cutoff,
            )
            .unwrap_or_default();
            for mut c in commits {
                c.repo_name = repo.name.clone();
                all.push(c);
            }
        }
        all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        all.truncate(TIMELINE_MAX_TOTAL);

        // Persist for the next cold start.
        let mut conn = cache::open(&app).map_err(|e| e.to_string())?;
        cache::upsert_commits(&mut conn, &all).map_err(|e| e.to_string())?;
        Ok(all)
    })
    .await
    .map_err(|e| e.to_string())?
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[tauri::command]
pub async fn discover_repos(app: AppHandle) -> Result<usize, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<usize, String> {
        let mut found: Vec<cache::Repo> = Vec::new();
        let roots = discovery::default_roots();

        for root in &roots {
            let root_str = root.to_string_lossy().into_owned();
            discovery::scan_path(root, |path| {
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                found.push(cache::Repo {
                    path: path.to_string_lossy().into_owned(),
                    name,
                });
                let _ = app.emit(
                    "discovery://progress",
                    ScanProgress {
                        root: root_str.clone(),
                        found: found.len(),
                    },
                );
            });
        }

        let mut conn = cache::open(&app).map_err(|e| e.to_string())?;
        cache::upsert_repos(&mut conn, &found).map_err(|e| e.to_string())?;

        let count = found.len();
        let _ = app.emit("discovery://complete", ScanComplete { count });
        Ok(count)
    })
    .await
    .map_err(|e| e.to_string())?
}
