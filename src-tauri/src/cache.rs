use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::git::CommitSummary;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    pub path: String,
    pub name: String,
    /// Lifecycle status: "active" | "missing" | "removed". Frontend
    /// greys out "missing" rows and filters out "removed". Defaults to
    /// "active" for old rows that predate the lifecycle migration.
    #[serde(default = "default_status")]
    pub status: String,
}

fn default_status() -> String {
    "active".to_string()
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
            branch_label TEXT,
            is_merge INTEGER NOT NULL DEFAULT 0,
            is_tagged INTEGER NOT NULL DEFAULT 0,
            parents TEXT NOT NULL DEFAULT '[]',
            message TEXT NOT NULL DEFAULT '',
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
    // Migrations: idempotent ALTERs for columns added after the original
    // schema. SQLite raises "duplicate column" on a fresh DB; ignored.
    let _ = conn.execute("ALTER TABLE commits ADD COLUMN branch_label TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE commits ADD COLUMN is_merge INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE commits ADD COLUMN is_tagged INTEGER NOT NULL DEFAULT 0",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE commits ADD COLUMN parents TEXT NOT NULL DEFAULT '[]'",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE commits ADD COLUMN message TEXT NOT NULL DEFAULT ''",
        [],
    );
    let _ = conn.execute("ALTER TABLE commits ADD COLUMN remote_tip_label TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE commits ADD COLUMN remote_tip_extra_count INTEGER NOT NULL DEFAULT 0",
        [],
    );

    // v0.1.1 discovery lifecycle migration. `repos` gains identity +
    // status + provenance fields so the tiered scanner can express
    // "this row came from VS Code recents, confidence 80, last verified
    // 3 minutes ago, currently missing on disk." All ALTERs are
    // idempotent ("duplicate column" errors swallowed on warm DBs).
    let _ = conn.execute("ALTER TABLE repos ADD COLUMN canonical_path TEXT", []);
    let _ = conn.execute("ALTER TABLE repos ADD COLUMN gitdir_path TEXT", []);
    let _ = conn.execute(
        "ALTER TABLE repos ADD COLUMN status TEXT NOT NULL DEFAULT 'active'",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE repos ADD COLUMN user_state TEXT NOT NULL DEFAULT 'normal'",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE repos ADD COLUMN primary_source TEXT NOT NULL DEFAULT 'unknown'",
        [],
    );
    let _ = conn.execute(
        "ALTER TABLE repos ADD COLUMN confidence INTEGER NOT NULL DEFAULT 50",
        [],
    );
    let _ = conn.execute("ALTER TABLE repos ADD COLUMN first_seen_at INTEGER", []);
    let _ = conn.execute("ALTER TABLE repos ADD COLUMN last_verified_at INTEGER", []);
    let _ = conn.execute("ALTER TABLE repos ADD COLUMN missing_since INTEGER", []);
    let _ = conn.execute("ALTER TABLE repos ADD COLUMN removed_at INTEGER", []);
    let _ = conn.execute("ALTER TABLE repos ADD COLUMN last_emit_at INTEGER", []);
    let _ = conn.execute(
        "ALTER TABLE repos ADD COLUMN repo_kind TEXT NOT NULL DEFAULT 'workdir'",
        [],
    );

    // Backfill canonical_path for rows from older versions so the
    // unique index has something to grab onto.
    let _ = conn.execute(
        "UPDATE repos SET canonical_path = path WHERE canonical_path IS NULL",
        [],
    );

    conn.execute_batch(
        r#"
        CREATE UNIQUE INDEX IF NOT EXISTS idx_repos_canonical_path
            ON repos(canonical_path);
        CREATE INDEX IF NOT EXISTS idx_repos_paint_order
            ON repos(user_state, status, confidence, last_seen_at);

        -- Per-source provenance: a repo can be known to multiple sources
        -- (VS Code recents AND fs walk AND manual), each with its own
        -- confidence + last-seen so source-level decay/learning works.
        CREATE TABLE IF NOT EXISTS repo_sources (
            repo_canonical_path TEXT NOT NULL,
            source TEXT NOT NULL,
            source_path TEXT,
            source_mtime INTEGER,
            raw_hint TEXT,
            confidence INTEGER NOT NULL,
            first_seen_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL,
            last_success_at INTEGER,
            fail_count INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(repo_canonical_path, source)
        );

        -- Learned scan roots. When we find a repo at C:\k2\keymall\workspace
        -- we learn C:\k2\keymall (score 0.95) and conservatively C:\k2
        -- (score 0.65), so next pass finds siblings without making the
        -- user re-enter paths.
        CREATE TABLE IF NOT EXISTS discovery_roots (
            root_path TEXT PRIMARY KEY,
            root_kind TEXT NOT NULL,
            created_from_repo TEXT,
            score REAL NOT NULL,
            max_depth INTEGER NOT NULL,
            entry_budget INTEGER NOT NULL,
            repo_hits INTEGER NOT NULL DEFAULT 0,
            miss_count INTEGER NOT NULL DEFAULT 0,
            last_scan_at INTEGER,
            cooldown_until INTEGER,
            enabled INTEGER NOT NULL DEFAULT 1,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_discovery_roots_priority
            ON discovery_roots(enabled, cooldown_until, score DESC, last_scan_at);

        -- "User hid this repo, do not auto-rediscover it." Survives even if
        -- a tier source still reports the path next scan.
        CREATE TABLE IF NOT EXISTS discovery_tombstones (
            canonical_path TEXT PRIMARY KEY,
            removed_at INTEGER NOT NULL,
            reason TEXT NOT NULL,
            last_known_name TEXT,
            last_known_source TEXT
        );

        -- observed path → canonical path mapping, so symlinks/case
        -- variants from different IDEs don't double-count.
        CREATE TABLE IF NOT EXISTS path_aliases (
            observed_path TEXT NOT NULL,
            canonical_path TEXT NOT NULL,
            source TEXT NOT NULL,
            first_seen_at INTEGER NOT NULL,
            last_seen_at INTEGER NOT NULL,
            PRIMARY KEY(observed_path, source)
        );

        -- Per-tier-run audit log. Mostly for debugging "why didn't my
        -- repo appear?" — keep tiny, GC the oldest entries periodically.
        CREATE TABLE IF NOT EXISTS discovery_runs (
            run_id TEXT PRIMARY KEY,
            tier TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            finished_at INTEGER,
            budget_ms INTEGER,
            candidates_seen INTEGER NOT NULL DEFAULT 0,
            candidates_valid INTEGER NOT NULL DEFAULT 0,
            repos_emitted INTEGER NOT NULL DEFAULT 0,
            cancelled INTEGER NOT NULL DEFAULT 0
        );

        -- One-row table for app-wide flags (e.g. first-run scan completion).
        -- Lives in cache.db rather than settings.json so a wiped cache
        -- correctly re-triggers the first-run scan.
        CREATE TABLE IF NOT EXISTS meta (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL,
            updated_at INTEGER NOT NULL
        );
        "#,
    )?;

    // PRAGMA user_version = 2 marks schema generation 2 (post v0.1.0).
    // We don't gate behaviour on this yet, but future migrations can.
    let _ = conn.pragma_update(None, "user_version", 2_i64);

    Ok(conn)
}

// ----- repos -----

pub fn list_repos(conn: &Connection) -> Result<Vec<Repo>> {
    // Filter out user-removed (tombstoned) rows so a deleted repo
    // doesn't reappear in the chip dropdown. Missing rows DO come
    // through — the UI greys them out so the user can decide.
    let mut stmt = conn.prepare(
        r#"
        SELECT path, name, status
        FROM repos
        WHERE user_state != 'removed'
        ORDER BY name COLLATE NOCASE
        "#,
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Repo {
                path: row.get(0)?,
                name: row.get(1)?,
                status: row.get(2)?,
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
            INSERT INTO repos (
                path, name, discovered_at, last_seen_at,
                canonical_path, first_seen_at, last_verified_at
            )
            VALUES (?1, ?2, ?3, ?3, ?1, ?3, ?3)
            ON CONFLICT(path) DO UPDATE SET
                name = excluded.name,
                last_seen_at = excluded.last_seen_at,
                last_verified_at = excluded.last_verified_at,
                -- Restore from missing if the path showed up again
                status = CASE
                    WHEN repos.status = 'missing' THEN 'active'
                    ELSE repos.status
                END,
                missing_since = CASE
                    WHEN repos.status = 'missing' THEN NULL
                    ELSE repos.missing_since
                END
            "#,
        )?;
        for r in repos {
            stmt.execute(params![r.path, r.name, now])?;
        }
    }
    tx.commit()?;
    Ok(())
}

// ----- meta (app-wide flags lived in cache.db so wiping cache resets them) -----

/// Read a value from the meta table. None if the key has never been set.
#[allow(dead_code)] // wired up by orchestrator
pub fn meta_get(conn: &Connection, key: &str) -> Result<Option<String>> {
    let res = conn.query_row(
        "SELECT value FROM meta WHERE key = ?1",
        params![key],
        |r| r.get::<_, String>(0),
    );
    match res {
        Ok(v) => Ok(Some(v)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Upsert a value into the meta table.
#[allow(dead_code)] // wired up by orchestrator
pub fn meta_set(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO meta (key, value, updated_at) VALUES (?1, ?2, ?3)
        ON CONFLICT(key) DO UPDATE SET
            value = excluded.value,
            updated_at = excluded.updated_at
        "#,
        params![key, value, unix_now()],
    )?;
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
            INSERT INTO commits (repo_path, hash, repo_name, short_hash, summary, author, email, timestamp, branch_label, is_merge, is_tagged, parents, message, remote_tip_label, remote_tip_extra_count)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            ON CONFLICT(repo_path, hash) DO UPDATE SET
                repo_name = excluded.repo_name,
                summary = excluded.summary,
                author = excluded.author,
                email = excluded.email,
                timestamp = excluded.timestamp,
                branch_label = excluded.branch_label,
                is_merge = excluded.is_merge,
                is_tagged = excluded.is_tagged,
                parents = excluded.parents,
                message = excluded.message,
                remote_tip_label = excluded.remote_tip_label,
                remote_tip_extra_count = excluded.remote_tip_extra_count
            "#,
        )?;
        for c in commits {
            let parents_json = serde_json::to_string(&c.parents).unwrap_or_else(|_| "[]".into());
            stmt.execute(params![
                c.repo_path,
                c.hash,
                c.repo_name,
                c.short_hash,
                c.summary,
                c.author,
                c.email,
                c.timestamp,
                c.branch_label,
                c.is_merge as i32,
                c.is_tagged as i32,
                parents_json,
                c.message,
                c.remote_tip_label,
                c.remote_tip_extra_count as i64,
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
        SELECT repo_path, repo_name, hash, short_hash, summary, author, email, timestamp, branch_label, is_merge, is_tagged, parents, message, remote_tip_label, remote_tip_extra_count
        FROM commits
        WHERE timestamp >= ?1
        ORDER BY timestamp DESC
        LIMIT ?2
        "#,
    )?;
    let rows = stmt
        .query_map(params![since, limit as i64], |row| {
            let parents_json: String = row.get(11)?;
            let parents: Vec<String> = serde_json::from_str(&parents_json).unwrap_or_default();
            Ok(CommitSummary {
                repo_path: row.get(0)?,
                repo_name: row.get(1)?,
                hash: row.get(2)?,
                short_hash: row.get(3)?,
                summary: row.get(4)?,
                author: row.get(5)?,
                email: row.get(6)?,
                timestamp: row.get(7)?,
                branch_label: row.get(8)?,
                is_merge: row.get::<_, i32>(9)? != 0,
                is_tagged: row.get::<_, i32>(10)? != 0,
                parents,
                message: row.get(12)?,
                remote_tip_label: row.get(13)?,
                remote_tip_extra_count: row.get::<_, i64>(14)? as usize,
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
