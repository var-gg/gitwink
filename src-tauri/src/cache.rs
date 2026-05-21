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
    // (The `commits` table is a pure cache recreated by the schema-v3
    // block below — its columns all live in that CREATE, no ALTERs here.)

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

    // Schema v3: the `commits` table gains `repo_id` + `sort_ts` for the
    // windowed timeline (keyset pagination over a total-order index).
    // `commits` is a pure cache the scanner refills, so we drop & recreate
    // rather than ALTER — `repos` (tombstones, user state) is untouched.
    let schema_ver: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);
    if schema_ver < 3 {
        conn.execute_batch(
            r#"
            DROP TABLE IF EXISTS commits;
            CREATE TABLE commits (
                repo_path TEXT NOT NULL,
                hash TEXT NOT NULL,
                repo_id INTEGER,
                repo_name TEXT NOT NULL,
                short_hash TEXT NOT NULL,
                summary TEXT NOT NULL,
                author TEXT NOT NULL,
                email TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                sort_ts INTEGER NOT NULL,
                branch_label TEXT,
                is_merge INTEGER NOT NULL DEFAULT 0,
                is_tagged INTEGER NOT NULL DEFAULT 0,
                parents TEXT NOT NULL DEFAULT '[]',
                message TEXT NOT NULL DEFAULT '',
                remote_tip_label TEXT,
                remote_tip_extra_count INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (repo_path, hash)
            );
            -- Total-order index for keyset window queries. Newest-first is
            -- ascending on sort_ts (= -timestamp); repo_id + hash break ties
            -- into a deterministic total order, stable under concurrent
            -- scanner inserts.
            CREATE INDEX idx_commits_timeline ON commits(sort_ts, repo_id, hash);
            -- Retained for the legacy since-filtered list_recent_commits.
            CREATE INDEX idx_commits_ts ON commits(timestamp);
            "#,
        )?;
    }
    let _ = conn.pragma_update(None, "user_version", 3_i64);

    // v0.1.2 cleanup: the v0.1.1 orchestrator used `fs::canonicalize`
    // and stored its `\\?\…` Windows extended-length output as
    // canonical_path. Existing rows from v0.1.0 / migration backfill
    // used the non-prefixed form, so the same repo on disk ended up as
    // two rows with no way for the unique index to dedup them. Strip
    // the prefix here once — idempotent on warm DBs that have no
    // prefixed rows.
    #[cfg(target_os = "windows")]
    dedupe_unc_canonical_paths(&conn);

    Ok(conn)
}

