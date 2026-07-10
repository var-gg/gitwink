use std::collections::HashSet;
use std::path::Path;
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::{
    cache, discovery_orchestrator, git, history_git, history_indexer, history_store, settings,
    watcher,
};

const RESTORE_WINDOW_SECONDS: i64 = 7 * 86_400;
const RESTORE_COMMIT_CAP: usize = 100;
/// Two failed probes must be separated enough that they are independent OS /
/// network observations, but close enough to still be consecutive evidence.
/// This prevents a one-off SMB or removable-drive timeout from hiding a repo.
pub(crate) const MISSING_CONFIRMATION_GRACE_SECONDS: i64 = 2;
const MISSING_CONFIRMATION_WINDOW_SECONDS: i64 = 5 * 60;

/// Serialize lifecycle work for one physical repository without forcing an
/// unrelated repository to wait for cache purges, network-path watcher I/O,
/// or frontend event delivery. The global lifecycle mutex remains the short
/// database CAS edge; this identity-scoped guard prevents a restore from
/// overtaking an exclusion after that edge has committed (and vice versa).
static REPO_OPERATIONS: LazyLock<(Mutex<HashSet<String>>, Condvar)> =
    LazyLock::new(|| (Mutex::new(HashSet::new()), Condvar::new()));

struct RepoOperationGuard {
    identity_key: String,
}

fn lock_repo_operation(raw_path: &str) -> RepoOperationGuard {
    let identity_key = discovery_orchestrator::repo_identity_match_key(raw_path);
    let (active, ready) = &*REPO_OPERATIONS;
    let mut active = active
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    while active.contains(&identity_key) {
        active = ready
            .wait(active)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
    active.insert(identity_key.clone());
    RepoOperationGuard { identity_key }
}

impl Drop for RepoOperationGuard {
    fn drop(&mut self) {
        let (active, ready) = &*REPO_OPERATIONS;
        let mut active = active
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        active.remove(&self.identity_key);
        ready.notify_all();
    }
}

/// A recoverable repository exclusion. The repository row, pin preference,
/// branch selection and folder are deliberately retained; only discovery,
/// watching and cached timeline rows are suspended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExcludedRepo {
    pub path: String,
    pub name: String,
    pub excluded_at: i64,
    /// Listing Settings must never touch a possibly-dead SMB/removable path.
    /// `None` means availability is intentionally checked only when Restore
    /// is pressed, where that filesystem cost is an expected user action.
    pub folder_available: Option<bool>,
}

/// Exact repository/lifecycle snapshot reviewed by the user before the
/// destructive "use current repo" confirmation. Confirm never accepts only a
/// path: if the folder changes again, the CAS expires and Settings must show a
/// fresh candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplacementConfirmation {
    pub canonical_path: String,
    pub repo_name: String,
    pub storage_identity: String,
    pub refs_fingerprint: String,
    pub lifecycle_epoch: i64,
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
    let _operation = lock_repo_operation(canonical_path);
    let initial_conn = cache::open(app)?;
    let canonical_path = resolve_non_removed_repo_identity(&initial_conn, canonical_path)?
        .ok_or_else(|| anyhow::anyhow!("repository is not available: {canonical_path}"))?;
    let lifecycle_fence = discovery_orchestrator::capture_non_removed_repo_lifecycle_fence(
        &initial_conn,
        &canonical_path,
    )?
    .ok_or_else(|| anyhow::anyhow!("repository is not available: {canonical_path}"))?;
    let repo_status: String = initial_conn.query_row(
        "SELECT status FROM repos WHERE canonical_path = ?1",
        params![&canonical_path],
        |row| row.get(0),
    )?;
    let stored_identity = repo_storage_identity_in(&initial_conn, &canonical_path)?;
    let storage_identity = stored_identity.or_else(|| {
        (repo_status == "active")
            .then(|| git::repo_storage_identity(Path::new(&canonical_path)).ok())
            .flatten()
    });
    // Legacy repo_id=0 aliases require a bounded physical purge, but finding
    // them can scan an old cache. Keep that read outside the global lifecycle
    // CAS; modern writers always attach repo_id, so this legacy set cannot
    // grow between the read and transition.
    let cache_aliases = unresolved_commit_aliases_in(&initial_conn, &canonical_path)?;
    drop(initial_conn);

    let now = unix_now();
    let affected_paths = {
        // Only the durable compare-and-swap transition is globally
        // serialized. Slow cache cleanup, watcher I/O and events happen after
        // this guard is dropped; `_operation` keeps the same physical repo
        // from starting a restore until that cleanup has finished.
        let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
        let mut conn = cache::open(app)?;
        if !discovery_orchestrator::repo_lifecycle_fence_matches(&conn, &lifecycle_fence)? {
            anyhow::bail!("Repository lifecycle changed while excluding; retry");
        }
        if let Some(identity) = storage_identity {
            conn.execute(
                "UPDATE repos SET repo_storage_identity = COALESCE(repo_storage_identity, ?1) \
                 WHERE canonical_path = ?2",
                params![identity, &canonical_path],
            )?;
        }
        exclude_repo_in_with_cache_aliases(&mut conn, &canonical_path, now, &cache_aliases)?
    };

    if let Some(indexer) = app.try_state::<Arc<history_indexer::HistoryIndexer>>() {
        for path in &affected_paths {
            indexer.cancel(path);
        }
    }

    // Timeline rows are disposable cache. The catalog gate makes exclusion
    // immediate; bounded cleanup later reclaims the now-invisible rows, and a
    // restore repopulates the active window. Cleanup is best-effort after the
    // durable exclusion commit: reporting the command as failed here would
    // invite a retry that can only say "not active".
    match cache::open(app) {
        Ok(mut conn) => {
            for path in &affected_paths {
                if let Err(error) = history_store::forget_repo(&mut conn, path) {
                    eprintln!(
                        "failed to forget excluded repository coverage for {path}: {error:#}"
                    );
                }
            }
        }
        Err(error) => {
            eprintln!("failed to reopen cache for excluded repository cleanup: {error:#}");
        }
    }

    for path in &affected_paths {
        watcher::reconcile_repo_registration(app, path);
        emit_repo_status(app, path, "removed");
        if let Some(indexer) = app.try_state::<Arc<history_indexer::HistoryIndexer>>() {
            let _ = indexer.request_cleanup(path.clone());
        }
    }
    emit_catalog_changed(app, &canonical_path, "excluded");
    let _ = discovery_orchestrator::append_scan_log(
        app,
        &format!(
            "{now} user_exclude: {canonical_path} identity_rows={}",
            affected_paths.len()
        ),
    );
    Ok(())
}

/// Restore an excluded repository. An available path must still be a Git
/// working tree before the exclusion is lifted. A currently unavailable path
/// can still be restored: it returns as `missing` and the existing lifecycle
/// reconciler activates it when the drive/folder comes back.
pub fn restore_repo(app: &AppHandle, canonical_path: &str) -> Result<()> {
    restore_repo_impl(app, canonical_path, false, None)
}

