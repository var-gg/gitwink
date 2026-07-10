//! Schema owned by the progressively-complete history index.
//!
//! The regular timeline cache and the history index intentionally share the
//! `commits` table: a commit has one rowid, one generation and one lifecycle.
//! This module only adds coverage metadata and a content index around that
//! single source of truth.

use anyhow::Result;
use rusqlite::{Connection, Transaction};

pub(crate) const HISTORY_SCHEMA_VERSION: i64 = 12;

fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let name: String = row.get(1)?;
        if name == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn table_exists(conn: &Connection, table: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        [table],
        |row| row.get::<_, i64>(0),
    )? != 0)
}

/// Upgrade a v5/v6 database to the current history-index schema. Re-running
/// it for v6 intentionally recreates only the lazy text index so an interim
/// unicode61 development database cannot be queried with trigram semantics.
///
/// This is called only from cache.rs's `user_version`-gated IMMEDIATE
/// migration transaction. Legacy rows are deliberately not indexed here:
/// `history_store::backfill_fts_batch` seeds them lazily in bounded writes,
/// and a full history scan indexes each row it touches. There is no O(history)
/// `rebuild`, `optimize` or SELECT-at-startup path.
pub(crate) fn migrate(tx: &Transaction<'_>) -> Result<()> {
    // Lifecycle ABA fencing is correctness-critical for slow Git read ->
    // cache-write workflows. Unlike older best-effort ALTERs this lives in
    // the serialized, versioned transaction and propagates any failure.
    if !has_column(tx, "repos", "lifecycle_epoch")? {
        tx.execute(
            "ALTER TABLE repos ADD COLUMN lifecycle_epoch INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    // Stable repository identity is captured by read-only discovery/exclusion
    // code, never by migration. NULL means an older row has not yet been
    // observed with the new identity probe.
    if !has_column(tx, "repos", "repo_storage_identity")? {
        tx.execute(
            "ALTER TABLE repos ADD COLUMN repo_storage_identity TEXT",
            [],
        )?;
    }
    if !has_column(tx, "commits", "history_scan_epoch")? {
        tx.execute(
            "ALTER TABLE commits ADD COLUMN history_scan_epoch INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if !has_column(tx, "commits", "message_truncated")? {
        tx.execute(
            "ALTER TABLE commits ADD COLUMN message_truncated INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }

    let legacy_cleanup = table_exists(tx, "history_cleanup_jobs")?
        && !has_column(tx, "history_cleanup_jobs", "cleanup_phase")?;
    if legacy_cleanup {
        tx.execute_batch(
            "DROP INDEX IF EXISTS idx_history_cleanup_jobs_repo; \
             ALTER TABLE history_cleanup_jobs RENAME TO history_cleanup_jobs_v11;",
        )?;
    }

    tx.execute_batch(
        r#"
        CREATE INDEX IF NOT EXISTS idx_commits_repo_epoch
            ON commits(repo_path, history_scan_epoch);
        CREATE INDEX IF NOT EXISTS idx_commits_repo_timeline
            ON commits(repo_path, sort_ts, hash);
        CREATE INDEX IF NOT EXISTS idx_commits_author_timeline
            ON commits(author, sort_ts, repo_path, hash);

        CREATE TABLE IF NOT EXISTS history_coverage (
            repo_path TEXT PRIMARY KEY,
            state TEXT NOT NULL DEFAULT 'stale'
                CHECK(state IN ('stale', 'indexing', 'complete', 'cancelled', 'failed')),
            scan_epoch INTEGER NOT NULL DEFAULT 0,
            published_epoch INTEGER NOT NULL DEFAULT 0,
            target_lifecycle_epoch INTEGER,
            target_root_fingerprint TEXT,
            indexed_root_fingerprint TEXT,
            commits_indexed INTEGER NOT NULL DEFAULT 0,
            batches_indexed INTEGER NOT NULL DEFAULT 0,
            completed_generation INTEGER,
            started_at INTEGER,
            completed_at INTEGER,
            updated_at INTEGER NOT NULL,
            last_error TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_history_coverage_state
            ON history_coverage(state, updated_at);

        -- Logical publication is intentionally short; potentially large FTS
        -- and row reclamation runs afterwards in bounded transactions. Keep a
        -- durable, idempotent work record so a crash or transient SQL error
        -- can never turn that deferred cleanup into permanent cache bloat.
        CREATE TABLE IF NOT EXISTS history_cleanup_jobs (
            repo_path TEXT NOT NULL,
            cleanup_kind TEXT NOT NULL
                CHECK(cleanup_kind IN ('full', 'delta', 'discard', 'purge')),
            published_epoch INTEGER NOT NULL,
            staging_epoch INTEGER NOT NULL,
            publication_generation INTEGER NOT NULL,
            cleanup_phase INTEGER NOT NULL DEFAULT 0
                CHECK(cleanup_phase BETWEEN 0 AND 2),
            created_at INTEGER NOT NULL,
            PRIMARY KEY(repo_path, cleanup_kind, published_epoch, staging_epoch)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS idx_history_cleanup_jobs_repo
            ON history_cleanup_jobs(repo_path, created_at);

        -- Exact root set that produced the last COMPLETE coverage row. It is
        -- separate from the compact fingerprint so destructive finalization
        -- can be based on equality of the full snapshot, never a hash alone.
        CREATE TABLE IF NOT EXISTS history_roots (
            repo_path TEXT NOT NULL,
            ref_name TEXT NOT NULL,
            oid TEXT NOT NULL,
            root_kind TEXT NOT NULL,
            PRIMARY KEY(repo_path, ref_name)
        );
        CREATE INDEX IF NOT EXISTS idx_history_roots_repo
            ON history_roots(repo_path);

        -- Reachability membership is versioned independently of commit
        -- metadata. While epoch N+1 is building, readers stay joined to the
        -- published epoch N; force-push/delete rebuilds therefore never leak
        -- a mixed old/new graph as current truth.
        CREATE TABLE IF NOT EXISTS history_membership (
            repo_path TEXT NOT NULL,
            scan_epoch INTEGER NOT NULL,
            hash TEXT NOT NULL,
            PRIMARY KEY(repo_path, scan_epoch, hash)
        ) WITHOUT ROWID;
        CREATE INDEX IF NOT EXISTS idx_history_membership_lookup
            ON history_membership(repo_path, hash, scan_epoch);

        -- External-content FTS cannot reveal whether a rowid has actually
        -- been tokenized (plain SELECT reads through to `commits`). Keep a
        -- tiny explicit ledger so lazy backfill and triggers are idempotent.
        CREATE TABLE IF NOT EXISTS history_fts_indexed (
            rowid INTEGER PRIMARY KEY
        );
        CREATE TABLE IF NOT EXISTS history_fts_state (
            singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
            complete INTEGER NOT NULL DEFAULT 0 CHECK(complete IN (0, 1))
        );

        CREATE TRIGGER IF NOT EXISTS commits_history_membership_ad
        AFTER DELETE ON commits BEGIN
            DELETE FROM history_membership
            WHERE repo_path = old.repo_path AND hash = old.hash;
        END;

        DROP TRIGGER IF EXISTS commits_fts_ai;
        DROP TRIGGER IF EXISTS commits_fts_ad;
        DROP TRIGGER IF EXISTS commits_fts_au;
        DROP TRIGGER IF EXISTS commits_fts_repo_au;
        DROP TABLE IF EXISTS commits_fts;
        DELETE FROM history_fts_indexed;
        INSERT INTO history_fts_state(singleton, complete) VALUES (1, 0)
        ON CONFLICT(singleton) DO UPDATE SET complete = 0;

        -- External-content FTS keeps one copy of immutable commit text. The
        -- canonical values and row lifetime remain owned by `commits`.
        CREATE VIRTUAL TABLE commits_fts USING fts5(
            summary,
            message,
            author,
            email,
            repo_name,
            hash,
            content='commits',
            content_rowid='rowid',
            tokenize='trigram',
            detail='none',
            columnsize=0
        );

        CREATE TRIGGER commits_fts_ai AFTER INSERT ON commits BEGIN
            INSERT INTO commits_fts(
                rowid, summary, message, author, email, repo_name, hash
            ) VALUES (
                new.rowid, new.summary, new.message, new.author,
                new.email, new.repo_name, new.hash
            );
            INSERT INTO history_fts_indexed(rowid)
            SELECT new.rowid
            WHERE NOT EXISTS (
                SELECT 1 FROM history_fts_indexed WHERE rowid = new.rowid
            );
        END;

        CREATE TRIGGER commits_fts_ad AFTER DELETE ON commits BEGIN
            INSERT INTO commits_fts(
                commits_fts, rowid, summary, message, author, email,
                repo_name, hash
            ) SELECT
                'delete', old.rowid, old.summary, old.message, old.author,
                old.email, old.repo_name, old.hash
            WHERE EXISTS (
                SELECT 1 FROM history_fts_indexed WHERE rowid = old.rowid
            );
            DELETE FROM history_fts_indexed WHERE rowid = old.rowid;
        END;

        -- UPDATE OF prevents epoch/generation-only writes from touching the
        -- text index. The WHEN guard also avoids churn when an ordinary
        -- cache upsert repeats byte-identical metadata.
        CREATE TRIGGER commits_fts_au
        AFTER UPDATE OF summary, message, author, email, repo_name, hash ON commits
        WHEN old.summary IS NOT new.summary
          OR old.message IS NOT new.message
          OR old.author IS NOT new.author
          OR old.email IS NOT new.email
          OR old.repo_name IS NOT new.repo_name
          OR old.hash IS NOT new.hash
        BEGIN
            INSERT INTO commits_fts(
                commits_fts, rowid, summary, message, author, email,
                repo_name, hash
            ) SELECT
                'delete', old.rowid, old.summary, old.message, old.author,
                old.email, old.repo_name, old.hash
            WHERE EXISTS (
                SELECT 1 FROM history_fts_indexed WHERE rowid = old.rowid
            );
            INSERT INTO commits_fts(
                rowid, summary, message, author, email, repo_name, hash
            ) VALUES (
                new.rowid, new.summary, new.message, new.author,
                new.email, new.repo_name, new.hash
            );
            INSERT INTO history_fts_indexed(rowid)
            SELECT new.rowid
            WHERE NOT EXISTS (
                SELECT 1 FROM history_fts_indexed WHERE rowid = new.rowid
            );
        END;

        -- A legacy row can be registered before its repo_id is known. Once
        -- discovery attaches it to the catalog, make that row searchable even
        -- if every text byte stayed identical and the ordinary text UPDATE
        -- trigger therefore did not run.
        CREATE TRIGGER commits_fts_repo_au
        AFTER UPDATE OF repo_id ON commits
        WHEN old.repo_id IS NOT new.repo_id
          AND old.summary IS new.summary
          AND old.message IS new.message
          AND old.author IS new.author
          AND old.email IS new.email
          AND old.repo_name IS new.repo_name
          AND old.hash IS new.hash
          AND NOT EXISTS (
                SELECT 1 FROM history_fts_indexed WHERE rowid = new.rowid
          )
        BEGIN
            INSERT INTO commits_fts(
                rowid, summary, message, author, email, repo_name, hash
            ) VALUES (
                new.rowid, new.summary, new.message, new.author,
                new.email, new.repo_name, new.hash
            );
            INSERT OR IGNORE INTO history_fts_indexed(rowid) VALUES (new.rowid);
        END;

        -- No legacy-row SELECT here. Epoch-0 membership and FTS are both
        -- populated by the same bounded lazy backfill path, so migration/open
        -- remains independent of commit count.
        "#,
    )?;
    if legacy_cleanup {
        tx.execute_batch(
            "INSERT OR IGNORE INTO history_cleanup_jobs ( \
                 repo_path, cleanup_kind, published_epoch, staging_epoch, \
                 publication_generation, cleanup_phase, created_at) \
             SELECT repo_path, cleanup_kind, published_epoch, staging_epoch, \
                    publication_generation, \
                    CASE cleanup_kind WHEN 'full' THEN 0 ELSE 1 END, created_at \
             FROM history_cleanup_jobs_v11; \
             DROP TABLE history_cleanup_jobs_v11;",
        )?;
    }
    // Early v6 development databases predate lifecycle fencing on coverage.
    // CREATE TABLE IF NOT EXISTS cannot evolve that existing shape, so keep
    // the additive edge explicit and version-gated. Without this column every
    // Search/All-time status read fails before the worker can publish.
    if !has_column(tx, "history_coverage", "target_lifecycle_epoch")? {
        tx.execute(
            "ALTER TABLE history_coverage ADD COLUMN target_lifecycle_epoch INTEGER",
            [],
        )?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    /// Small v5-compatible database used by history module tests. It mirrors
    /// every column touched by the production v6 migration and batch writer.
    pub(crate) fn legacy_v5_connection() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE repos (
                path TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                discovered_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL,
                canonical_path TEXT,
                status TEXT NOT NULL DEFAULT 'active',
                user_state TEXT NOT NULL DEFAULT 'normal'
            );
            CREATE TABLE meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE discovery_tombstones (
                canonical_path TEXT PRIMARY KEY,
                removed_at INTEGER NOT NULL,
                reason TEXT NOT NULL,
                last_known_name TEXT,
                last_known_source TEXT
            );
            CREATE TABLE path_aliases (
                observed_path TEXT NOT NULL,
                canonical_path TEXT NOT NULL,
                source TEXT NOT NULL,
                first_seen_at INTEGER NOT NULL,
                last_seen_at INTEGER NOT NULL,
                PRIMARY KEY(observed_path, source)
            );
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
            PRAGMA user_version = 5;
            "#,
        )
        .unwrap();
        conn
    }

    pub(crate) fn migrate_to_current(conn: &mut Connection) {
        {
            let tx = conn.transaction().unwrap();
            migrate(&tx).unwrap();
            tx.pragma_update(None, "user_version", HISTORY_SCHEMA_VERSION)
                .unwrap();
            tx.commit().unwrap();
        }
    }

    pub(crate) fn v5_connection() -> Connection {
        let mut conn = legacy_v5_connection();
        migrate_to_current(&mut conn);
        conn
    }

    pub(crate) fn insert_commit(conn: &Connection, hash: &str, summary: &str, message: &str) {
        conn.execute(
            r#"
            INSERT OR IGNORE INTO repos(
                path, name, discovered_at, last_seen_at, canonical_path,
                status, user_state
            ) VALUES ('/repo', 'repo', 1, 1, '/repo', 'active', 'normal')
            "#,
            [],
        )
        .unwrap();
        conn.execute(
            r#"
            INSERT INTO commits (
                repo_path, hash, repo_name, short_hash, summary, author,
                email, timestamp, sort_ts, parents, message
            ) VALUES ('/repo', ?1, 'repo', substr(?1, 1, 7), ?2,
                      '홍길동', 'hong@example.com', 100, -100, '[]', ?3)
            "#,
            rusqlite::params![hash, summary, message],
        )
        .unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};

    fn fts_count(conn: &Connection, query: &str) -> i64 {
        let expression = crate::history_query::substring_expression(query).unwrap();
        conn.query_row(
            "SELECT COUNT(*) FROM commits_fts WHERE commits_fts MATCH ?1",
            params![expression],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn current_schema_preserves_rows_without_unbounded_fts_seed() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE repos (
                path TEXT PRIMARY KEY, name TEXT NOT NULL,
                lifecycle_marker TEXT
            );
            INSERT INTO repos(path, name, lifecycle_marker)
            VALUES ('/repo', 'repo', 'preserve-me');
            CREATE TABLE commits (
                repo_path TEXT NOT NULL, hash TEXT NOT NULL,
                repo_id INTEGER NOT NULL DEFAULT 0, repo_name TEXT NOT NULL,
                short_hash TEXT NOT NULL, summary TEXT NOT NULL,
                author TEXT NOT NULL, email TEXT NOT NULL,
                timestamp INTEGER NOT NULL, sort_ts INTEGER NOT NULL DEFAULT 0,
                branch_label TEXT, is_merge INTEGER NOT NULL DEFAULT 0,
                is_tagged INTEGER NOT NULL DEFAULT 0,
                parents TEXT NOT NULL DEFAULT '[]', message TEXT NOT NULL DEFAULT '',
                remote_tip_label TEXT,
                remote_tip_extra_count INTEGER NOT NULL DEFAULT 0,
                first_seen_generation INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY(repo_path, hash)
            );
            INSERT INTO commits (
                repo_path, hash, repo_name, short_hash, summary, author,
                email, timestamp, sort_ts
            ) VALUES ('/repo', 'abc1234', 'repo', 'abc1234',
                      'preserved optimization', 'Ada', 'ada@example.com', 1, -1);
            PRAGMA user_version = 5;
            "#,
        )
        .unwrap();
        let rowid_before: i64 = conn
            .query_row("SELECT rowid FROM commits", [], |row| row.get(0))
            .unwrap();

        {
            let tx = conn.transaction().unwrap();
            migrate(&tx).unwrap();
            tx.pragma_update(None, "user_version", HISTORY_SCHEMA_VERSION)
                .unwrap();
            tx.commit().unwrap();
        }

        let (rowid_after, epoch, truncated): (i64, i64, i64) = conn
            .query_row(
                "SELECT rowid, history_scan_epoch, message_truncated FROM commits",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(rowid_after, rowid_before);
        assert_eq!((epoch, truncated), (0, 0));
        let (lifecycle_epoch, storage_identity, marker): (i64, Option<String>, String) = conn
            .query_row(
                "SELECT lifecycle_epoch, repo_storage_identity, lifecycle_marker FROM repos WHERE path = '/repo'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(lifecycle_epoch, 0);
        assert_eq!(
            storage_identity, None,
            "migration performs no Git I/O/backfill"
        );
        assert_eq!(marker, "preserve-me");
        assert_eq!(
            fts_count(&conn, "optimization"),
            0,
            "legacy rows are picked up by bounded lazy backfill, not migration"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM history_fts_indexed", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
        assert_eq!(
            conn.query_row(
                "SELECT complete FROM history_fts_state WHERE singleton = 1",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
            0,
            "migration records that the lazy legacy index is not covered yet"
        );
        assert_eq!(
            conn.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            HISTORY_SCHEMA_VERSION
        );
    }

    #[test]
    fn current_schema_repairs_early_coverage_shape_without_losing_state() {
        let mut conn = test_support::legacy_v5_connection();
        conn.execute_batch(
            r#"
            CREATE TABLE history_coverage (
                repo_path TEXT PRIMARY KEY,
                state TEXT NOT NULL DEFAULT 'stale',
                scan_epoch INTEGER NOT NULL DEFAULT 0,
                published_epoch INTEGER NOT NULL DEFAULT 0,
                target_root_fingerprint TEXT,
                indexed_root_fingerprint TEXT,
                commits_indexed INTEGER NOT NULL DEFAULT 0,
                batches_indexed INTEGER NOT NULL DEFAULT 0,
                completed_generation INTEGER,
                started_at INTEGER,
                completed_at INTEGER,
                updated_at INTEGER NOT NULL,
                last_error TEXT
            );
            INSERT INTO history_coverage(repo_path, state, updated_at)
            VALUES ('/repo', 'stale', 123);
            PRAGMA user_version = 7;
            "#,
        )
        .unwrap();
        {
            let tx = conn.transaction().unwrap();
            migrate(&tx).unwrap();
            tx.pragma_update(None, "user_version", HISTORY_SCHEMA_VERSION)
                .unwrap();
            tx.commit().unwrap();
        }
        assert!(has_column(&conn, "history_coverage", "target_lifecycle_epoch").unwrap());
        assert_eq!(
            conn.query_row(
                "SELECT state, updated_at FROM history_coverage WHERE repo_path = '/repo'",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap(),
            ("stale".to_string(), 123)
        );
    }

    #[test]
    fn v11_cleanup_jobs_gain_phases_without_losing_queued_work() {
        let mut conn = test_support::legacy_v5_connection();
        conn.execute_batch(
            r#"
            CREATE TABLE history_cleanup_jobs (
                repo_path TEXT NOT NULL,
                cleanup_kind TEXT NOT NULL CHECK(cleanup_kind IN ('full', 'delta')),
                published_epoch INTEGER NOT NULL,
                staging_epoch INTEGER NOT NULL,
                publication_generation INTEGER NOT NULL,
                created_at INTEGER NOT NULL,
                PRIMARY KEY(repo_path, cleanup_kind, published_epoch, staging_epoch)
            ) WITHOUT ROWID;
            CREATE INDEX idx_history_cleanup_jobs_repo
                ON history_cleanup_jobs(repo_path, created_at);
            INSERT INTO history_cleanup_jobs
            VALUES('/repo', 'full', 7, 7, 12, 100);
            "#,
        )
        .unwrap();
        {
            let tx = conn.transaction().unwrap();
            migrate(&tx).unwrap();
            tx.commit().unwrap();
        }
        assert!(has_column(&conn, "history_cleanup_jobs", "cleanup_phase").unwrap());
        assert_eq!(
            conn.query_row(
                "SELECT cleanup_kind, cleanup_phase FROM history_cleanup_jobs",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .unwrap(),
            ("full".to_string(), 0)
        );
        conn.execute(
            "INSERT INTO history_cleanup_jobs VALUES('/repo', 'discard', 7, 8, 0, 0, 101)",
            [],
        )
        .unwrap();
    }

    #[test]
    fn fts_tracks_insert_text_update_delete_and_unicode() {
        let conn = test_support::v5_connection();
        test_support::insert_commit(&conn, "abcdef123", "최적화 완료", "old body");
        let rowid: i64 = conn
            .query_row("SELECT rowid FROM commits", [], |row| row.get(0))
            .unwrap();

        assert_eq!(fts_count(&conn, "최적화"), 1);
        assert_eq!(fts_count(&conn, "old"), 1);
        conn.execute(
            "UPDATE commits SET summary = '최고의 경험', message = 'new body' WHERE rowid = ?1",
            params![rowid],
        )
        .unwrap();
        assert_eq!(fts_count(&conn, "최적화"), 0);
        assert_eq!(fts_count(&conn, "최고의"), 1);
        assert_eq!(fts_count(&conn, "new"), 1);
        assert_eq!(
            conn.query_row("SELECT rowid FROM commits", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            rowid
        );

        conn.execute("DELETE FROM commits WHERE rowid = ?1", params![rowid])
            .unwrap();
        assert_eq!(fts_count(&conn, "최고의"), 0);
    }

    #[test]
    fn epoch_only_update_does_not_reindex_text() {
        let conn = test_support::v5_connection();
        test_support::insert_commit(&conn, "abcdef123", "sentinel phrase", "body");
        let rowid: i64 = conn
            .query_row("SELECT rowid FROM commits", [], |row| row.get(0))
            .unwrap();
        assert_eq!(fts_count(&conn, "sentinel"), 1);

        // Remove only the index record. An epoch-only UPDATE must not make it
        // reappear; changing searchable text must.
        conn.execute(
            r#"
            INSERT INTO commits_fts(
                commits_fts, rowid, summary, message, author, email, repo_name, hash
            )
            SELECT 'delete', rowid, summary, message, author, email, repo_name, hash
            FROM commits WHERE rowid = ?1
            "#,
            params![rowid],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM history_fts_indexed WHERE rowid = ?1",
            params![rowid],
        )
        .unwrap();
        assert_eq!(fts_count(&conn, "sentinel"), 0);
        conn.execute(
            "UPDATE commits SET history_scan_epoch = 7, first_seen_generation = 9 WHERE rowid = ?1",
            params![rowid],
        )
        .unwrap();
        assert_eq!(fts_count(&conn, "sentinel"), 0);

        conn.execute(
            "UPDATE commits SET summary = 'sentinel phrase restored' WHERE rowid = ?1",
            params![rowid],
        )
        .unwrap();
        assert_eq!(fts_count(&conn, "sentinel"), 1);
    }

    #[test]
    fn combined_repo_attachment_and_text_update_keeps_fts_consistent() {
        let conn = test_support::v5_connection();
        test_support::insert_commit(&conn, "abcdef123", "legacy phrase", "old body");
        let rowid: i64 = conn
            .query_row("SELECT rowid FROM commits", [], |row| row.get(0))
            .unwrap();
        conn.execute(
            r#"
            INSERT INTO commits_fts(
                commits_fts, rowid, summary, message, author, email, repo_name, hash
            )
            SELECT 'delete', rowid, summary, message, author, email, repo_name, hash
            FROM commits WHERE rowid = ?1
            "#,
            params![rowid],
        )
        .unwrap();
        conn.execute(
            "DELETE FROM history_fts_indexed WHERE rowid = ?1",
            params![rowid],
        )
        .unwrap();

        conn.execute(
            "UPDATE commits SET repo_id = 5, summary = 'new optimized' WHERE rowid = ?1",
            params![rowid],
        )
        .unwrap();
        assert_eq!(fts_count(&conn, "optimized"), 1);
        conn.execute(
            "INSERT INTO commits_fts(commits_fts, rank) VALUES('integrity-check', 1)",
            [],
        )
        .unwrap();
    }
}
