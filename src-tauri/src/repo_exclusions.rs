use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use crate::{cache, discovery_orchestrator, git, watcher};

const RESTORE_WINDOW_SECONDS: i64 = 7 * 86_400;
const RESTORE_COMMIT_CAP: usize = 100;

/// A recoverable repository exclusion. The repository row, pin preference,
/// branch selection and folder are deliberately retained; only discovery,
/// watching and cached timeline rows are suspended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedRepo {
    pub path: String,
    pub name: String,
    pub excluded_at: i64,
    pub folder_available: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepoStatusPayload {
    canonical_path: String,
    status: &'static str,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepoCatalogChangedPayload {
    canonical_path: String,
    change: &'static str,
}

pub fn list_excluded_repos(app: &AppHandle) -> Result<Vec<ExcludedRepo>> {
    let conn = cache::open(app)?;
    list_excluded_repos_in(&conn)
}

/// Exclude a repository from gitwink without deleting its folder or durable
/// preferences. The `removed` database value is retained for downgrade
/// compatibility with older gitwink releases; product/API terminology is
/// `excluded` because this is a reversible ignore operation.
pub fn exclude_repo(app: &AppHandle, canonical_path: &str) -> Result<()> {
    let mut conn = cache::open(app)?;
    let now = unix_now();
    exclude_repo_in(&mut conn, canonical_path, now)?;

    // Timeline rows are disposable cache. Purging them makes the exclusion
    // immediate; restore repopulates the active seven-day window.
    let removed =
        cache::delete_repo_commits(&mut conn, canonical_path).unwrap_or(cache::UpsertOutcome {
            generation: 0,
            inserted: 0,
            deleted: 0,
        });
    if let Some(w) = app.try_state::<watcher::RepoWatcher>() {
        w.remove(Path::new(canonical_path));
    }

    emit_repo_status(app, canonical_path, "removed");
    emit_catalog_changed(app, canonical_path, "excluded");
    if removed.deleted > 0 {
        let _ = app.emit(
            "timeline://invalidated",
            cache::TimelineInvalidated {
                generation: removed.generation,
                inserted: 0,
                deleted: removed.deleted,
                repo_path: canonical_path.to_string(),
            },
        );
    }
    let _ = discovery_orchestrator::append_scan_log(
        app,
        &format!("{now} user_exclude: {canonical_path}"),
    );
    Ok(())
}

/// Restore an excluded repository. An available path must still be a Git
/// working tree before the exclusion is lifted. A currently unavailable path
/// can still be restored: it returns as `missing` and the existing lifecycle
/// reconciler activates it when the drive/folder comes back.
pub fn restore_repo(app: &AppHandle, canonical_path: &str) -> Result<()> {
    let path = Path::new(canonical_path);
    let folder_available = path.is_dir();
    if folder_available {
        validate_restorable_worktree(path)?;
    }

    let now = unix_now();
    let name = {
        let mut conn = cache::open(app)?;
        restore_repo_in(&mut conn, canonical_path, folder_available, now)?
    };

    if folder_available {
        refill_restored_repo(app, canonical_path, &name);
        if let Some(w) = app.try_state::<watcher::RepoWatcher>() {
            w.add(path);
        }
    }

    let status = if folder_available {
        "active"
    } else {
        "missing"
    };
    emit_repo_status(app, canonical_path, status);
    emit_catalog_changed(app, canonical_path, "restored");
    let _ = discovery_orchestrator::append_scan_log(
        app,
        &format!("{now} user_restore: {canonical_path} status={status}"),
    );
    Ok(())
}

fn list_excluded_repos_in(conn: &Connection) -> Result<Vec<ExcludedRepo>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT r.canonical_path, r.name,
               COALESCE(r.removed_at, t.removed_at, 0)
        FROM repos r
        LEFT JOIN discovery_tombstones t
          ON t.canonical_path = r.canonical_path
        WHERE r.user_state = 'removed'
        ORDER BY COALESCE(r.removed_at, t.removed_at, 0) DESC,
                 r.name COLLATE NOCASE
        "#,
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    Ok(rows
        .into_iter()
        .map(|(path, name, excluded_at)| ExcludedRepo {
            folder_available: Path::new(&path).is_dir(),
            path,
            name,
            excluded_at,
        })
        .collect())
}