/// Explicitly accept the Git repository currently occupying an excluded
/// path as a new identity. This never modifies the folder; it discards only
/// gitwink's old cache/provenance/preferences for that path.
pub fn inspect_replacement_repo(
    app: &AppHandle,
    canonical_path: &str,
) -> Result<ReplacementConfirmation> {
    let _operation = lock_repo_operation(canonical_path);
    let conn = cache::open(app)?;
    let canonical_path = resolve_removed_repo_identity(&conn, canonical_path)?
        .ok_or_else(|| anyhow::anyhow!("repository is not excluded: {canonical_path}"))?;
    let lifecycle_epoch: i64 = conn.query_row(
        "SELECT lifecycle_epoch FROM repos WHERE canonical_path = ?1 AND user_state = 'removed'",
        params![&canonical_path],
        |row| row.get(0),
    )?;
    drop(conn);
    let path = Path::new(&canonical_path);
    validate_restorable_worktree(path, &canonical_path)?;
    let (storage_identity, refs_fingerprint) = capture_stable_replacement_identity(path)?;
    let repo_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("repo")
        .to_string();
    Ok(ReplacementConfirmation {
        canonical_path,
        repo_name,
        storage_identity,
        refs_fingerprint,
        lifecycle_epoch,
    })
}

pub fn replace_excluded_repo(
    app: &AppHandle,
    confirmation: &ReplacementConfirmation,
) -> Result<()> {
    restore_repo_impl(app, &confirmation.canonical_path, true, Some(confirmation))
}

fn restore_repo_impl(
    app: &AppHandle,
    canonical_path: &str,
    replace_identity: bool,
    confirmation: Option<&ReplacementConfirmation>,
) -> Result<()> {
    let _operation = lock_repo_operation(canonical_path);
    let initial_conn = cache::open(app)?;
    let canonical_path = resolve_removed_repo_identity(&initial_conn, canonical_path)?
        .ok_or_else(|| anyhow::anyhow!("repository is not excluded: {canonical_path}"))?;
    let lifecycle_fence =
        discovery_orchestrator::capture_repo_lifecycle_fence(&initial_conn, &canonical_path)?
            .ok_or_else(|| anyhow::anyhow!("repository is not excluded: {canonical_path}"))?;
    if let Some(confirmation) = confirmation {
        let current_epoch: i64 = initial_conn.query_row(
            "SELECT lifecycle_epoch FROM repos WHERE canonical_path = ?1 AND user_state = 'removed'",
            params![&canonical_path],
            |row| row.get(0),
        )?;
        if current_epoch != confirmation.lifecycle_epoch
            || discovery_orchestrator::repo_identity_match_key(&canonical_path)
                != discovery_orchestrator::repo_identity_match_key(&confirmation.canonical_path)
        {
            anyhow::bail!(
                "REPLACEMENT_CONFIRMATION_EXPIRED: Repository lifecycle changed; review it again"
            );
        }
    }
    let equivalent_rows = equivalent_repo_rows_in(&initial_conn, &canonical_path)?;
    let expected_storage_identity = if replace_identity {
        None
    } else {
        equivalent_storage_identity(&equivalent_rows)?
    };
    let mut equivalent_paths = equivalent_rows
        .into_iter()
        .filter(|row| row.user_state == "removed")
        .map(|row| row.path)
        .collect::<Vec<_>>();
    for path in equivalent_tombstone_paths_in(&initial_conn, &canonical_path)? {
        if !equivalent_paths.contains(&path) {
            equivalent_paths.push(path);
        }
    }
    let path = Path::new(&canonical_path);
    let folder_available = path.is_dir();
    if replace_identity && !folder_available {
        anyhow::bail!("The replacement folder must be available and contain a Git repository");
    }
    let prepared_commits = if folder_available {
        validate_restorable_worktree(path, &canonical_path)?;
        let current_storage_identity = git::repo_storage_identity(path)?;
        if let Some(confirmation) = confirmation {
            let current_refs = git::repo_refs_fingerprint(path, None)?;
            if current_storage_identity != confirmation.storage_identity
                || current_refs != confirmation.refs_fingerprint
            {
                anyhow::bail!(
                    "REPLACEMENT_CONFIRMATION_EXPIRED: Folder contents changed; review the current repository again"
                );
            }
        }
        if expected_storage_identity
            .as_ref()
            .is_some_and(|expected| expected != &current_storage_identity)
        {
            anyhow::bail!(
                "REPOSITORY_IDENTITY_MISMATCH: Folder now contains a different Git repository"
            );
        }
        let repo_name = if replace_identity {
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("repo")
                .to_string()
        } else {
            repo_name_in(&initial_conn, &canonical_path)?
        };
        let (commits, refs_fingerprint) = read_recent_repo_commits_stable(path, repo_name.clone())?;
        Some((
            commits,
            refs_fingerprint,
            current_storage_identity,
            repo_name,
        ))
    } else {
        None
    };
    drop(initial_conn);

    // These are slow filesystem/Git proofs. They deliberately run before the
    // global lifecycle edge; the epoch fence below still rejects an exclude /
    // restore ABA while unrelated repositories are never blocked on SMB I/O.
    if folder_available {
        validate_restorable_worktree(Path::new(&canonical_path), &canonical_path)?;
    }
    if let Some((_, expected_refs, current_storage_identity, _)) = &prepared_commits {
        if (!replace_identity
            && expected_storage_identity
                .as_ref()
                .is_some_and(|expected| expected != current_storage_identity))
            || git::repo_storage_identity(Path::new(&canonical_path))
                .ok()
                .as_ref()
                != Some(current_storage_identity)
            || git::repo_refs_fingerprint(Path::new(&canonical_path), None)
                .ok()
                .as_ref()
                != Some(expected_refs)
        {
            anyhow::bail!("Repository changed while restore was loading; retry");
        }
    }

    // Purge while the whole normalized identity is still durably excluded.
    // This can delete a large history cache and must not hold the global
    // lifecycle mutex. The identity-scoped operation guard prevents another
    // restore from activating the repository before cleanup completes.
    let mut cleanup_conn = cache::open(app)?;
    for path in &equivalent_paths {
        history_store::forget_repo(&mut cleanup_conn, path)
            .with_context(|| format!("resetting stale repository history coverage: {path}"))?;
        history_store::drain_pending_cleanup(&cleanup_conn, path)
            .with_context(|| format!("purging stale repository history before restore: {path}"))?;
    }
    drop(cleanup_conn);

    if replace_identity {
        let mut preference_paths = equivalent_paths.clone();
        if !preference_paths.contains(&canonical_path) {
            preference_paths.push(canonical_path.clone());
        }
        settings::clear_repo_preferences(app, &preference_paths).context(
            "Couldn't reset the old repository's pin and branch preferences; it remains excluded",
        )?;
    }

    let now = unix_now();
    let (canonical_path, refill) = {
        // State activation plus its bounded (<=100 row) prepared refill is the
        // only globally serialized restore edge. No filesystem access occurs
        // here; epoch fencing rejects an exclude/restore ABA after the slow
        // validation above.
        let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
        let mut conn = cache::open(app)?;
        let canonical_path = resolve_removed_repo_identity(&conn, &canonical_path)?
            .ok_or_else(|| anyhow::anyhow!("repository is not excluded: {canonical_path}"))?;
        if !discovery_orchestrator::repo_lifecycle_fence_matches(&conn, &lifecycle_fence)? {
            anyhow::bail!("Repository changed while restore was loading; retry");
        }
        let observed_storage_identity = prepared_commits
            .as_ref()
            .map(|(_, _, storage_identity, _)| storage_identity.as_str());
        let replacement_name = replace_identity.then(|| {
            prepared_commits
                .as_ref()
                .map(|(_, _, _, repo_name)| repo_name.as_str())
                .unwrap_or("repo")
        });
        restore_repo_in(
            &mut conn,
            &canonical_path,
            folder_available,
            now,
            observed_storage_identity,
            replacement_name,
        )?;

        let refill = if let Some((commits, _, _, _)) = prepared_commits {
            match persist_recent_repo_commits(&mut conn, commits) {
                Ok(outcome) => outcome,
                Err(error) => {
                    // A failed cache transaction must not leave Settings
                    // claiming the repository is fully restored. Recreate the
                    // recoverable exclusion atomically and let the user retry.
                    exclude_repo_in(&mut conn, &canonical_path, now)?;
                    return Err(error);
                }
            }
        } else {
            None
        };
        (canonical_path, refill)
    };

    watcher::reconcile_repo_registration(app, &canonical_path);

    let status = if folder_available {
        "active"
    } else {
        "missing"
    };
    emit_repo_status(app, &canonical_path, status);
    emit_catalog_changed(
        app,
        &canonical_path,
        if replace_identity {
            "replaced"
        } else {
            "restored"
        },
    );
    emit_refill_invalidated(app, &canonical_path, refill);
    let _ = discovery_orchestrator::append_scan_log(
        app,
        &format!(
            "{now} {}: {canonical_path} status={status}",
            if replace_identity {
                "user_replace_identity"
            } else {
                "user_restore"
            }
        ),
    );
    Ok(())
}

