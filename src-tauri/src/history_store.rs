//! SQLite persistence and publication protocol for complete history scans.
//!
//! Metadata batches may land incrementally, but reachability is published by
//! epoch. Readers stay on `published_epoch` until an exhausted walk passes a
//! full root-snapshot recheck; force-push/delete rebuilds therefore cannot
//! expose a mixed graph as current truth.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use crate::cache;
#[cfg(test)]
use crate::history_git::WalkStats;
use crate::history_git::{
    HistoryCommit, HistoryRoot, HistoryRootKind, RootSnapshot, WalkCompletion,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CoverageState {
    Stale,
    Indexing,
    Complete,
    Cancelled,
    Failed,
}

impl CoverageState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Stale => "stale",
            Self::Indexing => "indexing",
            Self::Complete => "complete",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }

    fn from_str(value: &str) -> Result<Self> {
        match value {
            "stale" => Ok(Self::Stale),
            "indexing" => Ok(Self::Indexing),
            "complete" => Ok(Self::Complete),
            "cancelled" => Ok(Self::Cancelled),
            "failed" => Ok(Self::Failed),
            other => Err(anyhow!("unknown history coverage state {other:?}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HistoryCoverage {
    pub(crate) repo_path: String,
    pub(crate) state: CoverageState,
    pub(crate) scan_epoch: i64,
    pub(crate) published_epoch: i64,
    pub(crate) target_lifecycle_epoch: Option<i64>,
    pub(crate) target_root_fingerprint: Option<String>,
    pub(crate) indexed_root_fingerprint: Option<String>,
    pub(crate) commits_indexed: usize,
    pub(crate) batches_indexed: usize,
    pub(crate) completed_generation: Option<i64>,
    pub(crate) started_at: Option<i64>,
    pub(crate) completed_at: Option<i64>,
    pub(crate) updated_at: i64,
    pub(crate) last_error: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct ScanLease {
    pub(crate) repo_path: String,
    pub(crate) scan_epoch: i64,
    pub(crate) published_epoch: i64,
    pub(crate) lifecycle_epoch: i64,
    pub(crate) target: RootSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BatchOutcome {
    pub(crate) generation: i64,
    pub(crate) inserted: usize,
    pub(crate) rows: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinalizeOutcome {
    Published {
        generation: i64,
        deleted: usize,
        commits: usize,
        maintenance_pending: bool,
    },
    RootsChanged,
    Cancelled,
}

#[derive(Debug, Clone)]
struct CleanupJob {
    kind: String,
    published_epoch: i64,
    staging_epoch: i64,
    publication_generation: i64,
    phase: i64,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct CleanupProgress {
    pub(crate) deleted: usize,
    pub(crate) complete: bool,
}

fn cleanup_cancelled(cancellation: Option<&CancellationToken>) -> bool {
    cancellation.is_some_and(CancellationToken::is_cancelled)
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn coverage_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryCoverage> {
    let state: String = row.get(1)?;
    let state = CoverageState::from_str(&state).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            1,
            rusqlite::types::Type::Text,
            Box::new(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                error.to_string(),
            )),
        )
    })?;
    let commits: i64 = row.get(9)?;
    let batches: i64 = row.get(10)?;
    Ok(HistoryCoverage {
        repo_path: row.get(0)?,
        state,
        scan_epoch: row.get(2)?,
        published_epoch: row.get(3)?,
        target_lifecycle_epoch: row.get(4)?,
        target_root_fingerprint: row.get(5)?,
        indexed_root_fingerprint: row.get(6)?,
        completed_generation: row.get(7)?,
        started_at: row.get(8)?,
        commits_indexed: commits.max(0) as usize,
        batches_indexed: batches.max(0) as usize,
        completed_at: row.get(11)?,
        updated_at: row.get(12)?,
        last_error: row.get(13)?,
    })
}

pub(crate) fn coverage(conn: &Connection, repo_path: &str) -> Result<Option<HistoryCoverage>> {
    conn.query_row(
        r#"
        SELECT repo_path, state, scan_epoch, published_epoch,
               target_lifecycle_epoch, target_root_fingerprint, indexed_root_fingerprint,
               completed_generation, started_at, commits_indexed,
               batches_indexed, completed_at, updated_at, last_error
        FROM history_coverage WHERE repo_path = ?1
        "#,
        params![repo_path],
        coverage_from_row,
    )
    .optional()
    .map_err(Into::into)
}

pub(crate) fn active_lifecycle_epoch(conn: &Connection, repo_path: &str) -> Result<Option<i64>> {
    conn.query_row(
        r#"
        SELECT lifecycle_epoch FROM repos
        WHERE (path = ?1 OR canonical_path = ?1)
          AND status = 'active'
          AND user_state != 'removed'
        ORDER BY CASE WHEN path = ?1 THEN 0 ELSE 1 END
        LIMIT 1
        "#,
        params![repo_path],
        |row| row.get(0),
    )
    .optional()
    .map_err(Into::into)
}

pub(crate) fn begin_scan(
    conn: &mut Connection,
    repo_path: &str,
    target: RootSnapshot,
) -> Result<ScanLease> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let lifecycle_epoch: i64 = tx
        .query_row(
            r#"
            SELECT lifecycle_epoch FROM repos
            WHERE (path = ?1 OR canonical_path = ?1)
              AND status = 'active'
              AND user_state != 'removed'
            ORDER BY CASE WHEN path = ?1 THEN 0 ELSE 1 END
            LIMIT 1
            "#,
            params![repo_path],
            |row| row.get(0),
        )
        .optional()?
        .with_context(|| format!("repository is not active in catalog: {repo_path}"))?;
    let (prior_scan, published): (i64, i64) = tx
        .query_row(
            "SELECT scan_epoch, published_epoch FROM history_coverage WHERE repo_path = ?1",
            params![repo_path],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .unwrap_or((0, 0));
    let scan_epoch = prior_scan
        .max(published)
        .checked_add(1)
        .context("history epoch overflow")?;
    let now = unix_now();

    tx.execute(
        r#"
        INSERT INTO history_coverage (
            repo_path, state, scan_epoch, published_epoch,
            target_lifecycle_epoch, target_root_fingerprint, indexed_root_fingerprint,
            commits_indexed, batches_indexed, completed_generation,
            started_at, completed_at, updated_at, last_error
        ) VALUES (?1, 'indexing', ?2, ?3, ?4, ?5, NULL, 0, 0, NULL, ?6, NULL, ?6, NULL)
        ON CONFLICT(repo_path) DO UPDATE SET
            state = 'indexing',
            scan_epoch = excluded.scan_epoch,
            target_lifecycle_epoch = excluded.target_lifecycle_epoch,
            target_root_fingerprint = excluded.target_root_fingerprint,
            commits_indexed = 0,
            batches_indexed = 0,
            started_at = excluded.started_at,
            completed_at = NULL,
            updated_at = excluded.updated_at,
            last_error = NULL
        "#,
        params![
            repo_path,
            scan_epoch,
            published,
            lifecycle_epoch,
            target.fingerprint,
            now
        ],
    )?;
    // Convert every pre-existing non-published epoch into durable low-priority
    // cleanup work. This is metadata-only and bounded by the number of
    // interrupted scans; no commit/FTS row is deleted on the scan hot path.
    tx.execute(
        "INSERT OR IGNORE INTO history_cleanup_jobs ( \
             repo_path, cleanup_kind, published_epoch, staging_epoch, \
             publication_generation, cleanup_phase, created_at) \
         SELECT ?1, 'discard', ?2, abandoned_epoch, 0, 0, ?4 FROM ( \
             SELECT DISTINCT history_scan_epoch AS abandoned_epoch FROM commits \
             WHERE repo_path = ?1 AND history_scan_epoch > 0 \
               AND history_scan_epoch < ?3 AND history_scan_epoch != ?2 \
             UNION \
             SELECT DISTINCT scan_epoch AS abandoned_epoch FROM history_membership \
             WHERE repo_path = ?1 AND scan_epoch > 0 \
               AND scan_epoch < ?3 AND scan_epoch != ?2) abandoned \
         WHERE NOT EXISTS ( \
             SELECT 1 FROM history_cleanup_jobs owner \
             WHERE owner.repo_path = ?1 AND owner.staging_epoch = abandoned_epoch \
               AND owner.cleanup_kind IN ('full', 'delta'))",
        params![repo_path, published, scan_epoch, now],
    )?;
    tx.commit()?;
    Ok(ScanLease {
        repo_path: repo_path.to_string(),
        scan_epoch,
        published_epoch: published,
        lifecycle_epoch,
        target,
    })
}

fn ensure_active_scan(conn: &Connection, lease: &ScanLease) -> Result<()> {
    let active: bool = conn
        .query_row(
            r#"
            SELECT 1 FROM history_coverage
            WHERE repo_path = ?1 AND state = 'indexing' AND scan_epoch = ?2
              AND published_epoch = ?3 AND target_lifecycle_epoch = ?4
              AND target_root_fingerprint = ?5
              AND EXISTS (
                    SELECT 1 FROM repos
                    WHERE (path = ?1 OR canonical_path = ?1)
                      AND status = 'active'
                      AND user_state != 'removed'
                      AND lifecycle_epoch = ?4
              )
            "#,
            params![
                lease.repo_path,
                lease.scan_epoch,
                lease.published_epoch,
                lease.lifecycle_epoch,
                lease.target.fingerprint
            ],
            |_| Ok(true),
        )
        .optional()?
        .unwrap_or(false);
    if !active {
        bail!(
            "history scan lease is no longer active for {} epoch {}",
            lease.repo_path,
            lease.scan_epoch
        );
    }
    Ok(())
}

pub(crate) fn upsert_batch(
    conn: &mut Connection,
    lease: &ScanLease,
    commits: &[HistoryCommit],
) -> Result<BatchOutcome> {
    if commits.is_empty() {
        return Ok(BatchOutcome {
            generation: 0,
            inserted: 0,
            rows: 0,
        });
    }
    if commits
        .iter()
        .any(|commit| commit.summary.repo_path != lease.repo_path)
    {
        bail!("history batch contains a commit from a different repository");
    }

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_active_scan(&tx, lease)?;
    let generation = cache::next_generation(&tx)?;
    let mut inserted = 0usize;
    {
        let mut statement = tx.prepare(
            r#"
            INSERT INTO commits (
                repo_path, hash, repo_id, repo_name, short_hash, summary,
                author, email, timestamp, sort_ts, branch_label, is_merge,
                is_tagged, parents, message, remote_tip_label,
                remote_tip_extra_count, first_seen_generation,
                history_scan_epoch, message_truncated
            ) VALUES (
                ?1, ?2, COALESCE((SELECT rowid FROM repos WHERE path = ?1), 0),
                ?3, ?4, ?5, ?6, ?7, ?8, -?8, NULL, ?9, ?10, ?11, ?12,
                ?13, ?14, ?15, ?16, ?17
            )
            ON CONFLICT(repo_path, hash) DO UPDATE SET
                repo_id = excluded.repo_id,
                repo_name = excluded.repo_name,
                short_hash = excluded.short_hash,
                summary = excluded.summary,
                author = excluded.author,
                email = excluded.email,
                timestamp = excluded.timestamp,
                sort_ts = excluded.sort_ts,
                is_merge = excluded.is_merge,
                parents = excluded.parents,
                message = excluded.message,
                message_truncated = excluded.message_truncated
            RETURNING rowid, first_seen_generation
            "#,
        )?;
        let mut membership = tx.prepare(
            "INSERT OR IGNORE INTO history_membership(repo_path, scan_epoch, hash) VALUES (?1, ?2, ?3)",
        )?;
        for history_commit in commits {
            let commit = &history_commit.summary;
            let parents = serde_json::to_string(&commit.parents).unwrap_or_else(|_| "[]".into());
            let (rowid, first_seen): (i64, i64) = statement.query_row(
                params![
                    commit.repo_path,
                    commit.hash,
                    commit.repo_name,
                    commit.short_hash,
                    commit.summary,
                    commit.author,
                    commit.email,
                    commit.timestamp,
                    commit.is_merge as i32,
                    commit.is_tagged as i32,
                    parents,
                    commit.message,
                    commit.remote_tip_label,
                    commit.remote_tip_extra_count as i64,
                    generation,
                    lease.scan_epoch,
                    history_commit.message_truncated as i32,
                ],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            if first_seen == generation {
                inserted += 1;
            }
            membership.execute(params![lease.repo_path, lease.scan_epoch, commit.hash])?;
            // A v5 row starts with no FTS record by design. An unchanged
            // UPSERT does not fire the text trigger, so the full scan seeds
            // that exact rowid explicitly and idempotently here.
            crate::history_fts::index_row_if_missing(&tx, rowid)?;
        }
    }
    let updated = tx.execute(
        r#"
        UPDATE history_coverage SET
            commits_indexed = commits_indexed + ?3,
            batches_indexed = batches_indexed + 1,
            updated_at = ?4
        WHERE repo_path = ?1 AND state = 'indexing' AND scan_epoch = ?2
        "#,
        params![
            lease.repo_path,
            lease.scan_epoch,
            commits.len() as i64,
            unix_now()
        ],
    )?;
    if updated != 1 {
        bail!("history scan lease changed while writing a batch");
    }
    tx.commit()?;
    Ok(BatchOutcome {
        generation,
        inserted,
        rows: commits.len(),
    })
}

fn set_terminal_state(
    conn: &Connection,
    lease: &ScanLease,
    state: CoverageState,
    message: Option<&str>,
) -> Result<bool> {
    let tx = conn.unchecked_transaction()?;
    let changed = tx.execute(
        r#"
        UPDATE history_coverage SET state = ?3, updated_at = ?4, last_error = ?5
        WHERE repo_path = ?1 AND scan_epoch = ?2 AND state = 'indexing'
        "#,
        params![
            lease.repo_path,
            lease.scan_epoch,
            state.as_str(),
            unix_now(),
            message
        ],
    )?;
    if changed == 1 {
        enqueue_cleanup_job(
            &tx,
            &lease.repo_path,
            "discard",
            lease.published_epoch,
            lease.scan_epoch,
            0,
        )?;
    }
    tx.commit()?;
    Ok(changed == 1)
}

const HISTORY_GC_BATCH: i64 = 512;

fn enqueue_cleanup_job(
    conn: &Connection,
    repo_path: &str,
    kind: &str,
    published_epoch: i64,
    staging_epoch: i64,
    publication_generation: i64,
) -> Result<()> {
    let initial_phase = if kind == "delta" { 1 } else { 0 };
    conn.execute(
        "INSERT OR IGNORE INTO history_cleanup_jobs ( \
             repo_path, cleanup_kind, published_epoch, staging_epoch, \
             publication_generation, cleanup_phase, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            repo_path,
            kind,
            published_epoch,
            staging_epoch,
            publication_generation,
            initial_phase,
            unix_now()
        ],
    )?;
    Ok(())
}

pub(crate) fn queue_repo_purge(conn: &Connection, repo_path: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM history_cleanup_jobs WHERE repo_path = ?1",
        params![repo_path],
    )?;
    enqueue_cleanup_job(conn, repo_path, "purge", 0, 0, 0)
}

/// Execute at most one 512-row maintenance transaction. Logical visibility is
/// already fenced by coverage/membership, so cleanup always yields to actual
/// scan or UI work and can be resumed idempotently after cancellation/crash.
pub(crate) fn resume_pending_cleanup(
    conn: &Connection,
    repo_path: &str,
    cancellation: Option<&CancellationToken>,
) -> Result<CleanupProgress> {
    let job = conn
        .query_row(
            "SELECT cleanup_kind, published_epoch, staging_epoch, \
                    publication_generation, cleanup_phase \
             FROM history_cleanup_jobs WHERE repo_path = ?1 \
             ORDER BY created_at, published_epoch, staging_epoch LIMIT 1",
            params![repo_path],
            |row| {
                Ok(CleanupJob {
                    kind: row.get(0)?,
                    published_epoch: row.get(1)?,
                    staging_epoch: row.get(2)?,
                    publication_generation: row.get(3)?,
                    phase: row.get(4)?,
                })
            },
        )
        .optional()?;
    let Some(job) = job else {
        return Ok(CleanupProgress {
            deleted: 0,
            complete: true,
        });
    };
    if cleanup_cancelled(cancellation) {
        return Ok(CleanupProgress {
            deleted: 0,
            complete: false,
        });
    }

    let tx = conn.unchecked_transaction()?;
    if cleanup_cancelled(cancellation) {
        return Ok(CleanupProgress {
            deleted: 0,
            complete: false,
        });
    }
    let affected = match (job.kind.as_str(), job.phase) {
        ("full", 0) => tx.execute(
            "DELETE FROM commits WHERE rowid IN ( \
                 SELECT stale.rowid FROM commits stale \
                 WHERE stale.repo_path = ?1 \
                   AND stale.first_seen_generation <= ?3 \
                   AND NOT EXISTS (SELECT 1 FROM history_membership published \
                       WHERE published.repo_path = stale.repo_path \
                         AND published.hash = stale.hash AND published.scan_epoch = ?2) \
                   AND NOT EXISTS (SELECT 1 FROM history_membership future \
                       WHERE future.repo_path = stale.repo_path \
                         AND future.hash = stale.hash AND future.scan_epoch > ?2) \
                   AND EXISTS (SELECT 1 FROM history_coverage coverage \
                       WHERE coverage.repo_path = stale.repo_path \
                         AND coverage.published_epoch = ?2) \
                 LIMIT ?4)",
            params![
                repo_path,
                job.published_epoch,
                job.publication_generation,
                HISTORY_GC_BATCH
            ],
        )?,
        ("discard", 0) => tx.execute(
            "DELETE FROM commits WHERE rowid IN ( \
                 SELECT stale.rowid FROM commits stale \
                 WHERE stale.repo_path = ?1 AND stale.history_scan_epoch = ?2 \
                   AND NOT EXISTS (SELECT 1 FROM history_coverage coverage \
                       JOIN history_membership published \
                         ON published.repo_path = coverage.repo_path \
                        AND published.scan_epoch = coverage.published_epoch \
                        AND published.hash = stale.hash \
                       WHERE coverage.repo_path = stale.repo_path \
                         AND coverage.published_epoch > 0) \
                   AND NOT EXISTS (SELECT 1 FROM history_membership future \
                       WHERE future.repo_path = stale.repo_path \
                         AND future.hash = stale.hash AND future.scan_epoch > ?2) \
                 LIMIT ?3)",
            params![repo_path, job.staging_epoch, HISTORY_GC_BATCH],
        )?,
        ("purge", 0) => tx.execute(
            "DELETE FROM commits WHERE rowid IN ( \
                 SELECT stale.rowid FROM commits stale \
                 WHERE stale.repo_path = ?1 \
                   AND (EXISTS (SELECT 1 FROM repos excluded \
                          WHERE excluded.user_state = 'removed' \
                            AND (excluded.path = stale.repo_path \
                                 OR excluded.canonical_path = stale.repo_path)) \
                        OR EXISTS (SELECT 1 FROM discovery_tombstones tombstone \
                          WHERE tombstone.canonical_path = stale.repo_path \
                            AND tombstone.reason LIKE 'user_exclude%')) \
                 LIMIT ?2)",
            params![repo_path, HISTORY_GC_BATCH],
        )?,
        ("full" | "delta", 1) => tx.execute(
            "UPDATE commits SET history_scan_epoch = 0 WHERE rowid IN ( \
                 SELECT rowid FROM commits WHERE repo_path = ?1 \
                   AND history_scan_epoch = ?2 LIMIT ?3)",
            params![repo_path, job.staging_epoch, HISTORY_GC_BATCH],
        )?,
        ("full", 2) => tx.execute(
            "DELETE FROM history_membership WHERE (repo_path, scan_epoch, hash) IN ( \
                 SELECT repo_path, scan_epoch, hash FROM history_membership \
                 WHERE repo_path = ?1 AND scan_epoch < ?2 LIMIT ?3)",
            params![repo_path, job.published_epoch, HISTORY_GC_BATCH],
        )?,
        ("delta" | "discard", 2) => tx.execute(
            "DELETE FROM history_membership WHERE (repo_path, scan_epoch, hash) IN ( \
                 SELECT repo_path, scan_epoch, hash FROM history_membership \
                 WHERE repo_path = ?1 AND scan_epoch = ?2 LIMIT ?3)",
            params![repo_path, job.staging_epoch, HISTORY_GC_BATCH],
        )?,
        ("purge", 2) => tx.execute(
            "DELETE FROM history_membership WHERE (repo_path, scan_epoch, hash) IN ( \
                 SELECT repo_path, scan_epoch, hash FROM history_membership \
                 WHERE repo_path = ?1 LIMIT ?2)",
            params![repo_path, HISTORY_GC_BATCH],
        )?,
        (kind, phase) => bail!("invalid history cleanup phase {kind:?}/{phase}"),
    };

    let phase_complete = affected < HISTORY_GC_BATCH as usize;
    let next_phase = match (job.kind.as_str(), job.phase) {
        ("full", 0) => Some(1),
        ("discard" | "purge", 0) => Some(2),
        ("full" | "delta", 1) => Some(2),
        ("full" | "delta" | "discard" | "purge", 2) => None,
        _ => None,
    };
    if phase_complete {
        if let Some(next_phase) = next_phase {
            tx.execute(
                "UPDATE history_cleanup_jobs SET cleanup_phase = ?5 \
                 WHERE repo_path = ?1 AND cleanup_kind = ?2 \
                   AND published_epoch = ?3 AND staging_epoch = ?4",
                params![
                    repo_path,
                    job.kind,
                    job.published_epoch,
                    job.staging_epoch,
                    next_phase
                ],
            )?;
        } else {
            tx.execute(
                "DELETE FROM history_cleanup_jobs \
                 WHERE repo_path = ?1 AND cleanup_kind = ?2 \
                   AND published_epoch = ?3 AND staging_epoch = ?4",
                params![repo_path, job.kind, job.published_epoch, job.staging_epoch],
            )?;
        }
    }
    tx.commit()?;
    let pending: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM history_cleanup_jobs WHERE repo_path = ?1)",
        params![repo_path],
        |row| row.get::<_, i64>(0),
    )? != 0;
    Ok(CleanupProgress {
        deleted: if matches!(
            (job.kind.as_str(), job.phase),
            ("full" | "discard" | "purge", 0)
        ) {
            affected
        } else {
            0
        },
        complete: !pending,
    })
}

pub(crate) fn has_pending_cleanup(conn: &Connection, repo_path: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM history_cleanup_jobs WHERE repo_path = ?1)",
        params![repo_path],
        |row| row.get::<_, i64>(0),
    )? != 0)
}