fn exclude_repo_in(conn: &mut Connection, canonical_path: &str, now: i64) -> Result<()> {
    let tx = conn.transaction()?;
    let repo = tx
        .query_row(
            "SELECT name, primary_source FROM repos \
             WHERE canonical_path = ?1 AND user_state != 'removed'",
            params![canonical_path],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    let Some((name, source)) = repo else {
        anyhow::bail!("repository is not active: {canonical_path}");
    };

    tx.execute(
        "UPDATE repos SET user_state = 'removed', removed_at = ?1 \
         WHERE canonical_path = ?2",
        params![now, canonical_path],
    )?;
    tx.execute(
        r#"
        INSERT INTO discovery_tombstones (
            canonical_path, removed_at, reason,
            last_known_name, last_known_source
        )
        VALUES (?1, ?2, 'user_exclude', ?3, ?4)
        ON CONFLICT(canonical_path) DO UPDATE SET
            removed_at = excluded.removed_at,
            reason = excluded.reason,
            last_known_name = excluded.last_known_name,
            last_known_source = excluded.last_known_source
        "#,
        params![canonical_path, now, name, source],
    )?;
    tx.commit()?;
    Ok(())
}

fn restore_repo_in(
    conn: &mut Connection,
    canonical_path: &str,
    folder_available: bool,
    now: i64,
) -> Result<String> {
    let tx = conn.transaction()?;
    let name = tx
        .query_row(
            "SELECT name FROM repos \
             WHERE canonical_path = ?1 AND user_state = 'removed'",
            params![canonical_path],
            |row| row.get::<_, String>(0),
        )
        .optional()?;
    let Some(name) = name else {
        anyhow::bail!("repository is not excluded: {canonical_path}");
    };

    let status = if folder_available {
        "active"
    } else {
        "missing"
    };
    let missing_since = if folder_available { None } else { Some(now) };
    tx.execute(
        "UPDATE repos SET user_state = 'pinned', removed_at = NULL, \
         status = ?1, missing_since = ?2, last_verified_at = ?3 \
         WHERE canonical_path = ?4",
        params![status, missing_since, now, canonical_path],
    )?;
    tx.execute(
        "DELETE FROM discovery_tombstones WHERE canonical_path = ?1",
        params![canonical_path],
    )?;
    tx.commit()?;
    Ok(name)
}

fn validate_restorable_worktree(path: &Path) -> Result<()> {
    let repo = git2::Repository::discover(path)
        .with_context(|| format!("Not a Git working tree: {}", path.display()))?;
    let workdir = repo
        .workdir()
        .with_context(|| format!("Not a Git working tree: {}", path.display()))?;
    let expected = std::fs::canonicalize(path)
        .with_context(|| format!("Cannot resolve repository path: {}", path.display()))?;
    let actual = std::fs::canonicalize(workdir)
        .with_context(|| format!("Cannot resolve Git worktree: {}", workdir.display()))?;
    if actual != expected {
        anyhow::bail!(
            "Folder is no longer the excluded Git working tree: {}",
            path.display()
        );
    }
    Ok(())
}

fn refill_restored_repo(app: &AppHandle, canonical_path: &str, name: &str) {
    let cutoff = unix_now() - RESTORE_WINDOW_SECONDS;
    let commits = git::recent_commits(Path::new(canonical_path), RESTORE_COMMIT_CAP, cutoff)
        .unwrap_or_default()
        .into_iter()
        .map(|mut commit| {
            commit.repo_name = name.to_string();
            commit
        })
        .collect::<Vec<_>>();
    if commits.is_empty() {
        return;
    }
    let outcome = cache::open(app)
        .ok()
        .and_then(|mut conn| cache::upsert_commits(&mut conn, &commits).ok());
    if let Some(outcome) = outcome {
        let _ = app.emit(
            "timeline://invalidated",
            cache::TimelineInvalidated {
                generation: outcome.generation,
                inserted: outcome.inserted,
                deleted: 0,
                repo_path: canonical_path.to_string(),
            },
        );
    }
}

fn emit_repo_status(app: &AppHandle, canonical_path: &str, status: &'static str) {
    let _ = app.emit(
        "timeline://repo-status",
        RepoStatusPayload {
            canonical_path: canonical_path.to_string(),
            status,
        },
    );
}

fn emit_catalog_changed(app: &AppHandle, canonical_path: &str, change: &'static str) {
    let _ = app.emit(
        "timeline://repo-catalog-changed",
        RepoCatalogChangedPayload {
            canonical_path: canonical_path.to_string(),
            change,
        },
    );
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE repos (
                canonical_path TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                user_state TEXT NOT NULL DEFAULT 'normal',
                primary_source TEXT NOT NULL DEFAULT 'fs_walk',
                removed_at INTEGER,
                missing_since INTEGER,
                last_verified_at INTEGER
            );
            CREATE TABLE discovery_tombstones (
                canonical_path TEXT PRIMARY KEY,
                removed_at INTEGER NOT NULL,
                reason TEXT NOT NULL,
                last_known_name TEXT,
                last_known_source TEXT
            );
            "#,
        )
        .unwrap();
        conn
    }

    fn insert_repo(conn: &Connection, path: &str, name: &str) {
        conn.execute(
            "INSERT INTO repos (canonical_path, name) VALUES (?1, ?2)",
            params![path, name],
        )
        .unwrap();
    }

    #[test]
    fn exclusion_is_recoverable_and_keeps_metadata() {
        let mut conn = test_db();
        insert_repo(&conn, "/repos/alpha", "alpha");

        exclude_repo_in(&mut conn, "/repos/alpha", 123).unwrap();

        let row: (String, Option<i64>) = conn
            .query_row(
                "SELECT user_state, removed_at FROM repos WHERE canonical_path = ?1",
                ["/repos/alpha"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, ("removed".to_string(), Some(123)));

        let tombstone: (String, String) = conn
            .query_row(
                "SELECT reason, last_known_name FROM discovery_tombstones \
                 WHERE canonical_path = ?1",
                ["/repos/alpha"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(tombstone, ("user_exclude".to_string(), "alpha".to_string()));

        let excluded = list_excluded_repos_in(&conn).unwrap();
        assert_eq!(excluded.len(), 1);
        assert_eq!(excluded[0].path, "/repos/alpha");
        assert_eq!(excluded[0].excluded_at, 123);
    }

    #[test]
    fn restoring_available_repo_clears_tombstone_and_reactivates() {
        let mut conn = test_db();
        insert_repo(&conn, "/repos/alpha", "alpha");
        exclude_repo_in(&mut conn, "/repos/alpha", 123).unwrap();

        let name = restore_repo_in(&mut conn, "/repos/alpha", true, 456).unwrap();

        assert_eq!(name, "alpha");
        let row: (String, String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT user_state, status, removed_at, missing_since FROM repos \
                 WHERE canonical_path = ?1",
                ["/repos/alpha"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            row,
            ("pinned".to_string(), "active".to_string(), None, None)
        );
        let tombstones: i64 = conn
            .query_row("SELECT COUNT(*) FROM discovery_tombstones", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(tombstones, 0);
    }

    #[test]
    fn restoring_unavailable_repo_returns_it_as_missing() {
        let mut conn = test_db();
        insert_repo(&conn, "/repos/offline", "offline");
        exclude_repo_in(&mut conn, "/repos/offline", 123).unwrap();

        restore_repo_in(&mut conn, "/repos/offline", false, 456).unwrap();

        let row: (String, Option<i64>) = conn
            .query_row(
                "SELECT status, missing_since FROM repos WHERE canonical_path = ?1",
                ["/repos/offline"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(row, ("missing".to_string(), Some(456)));
    }

    #[test]
    fn restore_validation_requires_the_exact_worktree_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        git2::Repository::init(&root).unwrap();
        let child = root.join("nested");
        std::fs::create_dir(&child).unwrap();

        validate_restorable_worktree(&root).unwrap();
        assert!(validate_restorable_worktree(&child).is_err());
    }
}
