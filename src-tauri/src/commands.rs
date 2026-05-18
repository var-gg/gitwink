use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::{cache, discovery};

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