/// Mark an active repository missing and synchronously detach its watcher.
/// The lifecycle lock also drains any watcher refresh that was already in
/// flight, so no later cache write can race this state transition.
pub(crate) fn mark_repo_missing(app: &AppHandle, canonical_path: &str) -> Result<bool> {
    let _operation = lock_repo_operation(canonical_path);
    let canonical_path = {
        let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
        let conn = cache::open(app)?;
        let Some(canonical_path) = resolve_active_repo_identity(&conn, canonical_path)? else {
            return Ok(false);
        };
        let now = unix_now();
        if observe_repo_unavailable_in(&conn, &canonical_path, now)?
            != MissingObservation::Confirmed
        {
            return Ok(false);
        }
        canonical_path
    };
    watcher::reconcile_repo_registration(app, &canonical_path);
    // Discovery/manual registration does not take the identity-scoped guard
    // and can legitimately reactivate a path after our state edge. Serialize
    // only this final exact-state check + queued event so a stale "missing"
    // payload can never land after the newer active transition. Watcher I/O
    // remains outside the global lock.
    {
        let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
        let conn = cache::open(app)?;
        let still_missing: i64 = conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM repos WHERE canonical_path = ?1 \
             AND status = 'missing' AND user_state != 'removed')",
            params![&canonical_path],
            |row| row.get(0),
        )?;
        if still_missing != 0 {
            emit_repo_status(app, &canonical_path, "missing");
        }
    }
    Ok(true)
}

/// Recover a missing row only when the original path is once again the exact
/// Git working-tree root. State transition, cache refill, watcher attachment
/// and frontend events are one serialized edge; an empty/replaced directory
/// stays missing and emits nothing.
pub(crate) fn recover_missing_repo(app: &AppHandle, canonical_path: &str) -> Result<bool> {
    let _operation = lock_repo_operation(canonical_path);
    let initial_conn = cache::open(app)?;
    let Some(canonical_path) = resolve_missing_repo_identity(&initial_conn, canonical_path)? else {
        return Ok(false);
    };
    let Some(lifecycle_fence) =
        discovery_orchestrator::capture_repo_lifecycle_fence(&initial_conn, &canonical_path)?
    else {
        return Ok(false);
    };
    let path = Path::new(&canonical_path);
    if !path.is_dir() {
        return Ok(false);
    }
    validate_restorable_worktree(path, &canonical_path)?;
    let expected_storage_identity = repo_storage_identity_in(&initial_conn, &canonical_path)?;
    let current_storage_identity = git::repo_storage_identity(path)?;
    if expected_storage_identity
        .as_ref()
        .is_some_and(|expected| expected != &current_storage_identity)
    {
        return Ok(false);
    }
    let name = repo_name_in(&initial_conn, &canonical_path)?;
    let (commits, refs_fingerprint) = read_recent_repo_commits_stable(path, name)?;
    let observed_history_roots = history_git::snapshot_roots(path).ok();
    drop(initial_conn);

    // As with explicit restore, Git/OS revalidation is outside the global
    // lifecycle lock. The final epoch comparison is the serialized CAS edge.
    let path = Path::new(&canonical_path);
    validate_restorable_worktree(path, &canonical_path)?;
    if git::repo_storage_identity(path).ok().as_ref() != Some(&current_storage_identity)
        || git::repo_refs_fingerprint(path, None).ok().as_ref() != Some(&refs_fingerprint)
    {
        return Ok(false);
    }

    let now = unix_now();
    let (canonical_path, refill, history_changed) = {
        let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
        let mut conn = cache::open(app)?;
        let Some(canonical_path) = resolve_missing_repo_identity(&conn, &canonical_path)? else {
            return Ok(false);
        };
        if !discovery_orchestrator::repo_lifecycle_fence_matches(&conn, &lifecycle_fence)? {
            return Ok(false);
        }

        if !activate_missing_repo_in(&conn, &canonical_path, now)? {
            return Ok(false);
        }
        conn.execute(
            "UPDATE repos SET repo_storage_identity = COALESCE(repo_storage_identity, ?1) \
             WHERE canonical_path = ?2",
            params![current_storage_identity, &canonical_path],
        )?;
        let refill = match persist_recent_repo_commits(&mut conn, commits) {
            Ok(outcome) => outcome,
            Err(error) => {
                conn.execute(
                    "UPDATE repos SET status = 'missing', missing_since = COALESCE(missing_since, ?1), \
                     lifecycle_epoch = lifecycle_epoch + 1 \
                     WHERE canonical_path = ?2 AND user_state != 'removed'",
                    params![now, &canonical_path],
                )?;
                return Err(error);
            }
        };
        let history_changed =
            history_store::mark_stale(&conn, &canonical_path, observed_history_roots.as_ref())
                .unwrap_or(false);
        (canonical_path, refill, history_changed)
    };

    watcher::reconcile_repo_registration(app, &canonical_path);
    emit_repo_status(app, &canonical_path, "active");
    emit_catalog_changed(app, &canonical_path, "restored");
    emit_refill_invalidated(app, &canonical_path, refill);
    if history_changed {
        if let Some(indexer) = app.try_state::<Arc<history_indexer::HistoryIndexer>>() {
            let _ = indexer.request(canonical_path.clone());
        }
    }
    let _ = discovery_orchestrator::append_scan_log(
        app,
        &format!("{now} repo_recovered: {canonical_path}"),
    );
    Ok(true)
}

