use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::{cache, discovery, git, settings};

const MAX_COMMITS_PER_REPO: usize = 10;
const MAX_COMMITS_PER_REPO_NO_WINDOW: usize = 1_000;
const TIMELINE_WINDOW_DAYS: i64 = 7;
const TIMELINE_MAX_TOTAL: usize = 50;
const TIMELINE_MAX_TOTAL_NO_WINDOW: usize = 5_000;

fn cutoff_for(window_days: Option<i64>) -> i64 {
    match window_days {
        Some(d) if d > 0 => unix_now() - d * 86_400,
        _ => 0,
    }
}

fn per_repo_cap(window_days: Option<i64>) -> usize {
    if window_days.is_none() {
        MAX_COMMITS_PER_REPO_NO_WINDOW
    } else {
        MAX_COMMITS_PER_REPO
    }
}

fn total_cap(window_days: Option<i64>) -> usize {
    if window_days.is_none() {
        TIMELINE_MAX_TOTAL_NO_WINDOW
    } else {
        TIMELINE_MAX_TOTAL
    }
}

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

#[derive(Clone, Serialize)]
struct TimelineRepoFill {
    commits: Vec<git::CommitSummary>,
}

#[tauri::command]
pub async fn list_recent_commits_cached(
    app: AppHandle,
    window_days: Option<i64>,
) -> Result<Vec<git::CommitSummary>, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<git::CommitSummary>, String> {
        let conn = cache::open(&app).map_err(|e| e.to_string())?;
        cache::list_recent_commits(&conn, cutoff_for(window_days), total_cap(window_days))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn recent_commits(
    app: AppHandle,
    window_days: Option<i64>,
) -> Result<Vec<git::CommitSummary>, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<git::CommitSummary>, String> {
        let conn = cache::open(&app).map_err(|e| e.to_string())?;
        let repos = cache::list_repos(&conn).map_err(|e| e.to_string())?;
        let cutoff = cutoff_for(window_days);
        let per_repo = per_repo_cap(window_days);
        let total = total_cap(window_days);

        let mut all: Vec<git::CommitSummary> = Vec::new();
        for repo in repos {
            let commits =
                git::recent_commits(Path::new(&repo.path), per_repo, cutoff).unwrap_or_default();
            for mut c in commits {
                c.repo_name = repo.name.clone();
                all.push(c);
            }
        }
        all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        all.truncate(total);

        let mut conn = cache::open(&app).map_err(|e| e.to_string())?;
        cache::upsert_commits(&mut conn, &all).map_err(|e| e.to_string())?;
        Ok(all)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn list_branches(repo_path: String) -> Result<Vec<git::BranchInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git::list_branches(Path::new(&repo_path)).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn repo_commits(
    app: AppHandle,
    repo_path: String,
    branches: Option<Vec<String>>,
    window_days: Option<i64>,
) -> Result<Vec<git::CommitSummary>, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<git::CommitSummary>, String> {
        let cutoff = cutoff_for(window_days);
        let cap = per_repo_cap(window_days);
        let branches_slice: Option<&[String]> = branches.as_deref();
        let commits = git::repo_commits(Path::new(&repo_path), branches_slice, cap, cutoff)
            .map_err(|e| e.to_string())?;
        // Persist into the same commits cache so warm starts paint instantly
        // for this repo too.
        if !commits.is_empty() {
            if let Ok(mut conn) = cache::open(&app) {
                let _ = cache::upsert_commits(&mut conn, &commits);
            }
        }
        Ok(commits)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn get_pinned_repos(app: AppHandle) -> Vec<String> {
    settings::load(&app).pinned_repos
}

#[tauri::command]
pub fn set_pinned_repos(app: AppHandle, repos: Vec<String>) {
    settings::save_pinned_repos(&app, repos);
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

        let cutoff = unix_now() - TIMELINE_WINDOW_DAYS * 86_400;

        for root in &roots {
            let root_str = root.to_string_lossy().into_owned();
            discovery::scan_path(root, |path| {
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let path_str = path.to_string_lossy().into_owned();
                let repo = cache::Repo {
                    path: path_str.clone(),
                    name: name.clone(),
                };
                found.push(repo.clone());
                let _ = app.emit(
                    "discovery://progress",
                    ScanProgress {
                        root: root_str.clone(),
                        found: found.len(),
                    },
                );

                // Fill the timeline incrementally: read this repo's recent
                // commits right now and stream them to the panel so rows
                // appear as repos are found. Errors are silently skipped.
                let commits = git::recent_commits(&path, MAX_COMMITS_PER_REPO, cutoff)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|mut c| {
                        c.repo_name = name.clone();
                        c
                    })
                    .collect::<Vec<_>>();

                if !commits.is_empty() {
                    if let Ok(mut conn) = cache::open(&app) {
                        let _ = cache::upsert_commits(&mut conn, &commits);
                    }
                    let _ = app.emit(
                        "timeline://repo-fill",
                        TimelineRepoFill { commits },
                    );
                }
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