/// Walk every repos row whose canonical_path starts with `\\?\`. If a
/// non-prefixed twin already exists, drop the prefixed row (the twin is
/// the keeper). Otherwise rename in place. Same treatment for
/// repo_sources + path_aliases foreign references. True extended-UNC
/// paths (`\\?\UNC\…`) are skipped — stripping their prefix would
/// merge `\\?\UNC\server\share\foo` into `UNC\server\share\foo`, which
/// is a different path.
#[cfg(target_os = "windows")]
fn dedupe_unc_canonical_paths(conn: &Connection) {
    let prefixed: Vec<String> = match conn.prepare(
        // LIKE pattern `\\?\%` with ESCAPE not needed — `?` isn't a
        // SQL wildcard, `_` is.
        "SELECT canonical_path FROM repos WHERE canonical_path LIKE '\\\\?\\%'",
    ) {
        Ok(mut stmt) => stmt
            .query_map([], |r| r.get::<_, String>(0))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default(),
        Err(_) => return,
    };

    for prefixed_path in prefixed {
        let Some(rest) = prefixed_path.strip_prefix(r"\\?\") else {
            continue;
        };
        // Skip true extended-UNC; stripping would lose the UNC marker.
        if rest.starts_with("UNC\\") {
            continue;
        }
        let unprefixed = rest.to_string();

        // Twin exists → drop the prefixed row + its dangling source/alias rows.
        let twin: bool = conn
            .query_row(
                "SELECT 1 FROM repos WHERE canonical_path = ?1",
                params![&unprefixed],
                |_| Ok(true),
            )
            .unwrap_or(false);

        if twin {
            let _ = conn.execute(
                "DELETE FROM repos WHERE canonical_path = ?1",
                params![&prefixed_path],
            );
            let _ = conn.execute(
                "DELETE FROM repo_sources WHERE repo_canonical_path = ?1",
                params![&prefixed_path],
            );
            let _ = conn.execute(
                "DELETE FROM path_aliases WHERE canonical_path = ?1",
                params![&prefixed_path],
            );
            continue;
        }

        // No twin → rename in place across the three tables.
        let _ = conn.execute(
            "UPDATE repos SET canonical_path = ?1, path = ?1 WHERE canonical_path = ?2",
            params![&unprefixed, &prefixed_path],
        );
        let _ = conn.execute(
            "UPDATE repo_sources SET repo_canonical_path = ?1 WHERE repo_canonical_path = ?2",
            params![&unprefixed, &prefixed_path],
        );
        let _ = conn.execute(
            "UPDATE path_aliases SET canonical_path = ?1 WHERE canonical_path = ?2",
            params![&unprefixed, &prefixed_path],
        );
    }
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
            INSERT INTO commits (repo_path, hash, repo_id, repo_name, short_hash, summary, author, email, timestamp, sort_ts, branch_label, is_merge, is_tagged, parents, message, remote_tip_label, remote_tip_extra_count)
            VALUES (?1, ?2, COALESCE((SELECT rowid FROM repos WHERE path = ?1), 0), ?3, ?4, ?5, ?6, ?7, ?8, -?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            ON CONFLICT(repo_path, hash) DO UPDATE SET
                repo_id = excluded.repo_id,
                sort_ts = excluded.sort_ts,
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

// ----- timeline window queries (Phase 1: windowed-pull) -----

/// A keyset cursor into the timeline's total order. `(sort_ts, repo_id,
/// hash)` is unique across `commits`, so a cursor points unambiguously
/// between two rows and stays valid under concurrent scanner inserts.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cursor {
    pub sort_ts: i64,
    pub repo_id: i64,
    pub hash: String,
}

/// Which way a window query reads from its cursor. `Older` walks toward
/// older commits (down the timeline), `Newer` toward newer (up).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowDirection {
    Older,
    Newer,
}

/// Server-side timeline filters. Every `None` field means "no restriction".
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineFilters {
    /// Restrict to these repo ids. `None` = all repos.
    pub repo_ids: Option<Vec<i64>>,
    /// Restrict to these author names. `None` = all authors.
    pub authors: Option<Vec<String>>,
    /// Only commits at/after this unix-seconds timestamp. `None` = all time.
    pub since: Option<i64>,
}

/// One page of the timeline plus the cursors/flags the UI needs to fetch
/// adjacent pages. `rows` is always newest-first (sort_ts ascending).
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitWindow {
    pub rows: Vec<CommitSummary>,
    pub start_cursor: Option<Cursor>,
    pub end_cursor: Option<Cursor>,
    pub has_newer: bool,
    pub has_older: bool,
}

/// 16 columns: the 15 CommitSummary fields plus `repo_id` for the cursor.
const TIMELINE_COLS: &str = "repo_path, repo_name, hash, short_hash, summary, \
    author, email, timestamp, branch_label, is_merge, is_tagged, parents, \
    message, remote_tip_label, remote_tip_extra_count, repo_id";