fn list_excluded_repos_in(conn: &Connection) -> Result<Vec<ExcludedRepo>> {
    let mut stmt = conn.prepare(
        r#"
        SELECT r.canonical_path, r.name,
               COALESCE(r.removed_at, t.removed_at, 0),
               COALESCE(t.reason, '')
        FROM repos r
        LEFT JOIN discovery_tombstones t
          ON t.canonical_path = r.canonical_path
        WHERE r.user_state = 'removed'
        ORDER BY CASE WHEN COALESCE(t.reason, '') LIKE 'user_exclude_alias:%'
                      THEN 1 ELSE 0 END,
                 COALESCE(r.removed_at, t.removed_at, 0) DESC,
                 r.name COLLATE NOCASE
        "#,
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);
    let mut seen = HashSet::new();
    Ok(rows
        .into_iter()
        .filter_map(|(path, name, excluded_at, _reason)| {
            let key = discovery_orchestrator::repo_identity_match_key(&path);
            seen.insert(key).then_some(ExcludedRepo {
                folder_available: None,
                path,
                name,
                excluded_at,
            })
        })
        .collect())
}

#[derive(Debug, Clone)]
struct EquivalentRepoRow {
    path: String,
    name: String,
    source: String,
    user_state: String,
    repo_storage_identity: Option<String>,
    tombstone_reason: Option<String>,
}

fn equivalent_repo_rows_in(
    conn: &Connection,
    canonical_path: &str,
) -> Result<Vec<EquivalentRepoRow>> {
    let target_key = discovery_orchestrator::repo_identity_match_key(canonical_path);
    let mut stmt = conn.prepare(
        "SELECT r.canonical_path, r.name, r.primary_source, r.user_state, \
                r.repo_storage_identity, t.reason \
         FROM repos r LEFT JOIN discovery_tombstones t \
           ON t.canonical_path = r.canonical_path",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(EquivalentRepoRow {
                path: row.get(0)?,
                name: row.get(1)?,
                source: row.get(2)?,
                user_state: row.get(3)?,
                repo_storage_identity: row.get(4)?,
                tombstone_reason: row.get(5)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter(|row| discovery_orchestrator::repo_identity_match_key(&row.path) == target_key)
        .collect())
}

fn equivalent_tombstone_paths_in(conn: &Connection, canonical_path: &str) -> Result<Vec<String>> {
    if !sqlite_table_exists(conn, "discovery_tombstones")? {
        return Ok(Vec::new());
    }
    let target_key = discovery_orchestrator::repo_identity_match_key(canonical_path);
    let mut statement = conn.prepare("SELECT canonical_path FROM discovery_tombstones")?;
    let paths = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(paths
        .into_iter()
        .filter(|path| discovery_orchestrator::repo_identity_match_key(path) == target_key)
        .collect())
}

fn unresolved_commit_aliases_in(conn: &Connection, canonical_path: &str) -> Result<Vec<String>> {
    if !sqlite_table_exists(conn, "commits")? {
        return Ok(Vec::new());
    }
    let target_key = discovery_orchestrator::repo_identity_match_key(canonical_path);
    let mut statement = conn.prepare("SELECT DISTINCT repo_path FROM commits WHERE repo_id = 0")?;
    let paths = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(paths
        .into_iter()
        .filter(|path| discovery_orchestrator::repo_identity_match_key(path) == target_key)
        .collect())
}

fn upsert_exclusion_tombstone(
    conn: &Connection,
    path: &str,
    now: i64,
    reason: &str,
    name: &str,
    source: &str,
) -> Result<()> {
    conn.execute(
        r#"
        INSERT INTO discovery_tombstones (
            canonical_path, removed_at, reason,
            last_known_name, last_known_source
        )
        VALUES (?1, ?2, ?3, ?4, ?5)
        ON CONFLICT(canonical_path) DO UPDATE SET
            removed_at = excluded.removed_at,
            reason = excluded.reason,
            last_known_name = excluded.last_known_name,
            last_known_source = excluded.last_known_source
        "#,
        params![path, now, reason, name, source],
    )?;
    Ok(())
}

fn equivalent_storage_identity(rows: &[EquivalentRepoRow]) -> Result<Option<String>> {
    let identities: HashSet<&str> = rows
        .iter()
        .filter_map(|row| row.repo_storage_identity.as_deref())
        .collect();
    if identities.len() > 1 {
        anyhow::bail!(
            "Excluded repository aliases have conflicting storage identities; add the folder as a new repository"
        );
    }
    Ok(identities.into_iter().next().map(str::to_string))
}

fn row_restores_pinned(row: &EquivalentRepoRow) -> bool {
    if row.user_state == "pinned" {
        return true;
    }
    if row.user_state != "removed" {
        return false;
    }
    // Tombstones from the original implementation used the plain reason and
    // historically restored pinned. New normal-state markers are explicit.
    !matches!(
        row.tombstone_reason.as_deref(),
        Some("user_exclude:normal" | "user_exclude_alias:normal")
    )
}

fn exclude_repo_in(conn: &mut Connection, canonical_path: &str, now: i64) -> Result<Vec<String>> {
    let cache_aliases = unresolved_commit_aliases_in(conn, canonical_path)?;
    exclude_repo_in_with_cache_aliases(conn, canonical_path, now, &cache_aliases)
}

fn exclude_repo_in_with_cache_aliases(
    conn: &mut Connection,
    canonical_path: &str,
    now: i64,
    cache_aliases: &[String],
) -> Result<Vec<String>> {
    let tx = conn.transaction()?;
    let rows = equivalent_repo_rows_in(&tx, canonical_path)?;
    let Some(selected) = rows
        .iter()
        .find(|row| row.path == canonical_path && row.user_state != "removed")
    else {
        anyhow::bail!("repository is not active: {canonical_path}");
    };
    let mut affected_paths = rows.iter().map(|row| row.path.clone()).collect::<Vec<_>>();

    // A pre-normalization database can contain ordinary-UNC and extended-UNC
    // spellings for the same worktree. They are one lifecycle unit: excluding
    // only the selected spelling leaves an active twin able to repopulate the
    // timeline. Retain every row until restore (so metadata is recoverable),
    // but expose only the selected leader in Settings.
    for row in &rows {
        let restore_state = if row_restores_pinned(row) {
            "pinned"
        } else {
            "normal"
        };
        let reason = if row.path == canonical_path {
            format!("user_exclude:{restore_state}")
        } else {
            format!("user_exclude_alias:{restore_state}")
        };
        tx.execute(
            "UPDATE repos SET user_state = 'removed', removed_at = ?1, \
             lifecycle_epoch = lifecycle_epoch + 1 \
             WHERE canonical_path = ?2",
            params![now, &row.path],
        )?;
        upsert_exclusion_tombstone(&tx, &row.path, now, &reason, &row.name, &row.source)?;
        if sqlite_table_exists(&tx, "history_cleanup_jobs")? {
            history_store::queue_repo_purge(&tx, &row.path)?;
        }
    }
    // Very old rows can predate catalog attachment and carry repo_id=0 under
    // an equivalent Windows spelling. Give those physical cache aliases their
    // own recoverable tombstone + bounded purge job instead of leaking disk
    // forever. They remain hidden immediately and are retired on restore.
    for alias in cache_aliases {
        if affected_paths.contains(alias) {
            continue;
        }
        upsert_exclusion_tombstone(
            &tx,
            alias,
            now,
            "user_exclude_cache_alias:normal",
            &selected.name,
            &selected.source,
        )?;
        if sqlite_table_exists(&tx, "history_cleanup_jobs")? {
            history_store::queue_repo_purge(&tx, alias)?;
        }
        affected_paths.push(alias.clone());
    }
    tx.commit()?;
    Ok(affected_paths)
}

fn restore_repo_in(
    conn: &mut Connection,
    canonical_path: &str,
    folder_available: bool,
    now: i64,
    observed_storage_identity: Option<&str>,
    replacement_name: Option<&str>,
) -> Result<String> {
    let tx = conn.transaction()?;
    let rows = equivalent_repo_rows_in(&tx, canonical_path)?;
    let tombstone_paths = equivalent_tombstone_paths_in(&tx, canonical_path)?;
    let Some(selected) = rows
        .iter()
        .find(|row| row.path == canonical_path && row.user_state == "removed")
    else {
        anyhow::bail!("repository is not excluded: {canonical_path}");
    };
    if rows.iter().any(|row| row.user_state != "removed") {
        anyhow::bail!("an equivalent repository identity is already active");
    }
    let name = selected.name.clone();
    let restored_user_state = if rows.iter().any(row_restores_pinned) {
        "pinned"
    } else {
        "normal"
    };

    let status = if folder_available {
        "active"
    } else {
        "missing"
    };
    let missing_since = if folder_available { None } else { Some(now) };
    tx.execute(
        "UPDATE repos SET user_state = ?1, removed_at = NULL, \
         status = ?2, missing_since = ?3, last_verified_at = ?4, \
         lifecycle_epoch = lifecycle_epoch + 1 \
         WHERE canonical_path = ?5",
        params![
            restored_user_state,
            status,
            missing_since,
            now,
            canonical_path
        ],
    )?;
    for tombstone_path in tombstone_paths {
        tx.execute(
            "DELETE FROM discovery_tombstones WHERE canonical_path = ?1",
            params![tombstone_path],
        )?;
    }
    // Collapse invalid legacy path twins only after the user restores the
    // group. Exclusion itself retains every row and preference; restore keeps
    // the selected leader, merges non-conflicting provenance/aliases, and
    // retires duplicate catalog owners so discovery has exactly one active
    // identity and no sibling tombstone can keep it excluded.
    for row in rows.iter().filter(|row| row.path != canonical_path) {
        merge_duplicate_repo_metadata(&tx, &row.path, canonical_path)?;
        tx.execute(
            "DELETE FROM repos WHERE canonical_path = ?1",
            params![&row.path],
        )?;
    }
    if let Some(replacement_name) = replacement_name {
        let storage_identity = observed_storage_identity
            .context("replacement activation requires a storage identity")?;
        tx.execute(
            "UPDATE repos SET repo_storage_identity = ?1, name = ?2, \
               user_state = 'normal', primary_source = 'manual', confidence = 100, \
               first_seen_at = ?3, last_seen_at = ?3, last_verified_at = ?3 \
             WHERE canonical_path = ?4",
            params![storage_identity, replacement_name, now, canonical_path],
        )?;
        tx.execute(
            "DELETE FROM repo_sources WHERE repo_canonical_path = ?1",
            params![canonical_path],
        )?;
        tx.execute(
            "DELETE FROM path_aliases WHERE canonical_path = ?1",
            params![canonical_path],
        )?;
        tx.execute(
            "DELETE FROM discovery_roots WHERE created_from_repo = ?1",
            params![canonical_path],
        )?;
    } else if let Some(storage_identity) = observed_storage_identity {
        tx.execute(
            "UPDATE repos SET repo_storage_identity = COALESCE(repo_storage_identity, ?1) \
             WHERE canonical_path = ?2",
            params![storage_identity, canonical_path],
        )?;
    }
    tx.commit()?;
    Ok(name)
}

fn sqlite_table_exists(conn: &Connection, table: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
        params![table],
        |row| row.get::<_, i64>(0),
    )? != 0)
}