pub(crate) fn pending_cleanup_paths(conn: &Connection) -> Result<Vec<String>> {
    let mut statement =
        conn.prepare("SELECT DISTINCT repo_path FROM history_cleanup_jobs ORDER BY repo_path")?;
    let paths = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(paths)
}

pub(crate) fn cancel_scan(conn: &Connection, lease: &ScanLease) -> Result<bool> {
    set_terminal_state(conn, lease, CoverageState::Cancelled, None)
}

pub(crate) fn fail_scan(conn: &Connection, lease: &ScanLease, error: &str) -> Result<bool> {
    set_terminal_state(conn, lease, CoverageState::Failed, Some(error))
}

/// Once a worker's mutation gate is detached, retire whichever scan it left
/// active and hand its staging epoch to durable maintenance. Callers serialize
/// this before starting another scan for the same path.
pub(crate) fn retire_detached_scan(conn: &Connection, repo_path: &str) -> Result<bool> {
    let tx = conn.unchecked_transaction()?;
    let active = tx
        .query_row(
            "SELECT scan_epoch, published_epoch FROM history_coverage \
             WHERE repo_path = ?1 AND state = 'indexing'",
            params![repo_path],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .optional()?;
    let Some((scan_epoch, published_epoch)) = active else {
        tx.commit()?;
        return Ok(false);
    };
    let changed = tx.execute(
        "UPDATE history_coverage SET state = 'cancelled', updated_at = ?2 \
         WHERE repo_path = ?1 AND state = 'indexing' AND scan_epoch = ?3",
        params![repo_path, unix_now(), scan_epoch],
    )?;
    if changed == 1 {
        enqueue_cleanup_job(&tx, repo_path, "discard", published_epoch, scan_epoch, 0)?;
    }
    tx.commit()?;
    Ok(changed == 1)
}

pub(crate) fn mark_stale(
    conn: &Connection,
    repo_path: &str,
    observed: Option<&RootSnapshot>,
) -> Result<bool> {
    let now = unix_now();
    let changed = conn.execute(
        r#"
        UPDATE history_coverage SET
            state = 'stale',
            target_lifecycle_epoch = (
                SELECT lifecycle_epoch FROM repos
                WHERE (path = ?1 OR canonical_path = ?1)
                  AND status = 'active' AND user_state != 'removed'
                ORDER BY CASE WHEN path = ?1 THEN 0 ELSE 1 END
                LIMIT 1
            ),
            target_root_fingerprint = COALESCE(?2, target_root_fingerprint),
            updated_at = ?3,
            last_error = NULL
        WHERE repo_path = ?1
          AND EXISTS (
                SELECT 1 FROM repos
                WHERE (path = ?1 OR canonical_path = ?1)
                  AND status = 'active' AND user_state != 'removed'
          )
          AND (
                ?2 IS NULL
                OR indexed_root_fingerprint IS NULL
                OR indexed_root_fingerprint != ?2
          )
          AND NOT (
                state = 'indexing'
                AND ?2 IS NOT NULL
                AND target_root_fingerprint = ?2
          )
        "#,
        params![
            repo_path,
            observed.map(|root| root.fingerprint.as_str()),
            now
        ],
    )?;
    Ok(changed == 1)
}

/// Persist a terminal error even when root discovery failed before a scan
/// lease could be created. A previous published membership stays readable;
/// only the coverage state/error changes. This also prevents the UI from
/// polling forever on a repository that cannot currently be opened.
pub(crate) fn record_scan_failure(conn: &Connection, repo_path: &str, error: &str) -> Result<()> {
    let now = unix_now();
    conn.execute(
        r#"
        INSERT INTO history_coverage (
            repo_path, state, scan_epoch, published_epoch,
            target_lifecycle_epoch, target_root_fingerprint,
            indexed_root_fingerprint, commits_indexed, batches_indexed,
            completed_generation, started_at, completed_at, updated_at,
            last_error
        )
        SELECT ?1, 'failed', 0, 0, lifecycle_epoch, NULL,
               NULL, 0, 0, NULL, NULL, NULL, ?2, ?3
        FROM repos
        WHERE (path = ?1 OR canonical_path = ?1)
          AND status = 'active' AND user_state != 'removed'
        ORDER BY CASE WHEN path = ?1 THEN 0 ELSE 1 END
        LIMIT 1
        ON CONFLICT(repo_path) DO UPDATE SET
            state = 'failed',
            target_lifecycle_epoch = excluded.target_lifecycle_epoch,
            updated_at = excluded.updated_at,
            last_error = excluded.last_error
        "#,
        params![repo_path, now, error],
    )?;
    Ok(())
}

/// Exclusion intentionally forgets indexed history while retaining the repo
/// catalog/preferences. Keeping a coverage row after its commit rows were
/// purged would pin restore to an empty published membership and hide the
/// freshly-refilled recent window.
pub(crate) fn forget_repo(conn: &mut Connection, repo_path: &str) -> Result<()> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    queue_repo_purge(&tx, repo_path)?;
    tx.execute(
        "DELETE FROM history_roots WHERE repo_path = ?1",
        params![repo_path],
    )?;
    tx.execute(
        "DELETE FROM history_coverage WHERE repo_path = ?1",
        params![repo_path],
    )?;
    tx.commit()?;
    Ok(())
}

