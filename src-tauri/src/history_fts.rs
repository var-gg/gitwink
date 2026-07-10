//! Bounded maintenance for the external-content commit text index.
//!
//! Migration never scans legacy commits. Search/history orchestration calls
//! `backfill_batch` off the startup path, while the history writer calls
//! `index_row_if_missing` for every row it already has in hand.

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FtsBackfillOutcome {
    pub(crate) indexed: usize,
    pub(crate) has_more: bool,
}

pub(crate) fn is_ready(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT complete FROM history_fts_state WHERE singleton = 1",
        [],
        |row| row.get::<_, i64>(0),
    )
    .map(|complete| complete != 0)
    .unwrap_or(false)
}

pub(crate) fn index_row_if_missing(conn: &Connection, rowid: i64) -> Result<bool> {
    let inserted = conn.execute(
        r#"
        INSERT INTO commits_fts(
            rowid, summary, message, author, email, repo_name, hash
        )
        SELECT rowid, summary, message, author, email, repo_name, hash
        FROM commits
        WHERE rowid = ?1
          AND NOT EXISTS (
              SELECT 1 FROM history_fts_indexed WHERE rowid = ?1
          )
        "#,
        params![rowid],
    )?;
    if inserted > 0 {
        conn.execute(
            "INSERT OR IGNORE INTO history_fts_indexed(rowid) VALUES (?1)",
            params![rowid],
        )?;
    }
    Ok(inserted > 0)
}