fn merge_duplicate_repo_metadata(
    conn: &Connection,
    duplicate_path: &str,
    keeper_path: &str,
) -> Result<()> {
    if sqlite_table_exists(conn, "repo_sources")? {
        // Preserve sources the keeper did not already know. On a conflict the
        // keeper's row wins; it is the durable spelling the user restored.
        conn.execute(
            "UPDATE OR IGNORE repo_sources SET repo_canonical_path = ?1 \
             WHERE repo_canonical_path = ?2",
            params![keeper_path, duplicate_path],
        )?;
        conn.execute(
            "DELETE FROM repo_sources WHERE repo_canonical_path = ?1",
            params![duplicate_path],
        )?;
    }
    if sqlite_table_exists(conn, "path_aliases")? {
        conn.execute(
            "UPDATE path_aliases SET canonical_path = ?1 WHERE canonical_path = ?2",
            params![keeper_path, duplicate_path],
        )?;
    }
    if sqlite_table_exists(conn, "discovery_roots")? {
        conn.execute(
            "UPDATE discovery_roots SET created_from_repo = ?1 WHERE created_from_repo = ?2",
            params![keeper_path, duplicate_path],
        )?;
    }
    Ok(())
}

fn resolve_active_repo_identity(conn: &Connection, raw_path: &str) -> Result<Option<String>> {
    // Lifecycle commands receive the durable catalog path, not an arbitrary
    // user-entered alias. Avoid a second filesystem probe here — especially
    // important after an SMB `is_dir` failure. Keep the caller's exact
    // spelling so a legacy UNC twin can be selected deterministically.
    discovery_orchestrator::registered_active_repo_identity(conn, raw_path)
}

fn resolve_non_removed_repo_identity(conn: &Connection, raw_path: &str) -> Result<Option<String>> {
    discovery_orchestrator::registered_non_removed_repo_identity(conn, raw_path)
}

fn resolve_removed_repo_identity(conn: &Connection, raw_path: &str) -> Result<Option<String>> {
    discovery_orchestrator::registered_removed_repo_identity(conn, raw_path)
}