/// User-initiated restore/replace cannot reactivate a path until its old
/// identity's rows are gone. Drain using the same small transactions as
/// background maintenance so Settings may wait, but the panel and unrelated
/// repository writers never sit behind one giant FTS delete.
pub(crate) fn drain_pending_cleanup(conn: &Connection, repo_path: &str) -> Result<usize> {
    let mut deleted = 0usize;
    loop {
        let progress = resume_pending_cleanup(conn, repo_path, None)?;
        deleted += progress.deleted;
        if progress.complete {
            return Ok(deleted);
        }
        std::thread::yield_now();
    }
}

/// Atomically publish an exhausted scan or leave the prior epoch untouched.
pub(crate) fn finalize_scan(
    conn: &mut Connection,
    lease: &ScanLease,
    rechecked: &RootSnapshot,
    completion: WalkCompletion,
    cancellation: Option<&CancellationToken>,
) -> Result<FinalizeOutcome> {
    if cleanup_cancelled(cancellation) {
        cancel_scan(conn, lease)?;
        return Ok(FinalizeOutcome::Cancelled);
    }
    let stats = match completion {
        WalkCompletion::Exhausted(stats) => stats,
        WalkCompletion::Cancelled(_) => bail!("a cancelled history walk cannot be finalized"),
    };
    if rechecked.roots != lease.target.roots || rechecked.fingerprint != lease.target.fingerprint {
        set_terminal_state(
            conn,
            lease,
            CoverageState::Stale,
            Some("repository roots changed during history scan"),
        )?;
        return Ok(FinalizeOutcome::RootsChanged);
    }

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_active_scan(&tx, lease)?;
    if cleanup_cancelled(cancellation) {
        drop(tx);
        cancel_scan(conn, lease)?;
        return Ok(FinalizeOutcome::Cancelled);
    }
    let membership_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM history_membership WHERE repo_path = ?1 AND scan_epoch = ?2",
        params![lease.repo_path, lease.scan_epoch],
        |row| row.get(0),
    )?;
    if membership_count != stats.commits as i64 {
        bail!(
            "history membership count {} does not match exhausted walk count {}",
            membership_count,
            stats.commits
        );
    }
    let recorded: (i64, i64) = tx.query_row(
        "SELECT commits_indexed, batches_indexed FROM history_coverage WHERE repo_path = ?1",
        params![lease.repo_path],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if recorded != (stats.commits as i64, stats.batches as i64) {
        bail!(
            "history coverage counters {:?} do not match exhausted walk ({}, {})",
            recorded,
            stats.commits,
            stats.batches
        );
    }

    // UI invalidation describes the logical membership switch, never how many
    // physical rows a later cleanup slice happened to reclaim. One bit is
    // sufficient for the timeline's reload-vs-countNew decision and keeps the
    // publication transaction independent of history size.
    let logical_deleted = if lease.published_epoch > 0 {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM history_membership old \
             WHERE old.repo_path = ?1 AND old.scan_epoch = ?2 \
               AND NOT EXISTS (SELECT 1 FROM history_membership new \
                   WHERE new.repo_path = old.repo_path AND new.hash = old.hash \
                     AND new.scan_epoch = ?3))",
            params![lease.repo_path, lease.published_epoch, lease.scan_epoch],
            |row| row.get::<_, i64>(0),
        )? != 0
    } else {
        tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM commits baseline \
             WHERE baseline.repo_path = ?1 AND baseline.history_scan_epoch = 0 \
               AND NOT EXISTS (SELECT 1 FROM history_membership new \
                   WHERE new.repo_path = baseline.repo_path AND new.hash = baseline.hash \
                     AND new.scan_epoch = ?2))",
            params![lease.repo_path, lease.scan_epoch],
            |row| row.get::<_, i64>(0),
        )? != 0
    };

    // The coverage pointer is the logical publication boundary. Readers
    // observe either the previous published membership or the complete new
    // one; potentially large physical reclamation is intentionally deferred
    // to bounded post-commit transactions.
    let generation = cache::next_generation(&tx)?;
    // Publishing membership changes logical visibility even when physical GC
    // deletes nothing (for example, a previously cached row becomes newly
    // reachable). Invalidate every older pinned timeline snapshot so one
    // pagination sequence can never straddle two publication epochs.
    cache::record_destructive_generation(&tx, generation)?;
    let prior_roots = published_roots(&tx, &lease.repo_path)?;
    let prior_snapshot = RootSnapshot::from_roots(prior_roots);
    if prior_snapshot.roots.is_empty() {
        crate::history_decorations::replace_tip_decorations(&tx, &lease.repo_path, rechecked)?;
    } else {
        crate::history_decorations::refresh_tip_decorations(
            &tx,
            &lease.repo_path,
            &prior_snapshot,
            rechecked,
        )?;
    }

    tx.execute(
        "DELETE FROM history_roots WHERE repo_path = ?1",
        params![lease.repo_path],
    )?;
    {
        let mut insert_root = tx.prepare(
            "INSERT INTO history_roots(repo_path, ref_name, oid, root_kind) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for root in &rechecked.roots {
            insert_root.execute(params![
                lease.repo_path,
                root.ref_name,
                root.oid,
                root.kind.as_str()
            ])?;
        }
    }
    if cleanup_cancelled(cancellation) {
        drop(tx);
        cancel_scan(conn, lease)?;
        return Ok(FinalizeOutcome::Cancelled);
    }
    let now = unix_now();
    let updated = tx.execute(
        r#"
        UPDATE history_coverage SET
            state = 'complete',
            published_epoch = ?2,
            indexed_root_fingerprint = ?3,
            completed_generation = ?4,
            completed_at = ?5,
            updated_at = ?5,
            last_error = NULL
        WHERE repo_path = ?1 AND state = 'indexing' AND scan_epoch = ?2
        "#,
        params![
            lease.repo_path,
            lease.scan_epoch,
            rechecked.fingerprint,
            generation,
            now
        ],
    )?;
    if updated != 1 {
        bail!("history scan lease changed before publication");
    }
    enqueue_cleanup_job(
        &tx,
        &lease.repo_path,
        "full",
        lease.scan_epoch,
        lease.scan_epoch,
        generation,
    )?;
    tx.commit()?;
    Ok(FinalizeOutcome::Published {
        generation,
        deleted: usize::from(logical_deleted),
        commits: stats.commits,
        maintenance_pending: true,
    })
}

