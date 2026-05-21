use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::git::CommitSummary;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Repo {
    /// `repos.rowid` — the integer the `commits` cache stores as `repo_id`.
    /// The windowed timeline filters by these ids (`TimelineFilters`). 0 for
    /// a `Repo` value that did not come from `list_repos` (the legacy
    /// `discover_repos` upsert path, whose values never reach the UI).
    #[serde(default)]
    pub id: i64,
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

/// DDL for the `commits` table and its indexes (schema v4). Shared by the
/// `open()` migration and the test fixtures so the two never drift.
///
/// `repo_id`, `sort_ts` and `first_seen_generation` are `NOT NULL DEFAULT 0`
/// so the table tolerates writes from older builds during the rollout — the
/// shared cache.db is also opened by pre-v4 releases, which omit those
/// columns rather than hitting a constraint error. v4's own `upsert_commits`
/// always supplies real values.
///
/// `first_seen_generation` records the generation counter (see
/// `next_generation`) in effect when the row was first inserted, and is
/// never rewritten by a later conflict-update. A reader pinned to generation
/// N filters `first_seen_generation <= N` for an MVCC-lite snapshot the
/// background scanner's subsequent inserts cannot disturb.
const COMMITS_SCHEMA_V4: &str = r#"
CREATE TABLE commits (
    repo_path TEXT NOT NULL,
    hash TEXT NOT NULL,
    repo_id INTEGER NOT NULL DEFAULT 0,
    repo_name TEXT NOT NULL,
    short_hash TEXT NOT NULL,
    summary TEXT NOT NULL,
    author TEXT NOT NULL,
    email TEXT NOT NULL,
    timestamp INTEGER NOT NULL,
    sort_ts INTEGER NOT NULL DEFAULT 0,
    branch_label TEXT,
    is_merge INTEGER NOT NULL DEFAULT 0,
    is_tagged INTEGER NOT NULL DEFAULT 0,
    parents TEXT NOT NULL DEFAULT '[]',
    message TEXT NOT NULL DEFAULT '',
    remote_tip_label TEXT,
    remote_tip_extra_count INTEGER NOT NULL DEFAULT 0,
    first_seen_generation INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (repo_path, hash)
);
-- Total-order index for keyset window queries. Newest-first = ascending on
-- sort_ts (= -timestamp); repo_id + hash break ties into a deterministic
-- total order, stable under concurrent scanner inserts.
CREATE INDEX idx_commits_timeline ON commits(sort_ts, repo_id, hash);
-- Retained for the legacy since-filtered list_recent_commits.
CREATE INDEX idx_commits_ts ON commits(timestamp);
"#;

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
    // The cache is opened on separate connections from several threads — the
    // file watcher, command spawn_blocking tasks, the discovery orchestrator.
    // Without a busy timeout a concurrent writer fails immediately with
    // SQLITE_BUSY; 5s lets the short upsert transactions queue instead.
    // Phase 2's per-batch generation bump makes every `upsert_commits` a
    // (slightly longer) write transaction, so this keeps that contention
    // invisible.
    let _ = conn.busy_timeout(Duration::from_secs(5));
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

    // Schema v3 gave `commits` `repo_id` + `sort_ts` for the windowed
    // timeline; schema v4 adds `first_seen_generation` for the
    // generation/MVCC-lite snapshot model. `commits` is a pure cache the
    // scanner refills, so each step drops & recreates it rather than
    // ALTERing — `repos` (tombstones, user state) and `meta` (which holds
    // the generation counter itself) are untouched.
    let schema_ver: i64 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap_or(0);
    if schema_ver < 4 {
        conn.execute_batch("DROP TABLE IF EXISTS commits;")?;
        conn.execute_batch(COMMITS_SCHEMA_V4)?;
    }
    let _ = conn.pragma_update(None, "user_version", 4_i64);

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
        SELECT rowid, path, name, status
        FROM repos
        WHERE user_state != 'removed'
        ORDER BY name COLLATE NOCASE
        "#,
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(Repo {
                id: row.get(0)?,
                path: row.get(1)?,
                name: row.get(2)?,
                status: row.get(3)?,
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

// ----- commit generation (Phase 2: MVCC-lite snapshot) -----

/// `meta` key holding the monotonic commit-generation counter.
const GENERATION_KEY: &str = "commit_generation";

/// Atomically bump the commit generation and return the new value.
///
/// The generation is a counter the scanner advances exactly once per write
/// batch (see `upsert_commits`); every commit a batch newly inserts is
/// stamped with the generation then in effect. A reader that pins
/// `view_generation = N` then sees a stable snapshot — later batches insert
/// at a generation > N and stay invisible until the reader re-pins.
///
/// MUST be called as the first statement of the same transaction that
/// writes the batch's commits, so the bump and the rows it stamps commit
/// together: a reader can never observe a half-applied batch, and two
/// concurrent batches can never share a generation.
fn next_generation(conn: &Connection) -> Result<i64> {
    conn.execute(
        r#"
        INSERT INTO meta (key, value, updated_at) VALUES (?1, '1', ?2)
        ON CONFLICT(key) DO UPDATE SET
            value = CAST(value AS INTEGER) + 1,
            updated_at = excluded.updated_at
        "#,
        params![GENERATION_KEY, unix_now()],
    )?;
    let generation: i64 = conn.query_row(
        "SELECT CAST(value AS INTEGER) FROM meta WHERE key = ?1",
        params![GENERATION_KEY],
        |r| r.get(0),
    )?;
    Ok(generation)
}

/// The current commit generation — what a fresh reader should pin as its
/// `view_generation`. 0 before the scanner has written any batch.
pub fn current_generation(conn: &Connection) -> Result<i64> {
    let raw = meta_get(conn, GENERATION_KEY)?;
    Ok(raw.and_then(|s| s.parse().ok()).unwrap_or(0))
}

// ----- commits -----

/// What a `upsert_commits` batch did. `generation` is the new generation the
/// batch's freshly-inserted commits were stamped with; `inserted` counts only
/// genuinely-new commits — a conflict-update of an already-cached commit is
/// not counted, and does not change that commit's `first_seen_generation`.
/// Callers that emit a `timeline://invalidated` event forward both fields;
/// callers that only need the rows persisted ignore the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpsertOutcome {
    pub generation: i64,
    pub inserted: usize,
}

/// Lightweight scanner→UI invalidation signal, emitted as the
/// `timeline://invalidated` event. The windowed-pull model never pushes
/// commit arrays to the frontend: the scanner writes to the cache and only
/// announces "generation N landed, touching `repo_path`, +`inserted` new
/// commits". The UI then re-pulls whatever windows it currently shows. The
/// payload is tiny, so it stays cheap even when a discovery sweep emits one
/// per repo. camelCase to match the other event payloads.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TimelineInvalidated {
    pub generation: i64,
    pub inserted: usize,
    pub repo_path: String,
}

/// Insert or update a batch of commits, advancing the commit generation by
/// one. Freshly-inserted rows are stamped with the new generation;
/// already-cached rows keep their original `first_seen_generation` (only
/// their mutable fields — branch labels, remote tips — are refreshed).
/// Returns the new generation and the count of genuinely-new commits. An
/// empty batch is a no-op and returns the `generation: 0` sentinel.
pub fn upsert_commits(conn: &mut Connection, commits: &[CommitSummary]) -> Result<UpsertOutcome> {
    if commits.is_empty() {
        return Ok(UpsertOutcome {
            generation: 0,
            inserted: 0,
        });
    }
    let tx = conn.transaction()?;
    // One generation per batch — bumped first, before any row is written, so
    // the whole batch (counter + rows) commits atomically.
    let generation = next_generation(&tx)?;
    let mut inserted = 0usize;
    {
        let mut stmt = tx.prepare(
            r#"
            INSERT INTO commits (repo_path, hash, repo_id, repo_name, short_hash, summary, author, email, timestamp, sort_ts, branch_label, is_merge, is_tagged, parents, message, remote_tip_label, remote_tip_extra_count, first_seen_generation)
            VALUES (?1, ?2, COALESCE((SELECT rowid FROM repos WHERE path = ?1), 0), ?3, ?4, ?5, ?6, ?7, ?8, -?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
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
            RETURNING first_seen_generation
            "#,
        )?;
        for c in commits {
            let parents_json = serde_json::to_string(&c.parents).unwrap_or_else(|_| "[]".into());
            // RETURNING yields the row's `first_seen_generation` after the
            // statement. A fresh insert stamped it with `generation`; a
            // conflict-update left the (necessarily older) original value in
            // place. Since `generation` is strictly greater than every prior
            // generation, a match means "this row is new".
            let row_generation: i64 = stmt.query_row(
                params![
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
                    generation,
                ],
                |r| r.get(0),
            )?;
            if row_generation == generation {
                inserted += 1;
            }
        }
    }
    tx.commit()?;
    Ok(UpsertOutcome {
        generation,
        inserted,
    })
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
    /// MVCC-lite snapshot pin. When `Some(n)`, only commits whose
    /// `first_seen_generation <= n` are visible — commits the background
    /// scanner inserted after the UI pinned generation `n` stay hidden
    /// until it re-pins. `None` = no pin, every commit visible (the
    /// pre-Phase-2 behaviour the legacy non-windowed paths still rely on).
    pub view_generation: Option<i64>,
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
/// limit. `authors` (bounded, small), `since` and `view_generation` use
/// bound params.
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
    // MVCC-lite snapshot pin: hide commits first seen after the caller's
    // pinned generation. `first_seen_generation` is a residual predicate on
    // the `idx_commits_timeline` scan — the keyset order is unaffected, so
    // pagination over the pinned subset stays gap- and dupe-free.
    if let Some(view_generation) = filters.view_generation {
        sql.push_str(" AND first_seen_generation <= ?");
        params.push(Box::new(view_generation));
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

/// One author's commit tally under the active filters — the AuthorsChip
/// facet. Mirrors the frontend `AuthorTally`.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthorTally {
    pub name: String,
    pub count: i64,
    pub last_activity: i64,
}

/// Distinct commit authors under `filters`, most-recent activity first.
/// Phase 3's windowed timeline drops the full client-side commit array,
/// so the AuthorsChip can no longer tally authors itself — this is its
/// facet source. Callers pass the time-window (`since`) + generation pin
/// but leave `authors` unset, so the list covers every selectable author.
pub fn list_timeline_authors(
    conn: &Connection,
    filters: &TimelineFilters,
) -> Result<Vec<AuthorTally>> {
    let (filter_sql, bind) = build_filter_sql(filters);
    let sql = format!(
        "SELECT author, COUNT(*) AS cnt, MAX(timestamp) AS last \
         FROM commits WHERE 1=1{filter_sql} \
         GROUP BY author ORDER BY last DESC"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map(rusqlite::params_from_iter(bind.iter()), |r| {
            Ok(AuthorTally {
                name: r.get(0)?,
                count: r.get(1)?,
                last_activity: r.get(2)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh in-memory DB carrying just the v4 `commits` schema.
    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(COMMITS_SCHEMA_V4).unwrap();
        conn
    }

    /// A fresh in-memory DB with the v4 `commits` schema plus the minimal
    /// `repos` + `meta` tables `upsert_commits` touches — the `repos` rowid
    /// lookup and the `meta` generation counter respectively.
    fn upsert_test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(COMMITS_SCHEMA_V4).unwrap();
        conn.execute_batch(
            "CREATE TABLE repos (path TEXT PRIMARY KEY, name TEXT NOT NULL);\
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL, \
             updated_at INTEGER NOT NULL);",
        )
        .unwrap();
        conn
    }

    /// Build a minimal `CommitSummary` for `upsert_commits` tests.
    fn sample_commit(repo_path: &str, hash: &str, ts: i64) -> CommitSummary {
        CommitSummary {
            repo_path: repo_path.to_string(),
            repo_name: "repo".to_string(),
            hash: hash.to_string(),
            short_hash: hash.chars().take(7).collect(),
            summary: "summary".to_string(),
            author: "alice".to_string(),
            email: "alice@example.com".to_string(),
            timestamp: ts,
            branch_label: None,
            is_merge: false,
            is_tagged: false,
            parents: Vec::new(),
            message: "message".to_string(),
            remote_tip_label: None,
            remote_tip_extra_count: 0,
        }
    }

    /// Insert one commit at an explicit `first_seen_generation`. `sort_ts`
    /// is derived from `ts` exactly as `upsert_commits` does (= -timestamp).
    fn insert_gen(
        conn: &Connection,
        repo_id: i64,
        hash: &str,
        ts: i64,
        author: &str,
        generation: i64,
    ) {
        conn.execute(
            "INSERT INTO commits (repo_path, hash, repo_id, repo_name, \
             short_hash, summary, author, email, timestamp, sort_ts, \
             first_seen_generation) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                format!("/repo/{repo_id}"),
                hash,
                repo_id,
                format!("repo{repo_id}"),
                hash,
                "summary",
                author,
                "dev@example.com",
                ts,
                -ts,
                generation,
            ],
        )
        .unwrap();
    }

    /// Insert one commit at generation 0 (the always-visible default).
    fn insert(conn: &Connection, repo_id: i64, hash: &str, ts: i64, author: &str) {
        insert_gen(conn, repo_id, hash, ts, author, 0);
    }

    /// Page through the whole timeline (Older direction) via the cursor
    /// chain, returning hashes newest-first.
    fn walk(conn: &Connection, filters: &TimelineFilters, page: usize) -> Vec<String> {
        let mut out = Vec::new();
        let mut cursor: Option<Cursor> = None;
        loop {
            let w =
                list_commits_window(conn, filters, cursor.as_ref(), WindowDirection::Older, page)
                    .unwrap();
            out.extend(w.rows.iter().map(|c| c.hash.clone()));
            if !w.has_older {
                break;
            }
            cursor = Some(w.end_cursor.expect("has_older implies an end cursor"));
        }
        out
    }

    #[test]
    fn window_paginates_without_gaps_or_dupes() {
        let conn = test_db();
        for i in 0..50 {
            insert(&conn, 1, &format!("h{i:03}"), 1_000 + i, "alice");
        }
        let walked = walk(&conn, &TimelineFilters::default(), 7);
        assert_eq!(walked.len(), 50);
        let unique: std::collections::HashSet<_> = walked.iter().collect();
        assert_eq!(unique.len(), 50, "no duplicates across pages");
        assert_eq!(walked.first().unwrap().as_str(), "h049", "newest first");
        assert_eq!(walked.last().unwrap().as_str(), "h000", "oldest last");
    }

    #[test]
    fn identical_timestamps_keep_a_stable_total_order() {
        let conn = test_db();
        // 30 commits sharing one timestamp across 3 repos — the
        // (sort_ts, repo_id, hash) total order must still paginate cleanly.
        for repo in 1..=3 {
            for i in 0..10 {
                insert(&conn, repo, &format!("r{repo}c{i}"), 5_000, "bob");
            }
        }
        let f = TimelineFilters::default();
        let walked = walk(&conn, &f, 4);
        assert_eq!(walked.len(), 30);
        assert_eq!(
            walked.iter().collect::<std::collections::HashSet<_>>().len(),
            30,
            "identical timestamps still paginate without gaps or dupes"
        );
        assert_eq!(walk(&conn, &f, 9), walked, "ordering is deterministic");
    }

    #[test]
    fn count_and_filters() {
        let conn = test_db();
        for i in 0..20 {
            insert(&conn, 1, &format!("a{i:02}"), 1_000 + i, "alice");
        }
        for i in 0..15 {
            insert(&conn, 2, &format!("b{i:02}"), 1_000 + i, "bob");
        }
        assert_eq!(count_commits(&conn, &TimelineFilters::default()).unwrap(), 35);

        let repo2 = TimelineFilters {
            repo_ids: Some(vec![2]),
            ..Default::default()
        };
        assert_eq!(count_commits(&conn, &repo2).unwrap(), 15);
        assert_eq!(walk(&conn, &repo2, 6).len(), 15);

        let alice = TimelineFilters {
            authors: Some(vec!["alice".to_string()]),
            ..Default::default()
        };
        assert_eq!(count_commits(&conn, &alice).unwrap(), 20);

        // An explicit empty repo selection genuinely means "nothing".
        let none = TimelineFilters {
            repo_ids: Some(vec![]),
            ..Default::default()
        };
        assert_eq!(count_commits(&conn, &none).unwrap(), 0);
    }

    #[test]
    fn around_anchor_centres_on_a_present_anchor() {
        let conn = test_db();
        for i in 0..40 {
            insert(&conn, 1, &format!("c{i:02}"), 2_000 + i, "alice");
        }
        let anchor = Cursor {
            sort_ts: -2_020,
            repo_id: 1,
            hash: "c20".to_string(),
        };
        let around =
            list_commits_around_anchor(&conn, &TimelineFilters::default(), &anchor, 5, 5).unwrap();
        assert!(around.anchor_found);
        assert_eq!(around.rows.len(), 11, "5 newer + anchor + 5 older");
        assert_eq!(around.rows[0].hash.as_str(), "c25");
        assert_eq!(around.rows[5].hash.as_str(), "c20", "anchor in the middle");
        assert_eq!(around.rows[10].hash.as_str(), "c15");
    }

    #[test]
    fn around_anchor_handles_a_missing_anchor() {
        let conn = test_db();
        for i in 0..20 {
            insert(&conn, 1, &format!("d{i:02}"), 3_000 + i * 10, "alice");
        }
        // A cursor at a timestamp where no commit sits (between d09 and
        // d10) is still a valid position in the total order.
        let anchor = Cursor {
            sort_ts: -3_095,
            repo_id: 1,
            hash: "zzz".to_string(),
        };
        let around =
            list_commits_around_anchor(&conn, &TimelineFilters::default(), &anchor, 3, 3).unwrap();
        assert!(!around.anchor_found);
        assert_eq!(around.rows.len(), 6, "3 newer + 3 older, no anchor row");
    }

    #[test]
    fn generation_counter_is_monotonic() {
        let mut conn = upsert_test_db();
        assert_eq!(current_generation(&conn).unwrap(), 0, "0 before any batch");

        // upsert_commits bumps the generation exactly once per batch.
        let o1 = upsert_commits(&mut conn, &[sample_commit("/r/a", "h1", 100)]).unwrap();
        assert_eq!(o1.generation, 1);
        let o2 = upsert_commits(&mut conn, &[sample_commit("/r/a", "h2", 200)]).unwrap();
        assert_eq!(o2.generation, 2);
        assert_eq!(current_generation(&conn).unwrap(), 2);

        // An empty batch is a no-op: no bump, `generation: 0` sentinel.
        let o3 = upsert_commits(&mut conn, &[]).unwrap();
        assert_eq!(o3, UpsertOutcome { generation: 0, inserted: 0 });
        assert_eq!(
            current_generation(&conn).unwrap(),
            2,
            "empty batch did not bump"
        );
    }

    #[test]
    fn upsert_stamps_generation_and_counts_only_new_rows() {
        let mut conn = upsert_test_db();

        // Batch 1: three brand-new commits.
        let batch1 = [
            sample_commit("/r/a", "h1", 100),
            sample_commit("/r/a", "h2", 200),
            sample_commit("/r/a", "h3", 300),
        ];
        let o1 = upsert_commits(&mut conn, &batch1).unwrap();
        assert_eq!(
            o1,
            UpsertOutcome {
                generation: 1,
                inserted: 3
            }
        );

        // Batch 2: one repeat (h3) + one new (h4) — only h4 counts as inserted.
        let batch2 = [
            sample_commit("/r/a", "h3", 300),
            sample_commit("/r/a", "h4", 400),
        ];
        let o2 = upsert_commits(&mut conn, &batch2).unwrap();
        assert_eq!(
            o2,
            UpsertOutcome {
                generation: 2,
                inserted: 1
            }
        );

        // h3 keeps its original generation — it was *first* seen in batch 1,
        // so batch 2's conflict-update must not rewrite first_seen_generation.
        let h3_gen: i64 = conn
            .query_row(
                "SELECT first_seen_generation FROM commits WHERE hash = 'h3'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(h3_gen, 1, "first_seen_generation survives a re-upsert");

        // h4 is stamped with batch 2's generation.
        let h4_gen: i64 = conn
            .query_row(
                "SELECT first_seen_generation FROM commits WHERE hash = 'h4'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(h4_gen, 2);
    }

    #[test]
    fn window_view_generation_hides_later_batches() {
        let conn = test_db();
        // Generation 1: 10 commits at the even timestamps.
        for i in 0..10 {
            insert_gen(&conn, 1, &format!("g1c{i:02}"), 1_000 + i * 2, "alice", 1);
        }
        // Generation 2: 10 commits interleaved at the odd timestamps between
        // them — the scanner inserting into the middle of the timeline.
        for i in 0..10 {
            insert_gen(&conn, 1, &format!("g2c{i:02}"), 1_001 + i * 2, "alice", 2);
        }

        // A reader pinned to generation 1 sees only the generation-1 commits.
        let pinned = TimelineFilters {
            view_generation: Some(1),
            ..Default::default()
        };
        let walked = walk(&conn, &pinned, 4);
        assert_eq!(walked.len(), 10, "generation-2 inserts stay invisible");
        assert!(
            walked.iter().all(|h| h.starts_with("g1c")),
            "only generation-1 commits, none from the later batch"
        );
        assert_eq!(count_commits(&conn, &pinned).unwrap(), 10);

        // No pin sees every commit.
        let all = TimelineFilters::default();
        assert_eq!(walk(&conn, &all, 4).len(), 20);
        assert_eq!(count_commits(&conn, &all).unwrap(), 20);

        // Pinned to generation 2 also sees everything (1 <= 2 and 2 <= 2).
        let pinned2 = TimelineFilters {
            view_generation: Some(2),
            ..Default::default()
        };
        assert_eq!(walk(&conn, &pinned2, 4).len(), 20);
    }

    #[test]
    fn pinned_window_paginates_cleanly_while_newer_batches_arrive() {
        let conn = test_db();
        // The snapshot the reader pins: 40 commits at generation 1.
        for i in 0..40 {
            insert_gen(&conn, 1, &format!("base{i:02}"), 5_000 + i, "alice", 1);
        }
        let pinned = TimelineFilters {
            view_generation: Some(1),
            ..Default::default()
        };

        // Walk the snapshot a small page at a time; between every page a
        // concurrent "scanner" batch lands at a later generation, interleaved
        // into the same timestamp range. The pinned walk must be unaffected.
        let mut out: Vec<String> = Vec::new();
        let mut cursor: Option<Cursor> = None;
        let mut intruder = 0;
        loop {
            let w =
                list_commits_window(&conn, &pinned, cursor.as_ref(), WindowDirection::Older, 6)
                    .unwrap();
            out.extend(w.rows.iter().map(|c| c.hash.clone()));
            insert_gen(&conn, 1, &format!("new{intruder:02}"), 5_000 + intruder, "bob", 2);
            intruder += 1;
            if !w.has_older {
                break;
            }
            cursor = Some(w.end_cursor.expect("has_older implies an end cursor"));
        }
        assert_eq!(out.len(), 40, "exactly the pinned snapshot, no extra rows");
        let unique: std::collections::HashSet<_> = out.iter().collect();
        assert_eq!(unique.len(), 40, "no duplicates despite concurrent inserts");
        assert!(out.iter().all(|h| h.starts_with("base")));
    }
}