/// Map a row selecting `TIMELINE_COLS` to a CommitSummary + its cursor.
fn row_to_window_item(row: &rusqlite::Row) -> rusqlite::Result<(CommitSummary, Cursor)> {
    let parents_json: String = row.get(11)?;
    let parents: Vec<String> = serde_json::from_str(&parents_json).unwrap_or_default();
    let timestamp: i64 = row.get(7)?;
    let repo_id: i64 = row.get(15)?;
    let hash: String = row.get(2)?;
    let summary = CommitSummary {
        repo_path: row.get(0)?,
        repo_name: row.get(1)?,
        hash: hash.clone(),
        short_hash: row.get(3)?,
        summary: row.get(4)?,
        author: row.get(5)?,
        email: row.get(6)?,
        timestamp,
        branch_label: row.get(8)?,
        is_merge: row.get::<_, i32>(9)? != 0,
        is_tagged: row.get::<_, i32>(10)? != 0,
        parents,
        message: row.get(12)?,
        remote_tip_label: row.get(13)?,
        remote_tip_extra_count: row.get::<_, i64>(14)? as usize,
    };
    let cursor = Cursor {
        sort_ts: -timestamp,
        repo_id,
        hash,
    };
    Ok((summary, cursor))
}

/// Build the SQL filter fragment (each piece prefixed with " AND ") plus
/// the bound params it introduces. `repo_ids` go inline as an integer
/// literal — they are our own row ids, never user text, and a huge
/// multi-repo selection would otherwise blow past SQLite's bound-variable
/// limit. `authors` (bounded, small) and `since` use bound params.
fn build_filter_sql(filters: &TimelineFilters) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
    let mut sql = String::new();
    let mut params: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();

    if let Some(since) = filters.since {
        if since > 0 {
            sql.push_str(" AND timestamp >= ?");
            params.push(Box::new(since));
        }
    }
    if let Some(repo_ids) = &filters.repo_ids {
        // An explicit empty selection genuinely means "no repos" — the UI
        // sends `None` for the all-repos case.
        if repo_ids.is_empty() {
            sql.push_str(" AND 0");
        } else {
            let list = repo_ids
                .iter()
                .map(|id| id.to_string())
                .collect::<Vec<_>>()
                .join(",");
            sql.push_str(&format!(" AND repo_id IN ({list})"));
        }
    }
    if let Some(authors) = &filters.authors {
        if authors.is_empty() {
            sql.push_str(" AND 0");
        } else {
            let marks = vec!["?"; authors.len()].join(",");
            sql.push_str(&format!(" AND author IN ({marks})"));
            for a in authors {
                params.push(Box::new(a.clone()));
            }
        }
    }
    (sql, params)
}

/// Fetch one keyset-paginated page of the timeline. `cursor == None` reads
/// from the appropriate end (top for `Older`).
pub fn list_commits_window(
    conn: &Connection,
    filters: &TimelineFilters,
    cursor: Option<&Cursor>,
    direction: WindowDirection,
    limit: usize,
) -> Result<CommitWindow> {
    let (filter_sql, mut bind) = build_filter_sql(filters);

    // Overfetch one row to learn whether a further page exists in the
    // reading direction. Newer pages read DESC and get reversed so the
    // result is always newest-first.
    let keep = limit.saturating_add(1);
    let (order, keyset_op) = match direction {
        WindowDirection::Older => ("ASC", ">"),
        WindowDirection::Newer => ("DESC", "<"),
    };

    let mut sql = format!("SELECT {TIMELINE_COLS} FROM commits WHERE 1=1{filter_sql}");
    if cursor.is_some() {
        sql.push_str(&format!(" AND (sort_ts, repo_id, hash) {keyset_op} (?, ?, ?)"));
    }
    sql.push_str(&format!(
        " ORDER BY sort_ts {order}, repo_id {order}, hash {order} LIMIT ?"
    ));

    if let Some(c) = cursor {
        bind.push(Box::new(c.sort_ts));
        bind.push(Box::new(c.repo_id));
        bind.push(Box::new(c.hash.clone()));
    }
    bind.push(Box::new(keep as i64));

    let mut stmt = conn.prepare(&sql)?;
    let mut items: Vec<(CommitSummary, Cursor)> = stmt
        .query_map(rusqlite::params_from_iter(bind.iter()), row_to_window_item)?
        .collect::<Result<Vec<_>, _>>()?;

    // Newer pages came back DESC — flip to newest-first.
    if direction == WindowDirection::Newer {
        items.reverse();
    }

    // Trim the overfetched row, if any. After the Newer reverse it sits at
    // the front; for Older it sits at the back.
    let has_more = items.len() > limit;
    if has_more {
        match direction {
            WindowDirection::Older => items.truncate(limit),
            WindowDirection::Newer => {
                items.remove(0);
            }
        }
    }

    let start_cursor = items.first().map(|(_, c)| c.clone());
    let end_cursor = items.last().map(|(_, c)| c.clone());
    // A non-null cursor means we paged off a known row, so the opposite
    // direction is populated; the reading direction's flag is `has_more`.
    let (has_newer, has_older) = match direction {
        WindowDirection::Older => (cursor.is_some(), has_more),
        WindowDirection::Newer => (has_more, cursor.is_some()),
    };

    Ok(CommitWindow {
        rows: items.into_iter().map(|(s, _)| s).collect(),
        start_cursor,
        end_cursor,
        has_newer,
        has_older,
    })
}