/// Publish a proven additive root update without copying the entire previous
/// membership or rescanning/sweeping old history. New staging membership is
/// merged into the existing published epoch in one short transaction.
pub(crate) fn finalize_delta_scan(
    conn: &mut Connection,
    lease: &ScanLease,
    published: &RootSnapshot,
    rechecked: &RootSnapshot,
    completion: WalkCompletion,
    cancellation: Option<&CancellationToken>,
) -> Result<FinalizeOutcome> {
    if cleanup_cancelled(cancellation) {
        cancel_scan(conn, lease)?;
        return Ok(FinalizeOutcome::Cancelled);
    }
    if lease.published_epoch <= 0 {
        bail!("delta publication requires an existing complete epoch");
    }
    let stats = match completion {
        WalkCompletion::Exhausted(stats) => stats,
        WalkCompletion::Cancelled(_) => bail!("a cancelled history delta cannot be finalized"),
    };
    if rechecked.roots != lease.target.roots || rechecked.fingerprint != lease.target.fingerprint {
        set_terminal_state(
            conn,
            lease,
            CoverageState::Stale,
            Some("repository roots changed during history delta"),
        )?;
        return Ok(FinalizeOutcome::RootsChanged);
    }

    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure_active_scan(&tx, lease)?;
    if cleanup_cancelled(cancellation) {
        drop(tx);
        cancel_scan(conn, lease)?;
        return Ok(FinalizeOutcome::Cancelled);
    }
    let staged_count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM history_membership WHERE repo_path = ?1 AND scan_epoch = ?2",
        params![lease.repo_path, lease.scan_epoch],
        |row| row.get(0),
    )?;
    if staged_count != stats.commits as i64 {
        bail!(
            "history delta membership count {} does not match exhausted walk count {}",
            staged_count,
            stats.commits
        );
    }
    let recorded: (i64, i64) = tx.query_row(
        "SELECT commits_indexed, batches_indexed FROM history_coverage WHERE repo_path = ?1",
        params![lease.repo_path],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    if recorded != (stats.commits as i64, stats.batches as i64) {
        bail!(
            "history delta counters {:?} do not match exhausted walk ({}, {})",
            recorded,
            stats.commits,
            stats.batches
        );
    }

    let generation = cache::next_generation(&tx)?;
    cache::record_destructive_generation(&tx, generation)?;
    tx.execute(
        r#"
        INSERT OR IGNORE INTO history_membership(repo_path, scan_epoch, hash)
        SELECT repo_path, ?3, hash
        FROM history_membership
        WHERE repo_path = ?1 AND scan_epoch = ?2
        "#,
        params![lease.repo_path, lease.scan_epoch, lease.published_epoch],
    )?;
    crate::history_decorations::refresh_tip_decorations(
        &tx,
        &lease.repo_path,
        published,
        rechecked,
    )?;
    tx.execute(
        "DELETE FROM history_roots WHERE repo_path = ?1",
        params![lease.repo_path],
    )?;
    {
        let mut insert_root = tx.prepare(
            "INSERT INTO history_roots(repo_path, ref_name, oid, root_kind) VALUES (?1, ?2, ?3, ?4)",
        )?;
        for root in &rechecked.roots {
            insert_root.execute(params![
                lease.repo_path,
                root.ref_name,
                root.oid,
                root.kind.as_str()
            ])?;
        }
    }
    if cleanup_cancelled(cancellation) {
        drop(tx);
        cancel_scan(conn, lease)?;
        return Ok(FinalizeOutcome::Cancelled);
    }
    let total_published: i64 = tx.query_row(
        "SELECT COUNT(*) FROM history_membership WHERE repo_path = ?1 AND scan_epoch = ?2",
        params![lease.repo_path, lease.published_epoch],
        |row| row.get(0),
    )?;
    let now = unix_now();
    let updated = tx.execute(
        r#"
        UPDATE history_coverage SET
            state = 'complete',
            indexed_root_fingerprint = ?3,
            commits_indexed = ?4,
            completed_generation = ?5,
            completed_at = ?6,
            updated_at = ?6,
            last_error = NULL
        WHERE repo_path = ?1 AND state = 'indexing' AND scan_epoch = ?2
          AND published_epoch = ?7
        "#,
        params![
            lease.repo_path,
            lease.scan_epoch,
            rechecked.fingerprint,
            total_published,
            generation,
            now,
            lease.published_epoch
        ],
    )?;
    if updated != 1 {
        bail!("history delta lease changed before publication");
    }
    enqueue_cleanup_job(
        &tx,
        &lease.repo_path,
        "delta",
        lease.published_epoch,
        lease.scan_epoch,
        generation,
    )?;
    tx.commit()?;
    Ok(FinalizeOutcome::Published {
        generation,
        deleted: 0,
        commits: stats.commits,
        maintenance_pending: true,
    })
}

