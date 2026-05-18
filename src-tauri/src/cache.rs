use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::git::CommitSummary;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Repo {
    pub path: String,
    pub name: String,
}

/// Default ceiling for the diffs blob store. Older entries are evicted in
/// LRU order (by last_accessed_at) on startup to bring the total back to
/// half the cap.
pub const DIFF_CACHE_MAX_BYTES: i64 = 500 * 1024 * 1024;

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

        CREATE TABLE IF NOT EXISTS commits (
            repo_path TEXT NOT NULL,
            hash TEXT NOT NULL,
            repo_name TEXT NOT NULL,
            short_hash TEXT NOT NULL,
            summary TEXT NOT NULL,
            author TEXT NOT NULL,
            email TEXT NOT NULL,
            timestamp INTEGER NOT NULL,
            PRIMARY KEY (repo_path, hash)
        );
        CREATE INDEX IF NOT EXISTS idx_commits_ts ON commits(timestamp);

        CREATE TABLE IF NOT EXISTS diffs (
            repo_path TEXT NOT NULL,
            hash TEXT NOT NULL,
            file_path TEXT NOT NULL,
            patch_text TEXT NOT NULL,
            last_accessed_at INTEGER NOT NULL,
            bytes INTEGER NOT NULL,
            PRIMARY KEY (repo_path, hash, file_path)
        );
        CREATE INDEX IF NOT EXISTS idx_diffs_lru ON diffs(last_accessed_at);
        "#,
    )?;
    Ok(conn)
}

// ----- repos -----

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

// ----- commits -----

pub fn upsert_commits(conn: &mut Connection, commits: &[CommitSummary]) -> Result<()> {
    if commits.is_empty() {
        return Ok(());
    }
    let tx = conn.transaction()?;
    {
        let mut stmt = tx.prepare(
            r#"
            INSERT INTO commits (repo_path, hash, repo_name, short_hash, summary, author, email, timestamp)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            ON CONFLICT(repo_path, hash) DO UPDATE SET
                repo_name = excluded.repo_name,
                summary = excluded.summary,
                author = excluded.author,
                email = excluded.email,
                timestamp = excluded.timestamp
            "#,
        )?;
        for c in commits {
            stmt.execute(params![
                c.repo_path,
                c.hash,
                c.repo_name,
                c.short_hash,
                c.summary,
                c.author,
                c.email,
                c.timestamp
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

pub fn list_recent_commits(
    conn: &Connection,
    since: i64,
    limit: usize,
) -> Result<Vec<CommitSummary>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT repo_path, repo_name, hash, short_hash, summary, author, email, timestamp
        FROM commits
        WHERE timestamp >= ?1
        ORDER BY timestamp DESC
        LIMIT ?2
        "#,
    )?;
    let rows = stmt
        .query_map(params![since, limit as i64], |row| {
            Ok(CommitSummary {
                repo_path: row.get(0)?,
                repo_name: row.get(1)?,
                hash: row.get(2)?,
                short_hash: row.get(3)?,
                summary: row.get(4)?,
                author: row.get(5)?,
                email: row.get(6)?,
                timestamp: row.get(7)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ----- diffs -----

#[allow(dead_code)] // wired up by Phase D (separate diff window)
pub fn get_diff(
    conn: &Connection,
    repo_path: &str,
    hash: &str,
    file_path: &str,
) -> Result<Option<String>> {
    let mut stmt = conn.prepare(
        "SELECT patch_text FROM diffs WHERE repo_path = ?1 AND hash = ?2 AND file_path = ?3",
    )?;
    let res = stmt.query_row(params![repo_path, hash, file_path], |r| {
        r.get::<_, String>(0)
    });
    match res {
        Ok(s) => {
            conn.execute(
                "UPDATE diffs SET last_accessed_at = ?1 WHERE repo_path = ?2 AND hash = ?3 AND file_path = ?4",
                params![unix_now(), repo_path, hash, file_path],
            )?;
            Ok(Some(s))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[allow(dead_code)] // wired up by Phase D (separate diff window)
pub fn put_diff(
    conn: &Connection,
    repo_path: &str,
    hash: &str,
    file_path: &str,
    patch: &str,
) -> Result<()> {
    let now = unix_now();
    let bytes = patch.len() as i64;
    conn.execute(
        r#"
        INSERT INTO diffs (repo_path, hash, file_path, patch_text, last_accessed_at, bytes)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(repo_path, hash, file_path) DO UPDATE SET
            patch_text = excluded.patch_text,
            last_accessed_at = excluded.last_accessed_at,
            bytes = excluded.bytes
        "#,
        params![repo_path, hash, file_path, patch, now, bytes],
    )?;
    Ok(())
}

/// Evict diff rows in LRU order (oldest `last_accessed_at` first) until
/// the total size drops to ~half the cap. Returns how many rows were
/// removed. Safe to call when the cache is small — it is a no-op then.
pub fn gc_diffs(conn: &mut Connection, max_bytes: i64) -> Result<usize> {
    let total: i64 =
        conn.query_row("SELECT COALESCE(SUM(bytes), 0) FROM diffs", [], |r| r.get(0))?;
    if total <= max_bytes {
        return Ok(0);
    }
    let target = max_bytes / 2;
    let to_remove = total - target;

    let mut victims: Vec<i64> = Vec::new();
    {
        let mut stmt =
            conn.prepare("SELECT rowid, bytes FROM diffs ORDER BY last_accessed_at ASC")?;
        let mut rows = stmt.query([])?;
        let mut acc: i64 = 0;
        while let Some(row) = rows.next()? {
            if acc >= to_remove {
                break;
            }
            let rowid: i64 = row.get(0)?;
            let bytes: i64 = row.get(1)?;
            victims.push(rowid);
            acc += bytes;
        }
    }

    let tx = conn.transaction()?;
    let mut removed = 0;
    {
        let mut del = tx.prepare("DELETE FROM diffs WHERE rowid = ?1")?;
        for rowid in &victims {
            removed += del.execute(params![rowid])?;
        }
    }
    tx.commit()?;
    Ok(removed)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