/// Total commit count under `filters` — drives the virtual scrollbar size.
pub fn count_commits(conn: &Connection, filters: &TimelineFilters) -> Result<i64> {
    let (filter_sql, bind) = build_filter_sql(filters);
    let sql = format!("SELECT COUNT(*) FROM commits WHERE 1=1{filter_sql}");
    let n: i64 = conn.query_row(&sql, rusqlite::params_from_iter(bind.iter()), |r| r.get(0))?;
    Ok(n)
}

/// Rows centred on an anchor cursor — `before` rows newer + the anchor row
/// (when a commit sits exactly there and passes the filter) + `after` rows
/// older. Restores the viewport after a filter change, where the previous
/// anchor commit may or may not survive the new filter.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitAround {
    pub rows: Vec<CommitSummary>,
    pub anchor_found: bool,
    pub start_cursor: Option<Cursor>,
    pub end_cursor: Option<Cursor>,
    pub has_newer: bool,
    pub has_older: bool,
}

pub fn list_commits_around_anchor(
    conn: &Connection,
    filters: &TimelineFilters,
    anchor: &Cursor,
    before: usize,
    after: usize,
) -> Result<CommitAround> {
    let newer = list_commits_window(conn, filters, Some(anchor), WindowDirection::Newer, before)?;
    let older = list_commits_window(conn, filters, Some(anchor), WindowDirection::Older, after)?;

    // The window queries are strict (`<` / `>`), so the anchor row itself
    // lands in neither — fetch it directly to learn whether it survives the
    // filter and to place it between the two halves.
    let (filter_sql, filter_bind) = build_filter_sql(filters);
    let anchor_sql = format!(
        "SELECT {TIMELINE_COLS} FROM commits \
         WHERE sort_ts = ? AND repo_id = ? AND hash = ?{filter_sql}"
    );
    let mut anchor_bind: Vec<Box<dyn rusqlite::ToSql>> = vec![
        Box::new(anchor.sort_ts),
        Box::new(anchor.repo_id),
        Box::new(anchor.hash.clone()),
    ];
    anchor_bind.extend(filter_bind);
    let anchor_row = match conn.query_row(
        &anchor_sql,
        rusqlite::params_from_iter(anchor_bind.iter()),
        row_to_window_item,
    ) {
        Ok(item) => Some(item),
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(e) => return Err(e.into()),
    };
    let anchor_found = anchor_row.is_some();

    let mut rows: Vec<CommitSummary> =
        Vec::with_capacity(newer.rows.len() + older.rows.len() + 1);
    rows.extend(newer.rows);
    if let Some((summary, _)) = anchor_row {
        rows.push(summary);
    }
    rows.extend(older.rows);

    let start_cursor = newer
        .start_cursor
        .or(if anchor_found { Some(anchor.clone()) } else { None })
        .or(older.start_cursor);
    let end_cursor = older
        .end_cursor
        .or(if anchor_found { Some(anchor.clone()) } else { None })
        .or(newer.end_cursor);

    Ok(CommitAround {
        rows,
        anchor_found,
        start_cursor,
        end_cursor,
        has_newer: newer.has_newer,
        has_older: older.has_older,
    })
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