pub(crate) fn published_roots(conn: &Connection, repo_path: &str) -> Result<Vec<HistoryRoot>> {
    let mut statement = conn.prepare(
        "SELECT ref_name, oid, root_kind FROM history_roots WHERE repo_path = ?1 ORDER BY ref_name",
    )?;
    let roots = statement
        .query_map(params![repo_path], |row| {
            let kind: String = row.get(2)?;
            let kind = HistoryRootKind::from_str(&kind).ok_or_else(|| {
                rusqlite::Error::FromSqlConversionFailure(
                    2,
                    rusqlite::types::Type::Text,
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("invalid history root kind {kind:?}"),
                    )),
                )
            })?;
            Ok(HistoryRoot {
                ref_name: row.get(0)?,
                oid: row.get(1)?,
                kind,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(roots)
}

/// Keys visible in one explicitly-pinned publication epoch. Query paths must
/// use this membership (or an equivalent join); FTS presence alone is not
/// proof that a commit is currently reachable.
#[cfg(test)]
pub(crate) fn published_hashes(
    conn: &Connection,
    repo_path: &str,
    published_epoch: i64,
) -> Result<Vec<String>> {
    let mut statement = conn.prepare(
        r#"
        SELECT hash FROM history_membership
        WHERE repo_path = ?1 AND scan_epoch = ?2
        ORDER BY hash
        "#,
    )?;
    let hashes = statement
        .query_map(params![repo_path, published_epoch], |row| row.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(hashes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::git::CommitSummary;
    use crate::history_schema::test_support;

    fn root(name: &str, oid: &str) -> HistoryRoot {
        HistoryRoot {
            ref_name: name.to_string(),
            oid: oid.to_string(),
            kind: HistoryRootKind::Local,
        }
    }

    fn snapshot(name: &str, oid: &str, fingerprint: &str) -> RootSnapshot {
        RootSnapshot {
            roots: vec![root(name, oid)],
            fingerprint: fingerprint.to_string(),
        }
    }

    fn commit(hash: &str, summary: &str) -> HistoryCommit {
        HistoryCommit {
            summary: CommitSummary {
                repo_path: "/repo".to_string(),
                repo_name: "repo".to_string(),
                hash: hash.to_string(),
                short_hash: hash.chars().take(7).collect(),
                summary: summary.to_string(),
                author: "Ada".to_string(),
                email: "ada@example.com".to_string(),
                timestamp: 100,
                branch_label: None,
                is_merge: false,
                is_tagged: false,
                parents: Vec::new(),
                message: summary.to_string(),
                remote_tip_label: None,
                remote_tip_extra_count: 0,
            },
            message_truncated: false,
        }
    }

    fn insert_baseline(conn: &Connection, hash: &str) {
        test_support::insert_commit(conn, hash, "old published", "old published");
        conn.execute(
            "INSERT OR IGNORE INTO history_membership(repo_path, scan_epoch, hash) VALUES ('/repo', 0, ?1)",
            params![hash],
        )
        .unwrap();
        conn.execute(
            r#"
            INSERT OR REPLACE INTO history_coverage(
                repo_path, state, scan_epoch, published_epoch,
                commits_indexed, batches_indexed, updated_at
            ) VALUES ('/repo', 'stale', 0, 0, 1, 0, 1)
            "#,
            [],
        )
        .unwrap();
    }

    fn activate_repo(conn: &Connection) {
        conn.execute(
            r#"
            INSERT OR IGNORE INTO repos(
                path, name, discovered_at, last_seen_at, canonical_path,
                status, user_state, lifecycle_epoch
            ) VALUES ('/repo', 'repo', 1, 1, '/repo', 'active', 'normal', 0)
            "#,
            [],
        )
        .unwrap();
    }

    fn visible_timeline_hashes(conn: &Connection) -> Vec<String> {
        let mut hashes: Vec<String> = crate::cache::list_recent_commits(conn, 0, 100)
            .unwrap()
            .into_iter()
            .map(|commit| commit.hash)
            .collect();
        hashes.sort();
        hashes
    }

    #[test]
    fn upsert_preserves_rowid_and_fts_tracks_changed_text() {
        let mut conn = test_support::v5_connection();
        insert_baseline(&conn, "abcdef123");
        let rowid_before: i64 = conn
            .query_row("SELECT rowid FROM commits", [], |row| row.get(0))
            .unwrap();
        let target = snapshot("refs/heads/main", "1111111", "fp-1");
        let lease = begin_scan(&mut conn, "/repo", target).unwrap();
        upsert_batch(&mut conn, &lease, &[commit("abcdef123", "new optimized")]).unwrap();

        let rowid_after: i64 = conn
            .query_row("SELECT rowid FROM commits", [], |row| row.get(0))
            .unwrap();
        let matches: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM commits_fts WHERE commits_fts MATCH ?1",
                [crate::history_query::substring_expression("optimized").unwrap()],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(rowid_after, rowid_before);
        assert_eq!(matches, 1);
    }

    #[test]
    fn cancellation_keeps_published_snapshot_and_never_sweeps() {
        let mut conn = test_support::v5_connection();
        insert_baseline(&conn, "old1111");
        let target = snapshot("refs/heads/main", "new-tip", "fp-new");
        let lease = begin_scan(&mut conn, "/repo", target).unwrap();
        upsert_batch(&mut conn, &lease, &[commit("new1111", "staged")]).unwrap();
        assert_eq!(
            published_hashes(&conn, "/repo", 0).unwrap(),
            vec!["old1111"]
        );
        assert_eq!(
            visible_timeline_hashes(&conn),
            vec!["old1111"],
            "staging rows never leak through the legacy commits query"
        );

        assert!(cancel_scan(&conn, &lease).unwrap());
        assert_eq!(
            published_hashes(&conn, "/repo", 0).unwrap(),
            vec!["old1111"]
        );
        assert_eq!(visible_timeline_hashes(&conn), vec!["old1111"]);
        assert!(
            !resume_pending_cleanup(&conn, "/repo", None)
                .unwrap()
                .complete
        );
        drain_pending_cleanup(&conn, "/repo").unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1,
            "cancellation removes only never-published staging rows"
        );
        assert_eq!(
            coverage(&conn, "/repo").unwrap().unwrap().state,
            CoverageState::Cancelled
        );
    }

    #[test]
    fn changed_roots_leave_old_publication_visible_and_skip_sweep() {
        let mut conn = test_support::v5_connection();
        insert_baseline(&conn, "old1111");
        let target = snapshot("refs/heads/main", "tip-a", "fp-a");
        let lease = begin_scan(&mut conn, "/repo", target.clone()).unwrap();
        upsert_batch(&mut conn, &lease, &[commit("new1111", "staged")]).unwrap();
        let changed = snapshot("refs/heads/main", "tip-b", "fp-b");
        let result = finalize_scan(
            &mut conn,
            &lease,
            &changed,
            WalkCompletion::Exhausted(WalkStats {
                commits: 1,
                batches: 1,
                max_batch_len: 1,
            }),
            None,
        )
        .unwrap();

        assert_eq!(result, FinalizeOutcome::RootsChanged);
        assert_eq!(
            published_hashes(&conn, "/repo", 0).unwrap(),
            vec!["old1111"]
        );
        assert_eq!(
            visible_timeline_hashes(&conn),
            vec!["old1111"],
            "root-change staging never replaces the published snapshot"
        );
        drain_pending_cleanup(&conn, "/repo").unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            1
        );
        assert_eq!(
            coverage(&conn, "/repo").unwrap().unwrap().state,
            CoverageState::Stale
        );
    }

    #[test]
    fn exhausted_matching_roots_publish_atomically_then_cleanup_deferred_rows() {
        let mut conn = test_support::v5_connection();
        insert_baseline(&conn, "old1111");
        let target = snapshot("refs/heads/main", "tip-a", "fp-a");
        let lease = begin_scan(&mut conn, "/repo", target.clone()).unwrap();
        upsert_batch(&mut conn, &lease, &[commit("new1111", "new truth")]).unwrap();
        assert_eq!(
            published_hashes(&conn, "/repo", 0).unwrap(),
            vec!["old1111"]
        );

        let result = finalize_scan(
            &mut conn,
            &lease,
            &target,
            WalkCompletion::Exhausted(WalkStats {
                commits: 1,
                batches: 1,
                max_batch_len: 1,
            }),
            None,
        )
        .unwrap();
        let FinalizeOutcome::Published { deleted, .. } = result else {
            panic!("expected publication");
        };
        assert_eq!(deleted, 1);
        let state = coverage(&conn, "/repo").unwrap().unwrap();
        assert_eq!(state.state, CoverageState::Complete);
        assert_eq!(state.published_epoch, lease.scan_epoch);
        assert_eq!(
            published_hashes(&conn, "/repo", lease.scan_epoch).unwrap(),
            vec!["new1111"]
        );
        assert_eq!(visible_timeline_hashes(&conn), vec!["new1111"]);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            2,
            "publication switches logical visibility before physical GC"
        );
        assert!(has_pending_cleanup(&conn, "/repo").unwrap());
        drain_pending_cleanup(&conn, "/repo").unwrap();
        assert_eq!(
            conn.query_row("SELECT hash FROM commits", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "new1111"
        );
        assert!(!has_pending_cleanup(&conn, "/repo").unwrap());
        assert_eq!(published_roots(&conn, "/repo").unwrap(), target.roots);
    }

    #[test]
    fn cancelled_walk_is_rejected_by_finalize() {
        let mut conn = test_support::v5_connection();
        activate_repo(&conn);
        let target = snapshot("refs/heads/main", "tip-a", "fp-a");
        let lease = begin_scan(&mut conn, "/repo", target.clone()).unwrap();
        let error = finalize_scan(
            &mut conn,
            &lease,
            &target,
            WalkCompletion::Cancelled(WalkStats::default()),
            None,
        )
        .unwrap_err();
        assert!(error.to_string().contains("cancelled"));
    }

    #[test]
    fn exclusion_restore_aba_invalidates_every_old_scan_write() {
        let mut conn = test_support::v5_connection();
        activate_repo(&conn);
        let target = snapshot("refs/heads/main", "tip-a", "fp-a");
        let lease = begin_scan(&mut conn, "/repo", target).unwrap();

        // Simulate the exclusion transaction (epoch bump + purge), followed
        // by a restore (another bump). The boolean state ends active again,
        // but the pre-exclusion lease must stay permanently invalid.
        conn.execute(
            "UPDATE repos SET user_state = 'removed', lifecycle_epoch = lifecycle_epoch + 1 WHERE path = '/repo'",
            [],
        )
        .unwrap();
        conn.execute("DELETE FROM commits WHERE repo_path = '/repo'", [])
            .unwrap();
        conn.execute(
            "UPDATE repos SET user_state = 'normal', lifecycle_epoch = lifecycle_epoch + 1 WHERE path = '/repo'",
            [],
        )
        .unwrap();

        let error =
            upsert_batch(&mut conn, &lease, &[commit("late1111", "late write")]).unwrap_err();
        assert!(error.to_string().contains("no longer active"));
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM commits", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0,
            "a pre-exclusion worker cannot resurrect rows after purge"
        );
    }

    #[test]
    fn delta_publication_invalidates_a_reader_pinned_before_membership_changed() {
        let mut conn = test_support::v5_connection();
        insert_baseline(&conn, "old1111");
        let published = snapshot("refs/heads/main", "tip-a", "fp-a");
        let first = begin_scan(&mut conn, "/repo", published.clone()).unwrap();
        upsert_batch(&mut conn, &first, &[commit("old1111", "old published")]).unwrap();
        finalize_scan(
            &mut conn,
            &first,
            &published,
            WalkCompletion::Exhausted(WalkStats {
                commits: 1,
                batches: 1,
                max_batch_len: 1,
            }),
            None,
        )
        .unwrap();

        let newly_cached = commit("new2222", "new cached before publication").summary;
        crate::cache::upsert_commits(&mut conn, &[newly_cached]).unwrap();
        let pinned_generation = crate::cache::current_generation(&conn).unwrap();
        let pinned = crate::cache::TimelineFilters {
            view_generation: Some(pinned_generation),
            ..Default::default()
        };
        crate::cache::ensure_timeline_snapshot_valid(&conn, &pinned).unwrap();
        assert_eq!(visible_timeline_hashes(&conn), vec!["old1111"]);

        let target = snapshot("refs/heads/main", "tip-b", "fp-b");
        let delta = begin_scan(&mut conn, "/repo", target.clone()).unwrap();
        upsert_batch(
            &mut conn,
            &delta,
            &[commit("new2222", "new cached before publication")],
        )
        .unwrap();
        finalize_delta_scan(
            &mut conn,
            &delta,
            &published,
            &target,
            WalkCompletion::Exhausted(WalkStats {
                commits: 1,
                batches: 1,
                max_batch_len: 1,
            }),
            None,
        )
        .unwrap();

        assert!(crate::cache::ensure_timeline_snapshot_valid(&conn, &pinned).is_err());
        assert_eq!(visible_timeline_hashes(&conn), vec!["new2222", "old1111"]);
    }

    #[test]
    fn discard_cleanup_never_deletes_a_row_in_current_published_membership() {
        let conn = test_support::v5_connection();
        activate_repo(&conn);
        test_support::insert_commit(&conn, "published1", "published", "body");
        conn.execute(
            "UPDATE commits SET history_scan_epoch = 5 WHERE hash = 'published1'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO history_coverage (repo_path, state, scan_epoch, published_epoch, \
                 target_lifecycle_epoch, commits_indexed, batches_indexed, updated_at) \
             VALUES('/repo', 'complete', 5, 3, 0, 1, 1, 1)",
            [],
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO history_membership VALUES('/repo', 3, 'published1'); \
             INSERT INTO history_membership VALUES('/repo', 5, 'published1');",
        )
        .unwrap();
        enqueue_cleanup_job(&conn, "/repo", "discard", 3, 5, 0).unwrap();

        let progress = resume_pending_cleanup(&conn, "/repo", None).unwrap();
        assert!(!progress.complete);
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM commits WHERE hash = 'published1'",
                [],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            1
        );
        drain_pending_cleanup(&conn, "/repo").unwrap();
        assert_eq!(visible_timeline_hashes(&conn), vec!["published1"]);
    }
}
