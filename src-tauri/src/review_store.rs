//! Local review metadata, separate from disposable timeline/diff caches.
use crate::review_types::ReviewBaseline;
use anyhow::{bail, Result};
use rusqlite::{params, Connection, OptionalExtension};

pub fn initialize(conn: &Connection) -> Result<()> {
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS reviewed_baselines (
        repo_path TEXT NOT NULL, branch TEXT NOT NULL, identity TEXT NOT NULL,
        head TEXT NOT NULL, PRIMARY KEY(repo_path, branch));",
    )?;
    Ok(())
}

pub fn load(conn: &Connection, path: &str, branch: &str) -> Result<Option<ReviewBaseline>> {
    Ok(conn
        .query_row(
            "SELECT identity, head FROM reviewed_baselines WHERE repo_path=?1 AND branch=?2",
            params![path, branch],
            |row| {
                Ok(ReviewBaseline {
                    identity: row.get(0)?,
                    head: row.get(1)?,
                })
            },
        )
        .optional()?)
}

pub fn save(
    conn: &mut Connection,
    path: &str,
    branch: &str,
    expected: Option<&ReviewBaseline>,
    next: &ReviewBaseline,
) -> Result<()> {
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
    if load(&tx, path, branch)?.as_ref() != expected {
        bail!("Review baseline changed in another window. Refresh before marking reviewed.");
    }
    tx.execute(
        "INSERT INTO reviewed_baselines(repo_path,branch,identity,head) VALUES(?1,?2,?3,?4)
        ON CONFLICT(repo_path,branch) DO UPDATE SET identity=excluded.identity,head=excluded.head",
        params![path, branch, next.identity, next.head],
    )?;
    tx.commit()?;
    Ok(())
}
