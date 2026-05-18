use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    pub path: String,
    pub name: String,
}

fn db_path(app: &AppHandle) -> Result<PathBuf> {
    let dir = app
        .path()
        .app_config_dir()
        .context("resolving app config dir")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("mkdir {}", dir.display()))?;
    Ok(dir.join("cache.db"))
}

pub fn open(app: &AppHandle) -> Result<Connection> {
    let path = db_path(app)?;
    let conn = Connection::open(&path).with_context(|| format!("open {}", path.display()))?;
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS repos (
            path TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            discovered_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_repos_last_seen ON repos(last_seen_at);
        "#,
    )?;
    Ok(conn)
}

pub fn list_repos(conn: &Connection) -> Result<Vec<Repo>> {
    let mut stmt = conn.prepare("SELECT path, name FROM repos ORDER BY name COLLATE NOCASE")?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Repo {
                path: row.get(0)?,
                name: row.get(1)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn upsert_repos(conn: &mut Connection, repos: &[Repo]) -> Result<()> {
    let now = unix_now();
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            r#"
            INSERT INTO repos (path, name, discovered_at, last_seen_at)
            VALUES (?1, ?2, ?3, ?3)
            ON CONFLICT(path) DO UPDATE SET
                name = excluded.name,
                last_seen_at = excluded.last_seen_at
            "#,
        )?;
        for r in repos {
            stmt.execute(params![r.path, r.name, now])?;
        }
    }
    tx.commit()?;
    Ok(())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