/// Lazily index a short slice of legacy rows, and nothing calls this from
/// `cache::open`. Coverage deliberately means every `commits.rowid`, not only
/// rows currently attached to a catalog entry: timeline reads also admit
/// unresolved rows, and a later catalog activation must never turn one into
/// an FTS false negative.
pub(crate) fn backfill_batch(conn: &mut Connection, limit: usize) -> Result<FtsBackfillOutcome> {
    // FTS token writes are heavier than metadata rows; keep each WAL writer
    // deliberately short and let the orchestrator schedule another slice.
    // New rows have an 8 KiB message cap, but a pre-upgrade cache may contain
    // much larger text. Bound both row count and actual UTF-8 source bytes;
    // one oversized legacy row is still allowed so progress cannot stall.
    let keep = limit.clamp(1, 256);
    const SOURCE_BYTE_BUDGET: i64 = 2 * 1024 * 1024;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    // One connection-local batch table lets SQLite do the expensive work in
    // three set operations instead of thousands of prepare/step cycles per
    // slice. TEMP keeps concurrent read connections independent.
    tx.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS history_fts_backfill_batch(\
             rowid INTEGER PRIMARY KEY\
         ) WITHOUT ROWID;\
         DELETE FROM history_fts_backfill_batch;",
    )?;
    let indexed = tx.execute(
        r#"
        INSERT INTO history_fts_backfill_batch(rowid)
        WITH candidates AS (
            SELECT commits.rowid,
                   length(CAST(summary AS BLOB)) + length(CAST(message AS BLOB))
                   + length(CAST(author AS BLOB)) + length(CAST(email AS BLOB))
                   + length(CAST(repo_name AS BLOB)) + length(CAST(hash AS BLOB)) AS source_bytes,
                   ROW_NUMBER() OVER (ORDER BY commits.rowid) AS candidate_no
            FROM commits
            LEFT JOIN history_fts_indexed fts_state
              ON fts_state.rowid = commits.rowid
            WHERE fts_state.rowid IS NULL
            ORDER BY commits.rowid
            LIMIT ?1
        ), budgeted AS (
            SELECT rowid, candidate_no,
                   SUM(source_bytes) OVER (ORDER BY rowid) AS cumulative_bytes
            FROM candidates
        )
        SELECT rowid FROM budgeted
        WHERE candidate_no = 1 OR cumulative_bytes <= ?2
        ORDER BY rowid
        "#,
        params![keep as i64, SOURCE_BYTE_BUDGET],
    )?;
    tx.execute(
        r#"
        INSERT INTO commits_fts(
            rowid, summary, message, author, email, repo_name, hash
        )
        SELECT c.rowid, c.summary, c.message, c.author, c.email, c.repo_name, c.hash
        FROM commits c
        JOIN history_fts_backfill_batch batch ON batch.rowid = c.rowid
        "#,
        [],
    )?;
    tx.execute(
        r#"
        INSERT OR IGNORE INTO history_fts_indexed(rowid)
        SELECT rowid FROM history_fts_backfill_batch
        "#,
        [],
    )?;
    // Legacy epoch-0 membership is seeded in the same bounded path. Once any
    // full scan has started, its publication protocol owns membership and
    // lazy work must not contaminate the pinned snapshot.
    tx.execute(
        r#"
        INSERT OR IGNORE INTO history_membership(repo_path, scan_epoch, hash)
        SELECT c.repo_path, 0, c.hash
        FROM commits c
        JOIN history_fts_backfill_batch batch ON batch.rowid = c.rowid
        LEFT JOIN history_coverage coverage ON coverage.repo_path = c.repo_path
        WHERE coverage.repo_path IS NULL
           OR (coverage.scan_epoch = 0 AND coverage.published_epoch = 0)
        "#,
        [],
    )?;
    let has_more: bool = tx
        .query_row(
            r#"
            SELECT 1 FROM commits
            LEFT JOIN history_fts_indexed fts_state
              ON fts_state.rowid = commits.rowid
            WHERE fts_state.rowid IS NULL
            LIMIT 1
            "#,
            [],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    tx.execute(
        "UPDATE history_fts_state SET complete = ?1 WHERE singleton = 1",
        params![i64::from(!has_more)],
    )?;
    tx.commit()?;
    Ok(FtsBackfillOutcome { indexed, has_more })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_schema::test_support;

    #[test]
    fn legacy_rows_become_discoverable_in_bounded_slices() {
        let mut conn = test_support::legacy_v5_connection();
        test_support::insert_commit(&conn, "legacy11", "legacy optimization", "first body");
        test_support::insert_commit(&conn, "legacy22", "legacy indexing", "second body");
        test_support::migrate_to_current(&mut conn);

        let before: i64 = conn
            .query_row("SELECT COUNT(*) FROM history_fts_indexed", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(before, 0, "migration performs no unbounded FTS seed");
        assert!(!is_ready(&conn), "cold legacy coverage is explicit");
        assert_eq!(
            backfill_batch(&mut conn, 1).unwrap(),
            FtsBackfillOutcome {
                indexed: 1,
                has_more: true,
            }
        );
        assert_eq!(
            backfill_batch(&mut conn, 1).unwrap(),
            FtsBackfillOutcome {
                indexed: 1,
                has_more: false,
            }
        );
        assert_eq!(backfill_batch(&mut conn, 100).unwrap().indexed, 0);
        assert!(is_ready(&conn), "the hard gate opens only after coverage");
        assert_eq!(
            crate::history_query::search_repo_substring(&conn, "/repo", 0, "gacy", 10)
                .unwrap()
                .len(),
            2,
            "lazy backfill seeds matching epoch-0 reachability too"
        );
    }

    #[test]
    fn completeness_covers_removed_legacy_rows_too() {
        let mut conn = test_support::legacy_v5_connection();
        test_support::insert_commit(&conn, "excluded1", "private legacy", "private body");
        test_support::migrate_to_current(&mut conn);
        conn.execute(
            "UPDATE repos SET user_state = 'removed', lifecycle_epoch = lifecycle_epoch + 1 WHERE path = '/repo'",
            [],
        )
        .unwrap();

        assert_eq!(
            backfill_batch(&mut conn, 100).unwrap(),
            FtsBackfillOutcome {
                indexed: 1,
                has_more: false,
            }
        );
        assert!(
            is_ready(&conn),
            "complete always means every canonical commit row is indexed"
        );
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM commits_fts WHERE commits_fts MATCH ?1",
                [crate::history_query::substring_expression("private").unwrap()],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
    }

    #[test]
    fn orphan_repo_id_zero_row_cannot_make_ready_state_incomplete() {
        let mut conn = test_support::legacy_v5_connection();
        test_support::insert_commit(&conn, "orphan11", "orphan searchable", "body");
        conn.execute("DELETE FROM repos", []).unwrap();
        test_support::migrate_to_current(&mut conn);

        assert_eq!(
            backfill_batch(&mut conn, 100).unwrap(),
            FtsBackfillOutcome {
                indexed: 1,
                has_more: false,
            }
        );
        assert!(is_ready(&conn));
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM history_fts_indexed", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            1
        );
    }
}