fn resolve_missing_repo_identity(conn: &Connection, raw_path: &str) -> Result<Option<String>> {
    discovery_orchestrator::registered_missing_repo_identity(conn, raw_path)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MissingObservation {
    Pending,
    Confirmed,
    Ignored,
}

fn observe_repo_unavailable_in(
    conn: &Connection,
    canonical_path: &str,
    now: i64,
) -> Result<MissingObservation> {
    let row = conn
        .query_row(
            "SELECT status, user_state, missing_since, last_verified_at \
             FROM repos WHERE canonical_path = ?1",
            params![canonical_path],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((status, user_state, first_failure, last_verified)) = row else {
        return Ok(MissingObservation::Ignored);
    };
    if status != "active" || user_state == "removed" {
        return Ok(MissingObservation::Ignored);
    }

    let Some(first) = first_failure else {
        conn.execute(
            "UPDATE repos SET missing_since = ?1 \
             WHERE canonical_path = ?2 AND status = 'active' AND user_state != 'removed'",
            params![now, canonical_path],
        )?;
        return Ok(MissingObservation::Pending);
    };
    let age = now.saturating_sub(first);
    if last_verified.is_some_and(|verified| verified >= first)
        || age > MISSING_CONFIRMATION_WINDOW_SECONDS
    {
        conn.execute(
            "UPDATE repos SET missing_since = ?1 \
             WHERE canonical_path = ?2 AND status = 'active' AND user_state != 'removed'",
            params![now, canonical_path],
        )?;
        return Ok(MissingObservation::Pending);
    }
    if age < MISSING_CONFIRMATION_GRACE_SECONDS {
        return Ok(MissingObservation::Pending);
    }

    let updated = conn.execute(
        "UPDATE repos SET status = 'missing', lifecycle_epoch = lifecycle_epoch + 1 \
         WHERE canonical_path = ?1 AND status = 'active' AND user_state != 'removed'",
        params![canonical_path],
    )?;
    Ok(if updated > 0 {
        MissingObservation::Confirmed
    } else {
        MissingObservation::Ignored
    })
}

pub(crate) fn record_repo_available(app: &AppHandle, canonical_path: &str) -> Result<bool> {
    let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
    let conn = cache::open(app)?;
    let Some(canonical_path) = resolve_active_repo_identity(&conn, canonical_path)? else {
        return Ok(false);
    };
    clear_repo_unavailable_observation_in(&conn, &canonical_path, unix_now())
}

pub(crate) fn clear_repo_unavailable_observation_in(
    conn: &Connection,
    canonical_path: &str,
    now: i64,
) -> Result<bool> {
    let updated = conn.execute(
        "UPDATE repos SET missing_since = NULL, last_verified_at = ?1 \
         WHERE canonical_path = ?2 AND status = 'active' \
           AND user_state != 'removed' AND missing_since IS NOT NULL",
        params![now, canonical_path],
    )?;
    Ok(updated > 0)
}

fn repo_name_in(conn: &Connection, canonical_path: &str) -> Result<String> {
    conn.query_row(
        "SELECT name FROM repos WHERE canonical_path = ?1",
        params![canonical_path],
        |row| row.get(0),
    )
    .with_context(|| format!("repository is not registered: {canonical_path}"))
}

fn repo_storage_identity_in(conn: &Connection, canonical_path: &str) -> Result<Option<String>> {
    conn.query_row(
        "SELECT repo_storage_identity FROM repos WHERE canonical_path = ?1",
        params![canonical_path],
        |row| row.get(0),
    )
    .with_context(|| format!("repository is not registered: {canonical_path}"))
}

fn activate_missing_repo_in(conn: &Connection, canonical_path: &str, now: i64) -> Result<bool> {
    let updated = conn.execute(
        "UPDATE repos SET status = 'active', missing_since = NULL, last_verified_at = ?1, \
         lifecycle_epoch = lifecycle_epoch + 1 \
         WHERE canonical_path = ?2 AND status = 'missing' AND user_state != 'removed'",
        params![now, canonical_path],
    )?;
    Ok(updated > 0)
}

fn validate_restorable_worktree(path: &Path, expected_identity: &str) -> Result<()> {
    let requested = discovery_orchestrator::canonical_repo_identity(path);
    let actual = discovery_orchestrator::exact_worktree_identity(path)
        .with_context(|| format!("Not a Git working tree: {}", path.display()))?;
    let expected_key = discovery_orchestrator::repo_identity_match_key(expected_identity);
    if discovery_orchestrator::repo_identity_match_key(&requested) != expected_key
        || discovery_orchestrator::repo_identity_match_key(&actual) != expected_key
    {
        anyhow::bail!(
            "Folder is no longer the excluded Git working tree: {}",
            path.display()
        );
    }
    Ok(())
}

fn read_recent_repo_commits_stable(
    path: &Path,
    name: String,
) -> Result<(Vec<git::CommitSummary>, String)> {
    for _ in 0..2 {
        let refs_before = git::repo_refs_fingerprint(path, None)?;
        let commits = read_recent_repo_commits(path, name.clone())?;
        let refs_after = git::repo_refs_fingerprint(path, None)?;
        if refs_before == refs_after {
            return Ok((commits, refs_after));
        }
    }
    anyhow::bail!("Repository changed repeatedly while loading; retry")
}

fn capture_stable_replacement_identity(path: &Path) -> Result<(String, String)> {
    for _ in 0..3 {
        let storage_before = git::repo_storage_identity(path)?;
        let refs_before = git::repo_refs_fingerprint(path, None)?;
        let storage_after = git::repo_storage_identity(path)?;
        let refs_after = git::repo_refs_fingerprint(path, None)?;
        if storage_before == storage_after && refs_before == refs_after {
            return Ok((storage_after, refs_after));
        }
    }
    anyhow::bail!("Repository changed repeatedly while preparing confirmation; retry")
}

fn read_recent_repo_commits(path: &Path, name: String) -> Result<Vec<git::CommitSummary>> {
    let cutoff = unix_now() - RESTORE_WINDOW_SECONDS;
    git::recent_commits(path, RESTORE_COMMIT_CAP, cutoff)
        .with_context(|| format!("Couldn't read repository history: {}", path.display()))
        .map(|commits| {
            commits
                .into_iter()
                .map(|mut commit| {
                    commit.repo_name = name.clone();
                    commit
                })
                .collect()
        })
}

fn persist_recent_repo_commits(
    conn: &mut Connection,
    commits: Vec<git::CommitSummary>,
) -> Result<Option<cache::UpsertOutcome>> {
    if commits.is_empty() {
        return Ok(None);
    }
    Ok(Some(cache::upsert_commits(conn, &commits)?))
}

fn emit_refill_invalidated(
    app: &AppHandle,
    canonical_path: &str,
    outcome: Option<cache::UpsertOutcome>,
) {
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

    fn equivalent_row(identity: Option<&str>) -> EquivalentRepoRow {
        EquivalentRepoRow {
            path: "/repo".into(),
            name: "repo".into(),
            source: "test".into(),
            user_state: "removed".into(),
            repo_storage_identity: identity.map(str::to_string),
            tombstone_reason: Some("user_exclude:normal".into()),
        }
    }

    #[test]
    fn restore_identity_uses_every_equivalent_alias_and_rejects_conflicts() {
        assert_eq!(
            equivalent_storage_identity(&[equivalent_row(None), equivalent_row(Some("disk:a"))])
                .unwrap(),
            Some("disk:a".into())
        );
        assert!(equivalent_storage_identity(&[
            equivalent_row(Some("disk:a")),
            equivalent_row(Some("disk:b")),
        ])
        .is_err());
    }

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE repos (
                canonical_path TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'active',
                user_state TEXT NOT NULL DEFAULT 'normal',
                lifecycle_epoch INTEGER NOT NULL DEFAULT 0,
                repo_storage_identity TEXT,
                primary_source TEXT NOT NULL DEFAULT 'fs_walk',
                confidence INTEGER NOT NULL DEFAULT 0,
                first_seen_at INTEGER,
                last_seen_at INTEGER,
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
            CREATE TABLE repo_sources (
                repo_canonical_path TEXT NOT NULL,
                source TEXT NOT NULL,
                PRIMARY KEY(repo_canonical_path, source)
            );
            CREATE TABLE path_aliases (
                observed_path TEXT NOT NULL,
                canonical_path TEXT NOT NULL,
                source TEXT NOT NULL,
                PRIMARY KEY(observed_path, source)
            );
            CREATE TABLE discovery_roots (
                root_path TEXT PRIMARY KEY,
                created_from_repo TEXT
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
        assert_eq!(
            tombstone,
            ("user_exclude:normal".to_string(), "alpha".to_string())
        );

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

        let name = restore_repo_in(&mut conn, "/repos/alpha", true, 456, None, None).unwrap();

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
            ("normal".to_string(), "active".to_string(), None, None)
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

        restore_repo_in(&mut conn, "/repos/offline", false, 456, None, None).unwrap();

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
    fn exclusion_round_trip_preserves_pinned_state() {
        let mut conn = test_db();
        insert_repo(&conn, "/repos/pinned", "pinned");
        conn.execute(
            "UPDATE repos SET user_state = 'pinned' WHERE canonical_path = '/repos/pinned'",
            [],
        )
        .unwrap();

        exclude_repo_in(&mut conn, "/repos/pinned", 123).unwrap();
        restore_repo_in(&mut conn, "/repos/pinned", true, 456, None, None).unwrap();

        let state: String = conn
            .query_row(
                "SELECT user_state FROM repos WHERE canonical_path = '/repos/pinned'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(state, "pinned");
    }

    #[test]
    fn replacement_activation_resets_identity_and_provenance_in_one_transaction() {
        let mut conn = test_db();
        insert_repo(&conn, "/repos/replaced", "old");
        conn.execute(
            "UPDATE repos SET user_state = 'pinned', repo_storage_identity = 'disk:old' \
             WHERE canonical_path = '/repos/replaced'",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO repo_sources(repo_canonical_path, source) VALUES('/repos/replaced', 'vscode')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO path_aliases(observed_path, canonical_path, source) \
             VALUES('/alias', '/repos/replaced', 'vscode')",
            [],
        )
        .unwrap();
        exclude_repo_in(&mut conn, "/repos/replaced", 123).unwrap();

        restore_repo_in(
            &mut conn,
            "/repos/replaced",
            true,
            456,
            Some("disk:new"),
            Some("new"),
        )
        .unwrap();

        let row: (String, String, String, String, i64) = conn
            .query_row(
                "SELECT name, user_state, repo_storage_identity, primary_source, confidence \
                 FROM repos WHERE canonical_path = '/repos/replaced'",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(
            row,
            (
                "new".into(),
                "normal".into(),
                "disk:new".into(),
                "manual".into(),
                100,
            )
        );
        let provenance: i64 = conn
            .query_row(
                "SELECT (SELECT COUNT(*) FROM repo_sources) + \
                        (SELECT COUNT(*) FROM path_aliases)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(provenance, 0);
    }

    #[test]
    fn restore_validation_requires_the_exact_worktree_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("repo");
        git2::Repository::init(&root).unwrap();
        let child = root.join("nested");
        std::fs::create_dir(&child).unwrap();
        let identity = discovery_orchestrator::canonical_repo_identity(&root);

        validate_restorable_worktree(&root, &identity).unwrap();
        assert!(validate_restorable_worktree(&child, &identity).is_err());
    }

    #[test]
    fn empty_directory_cannot_recover_a_missing_repo() {
        let temp = tempfile::tempdir().unwrap();
        let expected = discovery_orchestrator::canonical_repo_identity(temp.path());

        assert!(validate_restorable_worktree(temp.path(), &expected).is_err());
    }

    #[test]
    fn missing_activation_is_conditional_and_preserves_removed_rows() {
        let conn = test_db();
        insert_repo(&conn, "/repos/missing", "missing");
        conn.execute(
            "UPDATE repos SET status = 'missing', missing_since = 100 \
             WHERE canonical_path = '/repos/missing'",
            [],
        )
        .unwrap();

        assert!(activate_missing_repo_in(&conn, "/repos/missing", 200).unwrap());
        assert!(!activate_missing_repo_in(&conn, "/repos/missing", 201).unwrap());

        insert_repo(&conn, "/repos/removed", "removed");
        conn.execute(
            "UPDATE repos SET status = 'missing', user_state = 'removed' \
             WHERE canonical_path = '/repos/removed'",
            [],
        )
        .unwrap();
        assert!(!activate_missing_repo_in(&conn, "/repos/removed", 200).unwrap());
    }

    #[test]
    fn missing_requires_two_separated_failures_and_positive_probe_clears_evidence() {
        let conn = test_db();
        insert_repo(&conn, "/repos/network", "network");

        assert_eq!(
            observe_repo_unavailable_in(&conn, "/repos/network", 100).unwrap(),
            MissingObservation::Pending
        );
        let first: (String, Option<i64>, i64) = conn
            .query_row(
                "SELECT status, missing_since, lifecycle_epoch FROM repos \
                 WHERE canonical_path = '/repos/network'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(first, ("active".to_string(), Some(100), 0));

        // A duplicate observation inside the grace period is not independent
        // evidence and does not move the persisted first-failure timestamp.
        assert_eq!(
            observe_repo_unavailable_in(&conn, "/repos/network", 101).unwrap(),
            MissingObservation::Pending
        );
        let first_failure: Option<i64> = conn
            .query_row(
                "SELECT missing_since FROM repos WHERE canonical_path = '/repos/network'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(first_failure, Some(100));

        assert!(clear_repo_unavailable_observation_in(&conn, "/repos/network", 102).unwrap());
        let cleared: (String, Option<i64>, Option<i64>) = conn
            .query_row(
                "SELECT status, missing_since, last_verified_at FROM repos \
                 WHERE canonical_path = '/repos/network'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(cleared, ("active".to_string(), None, Some(102)));

        assert_eq!(
            observe_repo_unavailable_in(&conn, "/repos/network", 200).unwrap(),
            MissingObservation::Pending
        );
        assert_eq!(
            observe_repo_unavailable_in(&conn, "/repos/network", 202).unwrap(),
            MissingObservation::Confirmed
        );
        let confirmed: (String, Option<i64>, i64) = conn
            .query_row(
                "SELECT status, missing_since, lifecycle_epoch FROM repos \
                 WHERE canonical_path = '/repos/network'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(confirmed, ("missing".to_string(), Some(200), 1));
    }

    #[test]
    fn stale_failure_is_not_treated_as_consecutive_evidence() {
        let conn = test_db();
        insert_repo(&conn, "/repos/network", "network");
        assert_eq!(
            observe_repo_unavailable_in(&conn, "/repos/network", 100).unwrap(),
            MissingObservation::Pending
        );

        let outside_window = 100 + MISSING_CONFIRMATION_WINDOW_SECONDS + 1;
        assert_eq!(
            observe_repo_unavailable_in(&conn, "/repos/network", outside_window).unwrap(),
            MissingObservation::Pending
        );
        let row: (String, Option<i64>, i64) = conn
            .query_row(
                "SELECT status, missing_since, lifecycle_epoch FROM repos \
                 WHERE canonical_path = '/repos/network'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, ("active".to_string(), Some(outside_window), 0));
    }

    #[test]
    fn lifecycle_resolution_does_not_cross_active_and_removed_unc_twins() {
        let conn = test_db();
        insert_repo(&conn, r"\\server\share\repo", "active");
        insert_repo(&conn, r"\\?\UNC\server\share\repo", "removed");
        conn.execute(
            "UPDATE repos SET user_state = 'removed' WHERE canonical_path = ?1",
            [r"\\?\UNC\server\share\repo"],
        )
        .unwrap();

        assert_eq!(
            resolve_active_repo_identity(&conn, r"\\?\UNC\server\share\repo").unwrap(),
            Some(r"\\server\share\repo".to_string())
        );
        assert_eq!(
            resolve_removed_repo_identity(&conn, r"\\server\share\repo").unwrap(),
            Some(r"\\?\UNC\server\share\repo".to_string())
        );
    }

    #[test]
    fn exclusion_and_restore_treat_extended_unc_twins_as_one_identity() {
        let mut conn = test_db();
        let ordinary = r"\\server\share\repo";
        let extended = r"\\?\UNC\server\share\repo";
        insert_repo(&conn, ordinary, "ordinary");
        insert_repo(&conn, extended, "extended");
        conn.execute(
            "UPDATE repos SET user_state = 'pinned' WHERE canonical_path = ?1",
            [extended],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO repo_sources (repo_canonical_path, source) VALUES (?1, 'manual')",
            [extended],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO path_aliases (observed_path, canonical_path, source) \
             VALUES ('legacy-alias', ?1, 'vscode')",
            [extended],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO discovery_roots (root_path, created_from_repo) \
             VALUES ('legacy-root', ?1)",
            [extended],
        )
        .unwrap();

        let mut affected = exclude_repo_in(&mut conn, ordinary, 123).unwrap();
        affected.sort();
        let mut expected = vec![ordinary.to_string(), extended.to_string()];
        expected.sort();
        assert_eq!(affected, expected);

        let removed: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM repos WHERE user_state = 'removed'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(removed, 2, "an active spelling must not survive exclusion");
        let alias_reason: String = conn
            .query_row(
                "SELECT reason FROM discovery_tombstones WHERE canonical_path = ?1",
                [extended],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(alias_reason, "user_exclude_alias:pinned");

        restore_repo_in(&mut conn, ordinary, false, 456, None, None).unwrap();
        let rows: i64 = conn
            .query_row("SELECT COUNT(*) FROM repos", [], |row| row.get(0))
            .unwrap();
        assert_eq!(rows, 1, "restore collapses duplicate catalog ownership");
        let restored: (String, String) = conn
            .query_row(
                "SELECT user_state, status FROM repos WHERE canonical_path = ?1",
                [ordinary],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(restored, ("pinned".to_string(), "missing".to_string()));
        let moved_source: String = conn
            .query_row(
                "SELECT repo_canonical_path FROM repo_sources WHERE source = 'manual'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let moved_alias: String = conn
            .query_row(
                "SELECT canonical_path FROM path_aliases WHERE observed_path = 'legacy-alias'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let moved_root: String = conn
            .query_row(
                "SELECT created_from_repo FROM discovery_roots WHERE root_path = 'legacy-root'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(moved_source, ordinary);
        assert_eq!(moved_alias, ordinary);
        assert_eq!(moved_root, ordinary);
        let tombstones: i64 = conn
            .query_row("SELECT COUNT(*) FROM discovery_tombstones", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(tombstones, 0);
    }

    #[test]
    fn exclusion_purges_unattached_unc_cache_alias_and_restore_clears_its_tombstone() {
        use crate::history_schema::test_support;

        let mut conn = test_support::v5_connection();
        conn.execute_batch(
            "ALTER TABLE repos ADD COLUMN primary_source TEXT NOT NULL DEFAULT 'unknown'; \
             ALTER TABLE repos ADD COLUMN removed_at INTEGER; \
             ALTER TABLE repos ADD COLUMN missing_since INTEGER; \
             ALTER TABLE repos ADD COLUMN last_verified_at INTEGER;",
        )
        .unwrap();
        let ordinary = r"\\server\share\repo";
        let extended = r"\\?\UNC\server\share\repo";
        conn.execute(
            "INSERT INTO repos (path, name, discovered_at, last_seen_at, canonical_path, \
                 status, user_state, primary_source) \
             VALUES (?1, 'repo', 1, 1, ?1, 'active', 'normal', 'manual')",
            [ordinary],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO commits (repo_path, hash, repo_id, repo_name, short_hash, summary, \
                 author, email, timestamp, sort_ts, parents, message) \
             VALUES (?1, 'legacy1', 0, 'repo', 'legacy1', 'legacy alias', \
                 'Ada', 'ada@example.com', 1, -1, '[]', 'legacy alias')",
            [extended],
        )
        .unwrap();

        let mut affected = exclude_repo_in(&mut conn, ordinary, 123).unwrap();
        affected.sort();
        let mut expected = vec![ordinary.to_string(), extended.to_string()];
        expected.sort();
        assert_eq!(affected, expected);
        assert_eq!(
            conn.query_row(
                "SELECT reason FROM discovery_tombstones WHERE canonical_path = ?1",
                [extended],
                |row| row.get::<_, String>(0),
            )
            .unwrap(),
            "user_exclude_cache_alias:normal"
        );

        history_store::drain_pending_cleanup(&conn, extended).unwrap();
        history_store::drain_pending_cleanup(&conn, ordinary).unwrap();
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM commits WHERE repo_path = ?1",
                [extended],
                |row| row.get::<_, i64>(0),
            )
            .unwrap(),
            0
        );

        restore_repo_in(&mut conn, ordinary, false, 456, None, None).unwrap();
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM discovery_tombstones", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0,
            "restoring the identity must remove cache-only alias tombstones"
        );
    }

    #[test]
    fn operation_guard_serializes_equivalent_unc_spellings_only() {
        use std::sync::mpsc;
        use std::time::Duration;

        let held = lock_repo_operation(r"\\server\share\repo");
        let (started_tx, started_rx) = mpsc::channel();
        let (acquired_tx, acquired_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            let _guard = lock_repo_operation(r"\\?\UNC\SERVER\SHARE\repo");
            acquired_tx.send(()).unwrap();
        });

        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(acquired_rx.recv_timeout(Duration::from_millis(30)).is_err());
        drop(held);
        acquired_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        worker.join().unwrap();
    }
}
