// Multi-tier discovery orchestrator.
//
// Runs as a background task requested by discovery_scheduler. Pulls
// candidates from every available source (cache, VS Code family, git config,
// filesystem walk, learned roots), deduplicates, validates with
// git2::Repository::discover, and upserts to cache. Learns parent roots from
// validated repos so the next run finds siblings.
//
// Commit 3 scope: fill the cache silently. UI still sees repos through
// the existing `list_repos` / `discover_repos` paths. Commit 4 wires
// event emission and a richer event model.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

use crate::cache;
use crate::discovery;
use crate::discovery_sources::{self, Candidate, DiscoverySource, GitConfigHint};
use crate::git;
use crate::history_indexer;
use crate::repo_exclusions;
use crate::watcher;

/// Payload mirroring `cache::Repo` plus the source the orchestrator
/// learned the repo from. Frontend uses `name` + `path` directly and
/// `source` only for debug tooltips. camelCase serialization matches
/// existing event payloads.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredRepoPayload {
    pub path: String,
    pub name: String,
    pub source: &'static str,
    pub confidence: i32,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepoStatusPayload {
    canonical_path: String,
    status: &'static str,
}

fn reconcile_identity_retirement(app: &AppHandle, canonical_path: &str) {
    if let Some(indexer) = app.try_state::<Arc<history_indexer::HistoryIndexer>>() {
        indexer.cancel(canonical_path);
    }
    watcher::reconcile_repo_registration(app, canonical_path);
    let _ = app.emit(
        "timeline://repo-status",
        RepoStatusPayload {
            canonical_path: canonical_path.to_string(),
            status: "missing",
        },
    );
}

/// Streamed every time the validation loop hits a milestone or finishes.
/// `state` is "scanning" while the pipeline is alive and "complete" once
/// the final scan.log line is written.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgressPayload {
    pub repos_found: usize,
    pub state: &'static str,
    pub errors: usize,
}

/// Meta-table key that tracks whether the first-launch full scan has
/// already completed. Lives in cache.db (not settings.json) so wiping
/// the cache correctly re-triggers the full scan.
pub const META_INITIAL_SCAN_COMPLETED: &str = "initial_scan_completed";

/// Hard cap on how long the synchronous portion of VS Code recents
/// reading is allowed to take. The orchestrator runs this in a
/// spawn_blocking, but we still don't want to drag a giant Cursor DB
/// past this — the deadline propagates into each read.
const VSCODE_RECENTS_DEADLINE_MS: u64 = 2_000;

/// Max candidates we'll validate per run. Validation is git2::open per
/// candidate; an unbounded list (e.g. JetBrains user with 1000 recents)
/// could otherwise pin a CPU core.
const VALIDATION_CAP: usize = 400;

/// scan.log size cap before rotation. Append-only file in app data dir.
const SCAN_LOG_MAX_BYTES: u64 = 1 * 1024 * 1024;

/// Top N candidates whose parent dir gets learned as a high-confidence
/// scan root. Without this cap a user with 200 IDE recents in
/// different parent dirs would seed too many learned roots.
const PARENT_LEARNING_CAP: usize = 64;

/// Serializes the small state-changing edge of repository lifecycle work.
///
/// Discovery validation and filesystem walking deliberately happen outside
/// this lock. Only the final DB/cache/watch/event transition is serialized so
/// an exclusion cannot race a queued watcher refresh or a just-validated
/// discovery and leave an excluded repository watched or cached again.
static REPO_LIFECYCLE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) fn lock_repo_lifecycle() -> MutexGuard<'static, ()> {
    REPO_LIFECYCLE_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Snapshot used to fence any slow Git read before it mutates cache state.
/// State alone is insufficient: active -> excluded -> restored is an ABA
/// transition whose final booleans look unchanged. `lifecycle_epoch` makes
/// every such round trip observable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepoLifecycleFence {
    canonical_path: String,
    epoch: i64,
    user_state: String,
    status: String,
}

impl RepoLifecycleFence {
    pub(crate) fn canonical_path(&self) -> &str {
        &self.canonical_path
    }
}

pub(crate) fn capture_repo_lifecycle_fence(
    conn: &Connection,
    canonical_path: &str,
) -> Result<Option<RepoLifecycleFence>> {
    let Some(stored_path) = registered_repo_identity(conn, canonical_path)? else {
        return Ok(None);
    };
    match conn.query_row(
        "SELECT lifecycle_epoch, user_state, status FROM repos WHERE canonical_path = ?1",
        params![&stored_path],
        |row| {
            Ok(RepoLifecycleFence {
                canonical_path: stored_path.clone(),
                epoch: row.get(0)?,
                user_state: row.get(1)?,
                status: row.get(2)?,
            })
        },
    ) {
        Ok(fence) => Ok(Some(fence)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(error) => Err(error.into()),
    }
}

pub(crate) fn capture_active_repo_lifecycle_fence(
    conn: &Connection,
    canonical_path: &str,
) -> Result<Option<RepoLifecycleFence>> {
    let Some(stored_path) = registered_active_repo_identity(conn, canonical_path)? else {
        return Ok(None);
    };
    Ok(capture_repo_lifecycle_fence(conn, &stored_path)?
        .filter(|fence| fence.user_state != "removed" && fence.status == "active"))
}

pub(crate) fn capture_non_removed_repo_lifecycle_fence(
    conn: &Connection,
    canonical_path: &str,
) -> Result<Option<RepoLifecycleFence>> {
    let Some(stored_path) = registered_non_removed_repo_identity(conn, canonical_path)? else {
        return Ok(None);
    };
    Ok(capture_repo_lifecycle_fence(conn, &stored_path)?
        .filter(|fence| fence.user_state != "removed"))
}

pub(crate) fn repo_lifecycle_fence_matches(
    conn: &Connection,
    fence: &RepoLifecycleFence,
) -> Result<bool> {
    let current = capture_repo_lifecycle_fence(conn, &fence.canonical_path)?;
    Ok(current.as_ref() == Some(fence))
}

/// Validated repo as the orchestrator sees it after `git2::Repository::discover`.
#[derive(Debug, Clone)]
struct ValidatedRepo {
    /// Path as the candidate source observed it (used for path_aliases).
    observed_path: String,
    /// Canonicalized workdir (primary key in the repos table).
    canonical_path: String,
    /// `.git` directory path.
    gitdir_path: String,
    /// Stable common-gitdir identity captured while the path is known online.
    repo_storage_identity: Option<String>,
    /// Display name = leaf folder name of canonical_path.
    name: String,
}

/// Manually add a repo by path (drag-drop / paste / future file picker).
/// Validates with git2 (will walk up if the user dropped a subdirectory
/// of a repo), upserts with primary_source='manual' + user_state='pinned'
/// + confidence 100 so it stays at the top of the paint order, learns
/// parent roots, and emits `timeline://repo-discovered` so the frontend
/// can append a row immediately.
///
/// Returns the discovered repo payload on success, or an error string
/// the frontend can surface inline (e.g. "Not a Git working tree").
pub fn add_repo_explicit(app: &AppHandle, raw_path: &str) -> Result<DiscoveredRepoPayload> {
    let path = PathBuf::from(raw_path.trim());
    if path.as_os_str().is_empty() {
        return Err(anyhow::anyhow!("empty path"));
    }
    let mut validated =
        validate_repo_candidate(&path).ok_or_else(|| anyhow::anyhow!("Not a Git working tree"))?;

    let now = unix_now();
    let accepted = {
        // Keep the durable catalog mutation indivisible with exclusion.
        // Validation above and initial history loading below are read-only and
        // intentionally outside the lock so a slow network path cannot block
        // an unrelated repository's lifecycle transition.
        let _lifecycle = lock_repo_lifecycle();
        let mut conn = cache::open(app)?;
        if repo_is_excluded(&conn, &validated.canonical_path)? {
            anyhow::bail!("Repository is excluded; restore it in Settings");
        }
        if let Some(stored) = registered_repo_identity(&conn, &validated.canonical_path)? {
            validated.canonical_path = stored;
        }
        let candidate = Candidate {
            path: PathBuf::from(&validated.canonical_path),
            source: DiscoverySource::Manual,
            confidence: 100,
            raw_hint: Some(format!("manual:{raw_path}")),
        };
        let tx = conn.transaction()?;
        let accepted = upsert_discovered_repo(&tx, &validated, &candidate, now)?;
        if accepted {
            // Manual add overrides any pre-existing 'normal' user_state and
            // promotes the repo to 'pinned' so it survives a tombstone-style
            // wipe and stays at the top of paint order.
            tx.execute(
                "UPDATE repos SET user_state = 'pinned' WHERE canonical_path = ?1",
                params![&validated.canonical_path],
            )?;
            record_repo_source(&tx, &validated, &candidate, now)?;
            record_path_alias(&tx, &validated, &candidate, now)?;
            for learned in learn_roots_from_repo(&validated, DiscoverySource::Manual) {
                upsert_discovery_root(&tx, &learned, &validated.canonical_path, now)?;
            }
        }
        tx.commit()?;
        accepted
    };
    if !accepted {
        reconcile_identity_retirement(app, &validated.canonical_path);
        anyhow::bail!(
            "Folder now contains a different Git repository. Move the replacement to a new folder before adding it so old preferences are never inherited"
        );
    }
    // The history walk is intentionally outside the lifecycle lock. The
    // announcement captures an epoch fence and serializes only its final
    // cache/watch/event edge.
    let payload = announce_registered_repo(app, &validated, DiscoverySource::Manual, 100)?;

    // A "super-repo" the user picked may contain independent nested
    // repos — standalone clones the parent typically .gitignore's.
    // Discover and register those too: the user added the folder to
    // watch its git activity, and that includes what's nested inside.
    discover_nested_repos(
        app,
        &PathBuf::from(&validated.canonical_path),
        &validated.canonical_path,
    );

    let _ = append_scan_log(
        app,
        &format!(
            "{} manual_add: path={} name={}",
            now, validated.canonical_path, validated.name
        ),
    );

    Ok(payload)
}

/// Stream a freshly-registered repo to the frontend: emit
/// `timeline://repo-discovered`, read + cache + emit its recent commits
/// via `timeline://repo-fill`, and attach the file watcher. Returns the
/// payload. Shared by explicit add and nested-repo discovery.
fn announce_registered_repo(
    app: &AppHandle,
    repo: &ValidatedRepo,
    source: DiscoverySource,
    confidence: i32,
) -> Result<DiscoveredRepoPayload> {
    let initial_conn = cache::open(app)?;
    let lifecycle_fence = capture_active_repo_lifecycle_fence(&initial_conn, &repo.canonical_path)?
        .ok_or_else(|| anyhow::anyhow!("Repository became excluded during discovery"))?;
    drop(initial_conn);

    let payload = DiscoveredRepoPayload {
        path: repo.canonical_path.clone(),
        name: repo.name.clone(),
        source: source.as_str(),
        confidence,
    };
    let repo_path = PathBuf::from(&repo.canonical_path);
    let cutoff = unix_now() - 7 * 86_400;
    // Cap matches the watcher's REFRESH_MAX_PER_REPO rationale: the initial
    // fill must cover a whole active week, not just the newest ten commits.
    let commits = git::recent_commits(&repo_path, 100, cutoff)
        .unwrap_or_default()
        .into_iter()
        .map(|mut c| {
            c.repo_name = repo.name.clone();
            c
        })
        .collect::<Vec<_>>();

    let outcome = {
        let _lifecycle = lock_repo_lifecycle();
        let mut conn = cache::open(app)?;
        if !repo_lifecycle_fence_matches(&conn, &lifecycle_fence)? {
            anyhow::bail!("Repository lifecycle changed during discovery");
        }
        if commits.is_empty() {
            None
        } else {
            Some(cache::upsert_commits(&mut conn, &commits)?)
        }
    };

    // Gitdir resolution, canonicalization and notify registration can block
    // on a disconnected network path. Reconcile outside the lifecycle mutex;
    // its before/after epoch snapshots repair any concurrent state change.
    watcher::reconcile_repo_registration(app, &repo.canonical_path);
    let _ = app.emit("timeline://repo-discovered", &payload);

    // Lightweight windowed-pull signal — the frontend re-pulls the affected
    // windows from the cache. It follows registration and watcher attachment,
    // so every visible row already has a durable, live catalog owner.
    if let Some(o) = outcome {
        let _ = app.emit(
            "timeline://invalidated",
            cache::TimelineInvalidated {
                generation: o.generation,
                inserted: o.inserted,
                deleted: 0,
                repo_path: repo.canonical_path.clone(),
            },
        );
    }

    Ok(payload)
}

/// Discover *independent* nested repos inside an explicitly-added folder.
/// A user who picks a "super-repo" containing standalone clones (each
/// with its own `.git` directory) expects those to show up too.
/// Submodules and linked worktrees — whose `.git` is a *file* pointing
/// into the parent — belong to the parent and are skipped. Nested repos
/// register with normal user_state; only the explicitly-picked folder is
/// pinned. Best-effort: any failure is swallowed.
fn discover_nested_repos(app: &AppHandle, super_path: &Path, super_canonical: &str) {
    // `scan_path` stops descending at the first `.git`, so walking
    // `super_path` itself would stop at the super-repo's own `.git` and
    // never see what's nested. Start one level down — walk each child.
    let mut found: Vec<PathBuf> = Vec::new();
    let Ok(entries) = std::fs::read_dir(super_path) else {
        return;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if !child.is_dir() {
            continue;
        }
        let name = child
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        if discovery::is_hard_excluded(&name) {
            continue;
        }
        discovery::scan_path(&child, |p| {
            // Independent repo only: `.git` is a directory. A `.git`
            // *file* means submodule / linked worktree — skip.
            if p.join(".git").is_dir() {
                found.push(p);
            }
        });
    }
    if found.is_empty() {
        return;
    }

    let now = unix_now();
    // Git validation can touch an unavailable/network worktree and therefore
    // never belongs under the global lifecycle lock.
    let prepared: Vec<ValidatedRepo> = found
        .iter()
        .filter_map(|path| validate_repo_candidate(path))
        .filter(|validated| {
            repo_identity_match_key(&validated.canonical_path)
                != repo_identity_match_key(super_canonical)
        })
        .collect();

    let mut registered: Vec<ValidatedRepo> = Vec::new();
    let mut identity_retired = Vec::new();
    {
        let _lifecycle = lock_repo_lifecycle();
        let Ok(mut conn) = cache::open(app) else {
            return;
        };
        let Ok(tx) = conn.transaction() else {
            return;
        };
        for mut validated in prepared {
            if repo_is_excluded(&tx, &validated.canonical_path).unwrap_or(true) {
                continue;
            }
            if let Ok(Some(stored)) = registered_repo_identity(&tx, &validated.canonical_path) {
                validated.canonical_path = stored;
            }
            let candidate = Candidate {
                path: PathBuf::from(&validated.canonical_path),
                source: DiscoverySource::Manual,
                confidence: 90,
                raw_hint: Some(format!("nested-of:{super_canonical}")),
            };
            match upsert_discovered_repo(&tx, &validated, &candidate, now) {
                Ok(true) => {}
                Ok(false) => {
                    identity_retired.push(validated.canonical_path.clone());
                    continue;
                }
                Err(_) => continue,
            }
            let _ = record_repo_source(&tx, &validated, &candidate, now);
            let _ = record_path_alias(&tx, &validated, &candidate, now);
            registered.push(validated);
        }
        if tx.commit().is_err() {
            return;
        }
    }

    for path in identity_retired {
        reconcile_identity_retirement(app, &path);
    }

    for validated in registered {
        if announce_registered_repo(app, &validated, DiscoverySource::Manual, 90).is_err() {
            continue;
        }
        let _ = append_scan_log(
            app,
            &format!(
                "{} nested_add: path={} name={} super={}",
                now, validated.canonical_path, validated.name, super_canonical
            ),
        );
    }
}

pub(crate) async fn run(
    app: &AppHandle,
    cancel: CancellationToken,
    trigger: &'static str,
) -> Result<()> {
    let app_for_blocking = app.clone();
    let cancel_for_blocking = cancel.clone();

    let join = tauri::async_runtime::spawn_blocking(move || -> Result<(usize, usize, bool)> {
        let mut conn = cache::open(&app_for_blocking)?;
        let is_first_run =
            cache::meta_get(&conn, META_INITIAL_SCAN_COMPLETED)?.as_deref() != Some("true");
        let summary = run_pipeline_sync(
            &app_for_blocking,
            &mut conn,
            cancel_for_blocking.clone(),
            is_first_run,
        )?;

        // Only mark the first-run flag if the scan actually completed
        // (didn't get cancelled mid-flight) — otherwise the next launch
        // would skip the full sweep with an incomplete cache.
        if is_first_run && !cancel_for_blocking.is_cancelled() {
            cache::meta_set(&conn, META_INITIAL_SCAN_COMPLETED, "true")?;
        }
        Ok((summary.0, summary.1, is_first_run))
    })
    .await;

    match join {
        Ok(Ok((seen, valid, first))) => {
            let line = format!(
                "{} discovery: trigger={} seen={} valid={} first_run={} cancelled={}",
                unix_now(),
                trigger,
                seen,
                valid,
                first,
                cancel.is_cancelled()
            );
            let _ = append_scan_log(app, &line);
            Ok(())
        }
        Ok(Err(e)) => {
            emit_scan_progress(app, 0, "complete", 1);
            Err(e)
        }
        Err(e) => {
            emit_scan_progress(app, 0, "complete", 1);
            Err(anyhow::anyhow!("orchestrator join error: {e}"))
        }
    }
}

/// Walk active rows, check path existence, flip status accordingly.
/// Emits one `timeline://repo-status` event per row that changed state
/// so the frontend can grey out / restore individual rows without a
/// full reload. Read-only: never deletes a row even if it's been gone
/// forever — that's the user's call via hide/relink.
fn verify_existing_repos(app: &AppHandle, conn: &mut Connection) -> Result<()> {
    let mut stmt = conn.prepare(
        r#"
        SELECT canonical_path, status, missing_since
        FROM repos
        WHERE user_state NOT IN ('removed')
          AND status IN ('active', 'missing')
        "#,
    )?;
    let rows: Vec<(String, String, Option<i64>)> = stmt
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    let mut pending_confirmations = Vec::new();
    for (path, status, pending_missing_probe) in rows {
        let exists = Path::new(&path).is_dir();
        match (status.as_str(), exists) {
            ("active", false) => match repo_exclusions::mark_repo_missing(app, &path) {
                Ok(false) => pending_confirmations.push(path.clone()),
                Ok(true) => {}
                Err(error) => {
                    eprintln!("marking repository missing failed for {path}: {error:#}");
                }
            },
            ("active", true) if pending_missing_probe.is_some() => {
                // Only rows with persisted failure evidence incur a write.
                // Healthy rows remain a read-only O(N) sweep as before.
                if let Err(error) = repo_exclusions::record_repo_available(app, &path) {
                    eprintln!("clearing repository missing probe failed for {path}: {error:#}");
                }
            }
            ("missing", true) => {
                // is_dir is merely the cheap trigger. Recovery itself opens
                // Git and requires the resolved worktree to equal this exact
                // stored identity before changing any state.
                if let Err(error) = repo_exclusions::recover_missing_repo(app, &path) {
                    eprintln!("repository recovery deferred for {path}: {error:#}");
                }
            }
            _ => {}
        }
    }
    schedule_missing_confirmations(app, pending_confirmations);
    Ok(())
}

/// Re-probe only the rows that actually failed the startup existence sweep.
/// This is one bounded background batch, not a second O(N) startup pass. The
/// first failure remains persisted in `missing_since`, so a restart or another
/// status reconciliation can supply the second observation as well.
fn schedule_missing_confirmations(app: &AppHandle, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(
            repo_exclusions::MISSING_CONFIRMATION_GRACE_SECONDS as u64 * 1_000 + 100,
        ));
        for path in paths {
            if Path::new(&path).is_dir() {
                let _ = repo_exclusions::record_repo_available(&app, &path);
            } else if let Err(error) = repo_exclusions::mark_repo_missing(&app, &path) {
                eprintln!("confirming repository missing failed for {path}: {error:#}");
            }
        }
    });
}

/// Update the tray tooltip to reflect the current scan state. Best-
/// effort: tray might not exist yet or the tooltip API might fail on
/// some Linux distros — log to stderr if so but don't propagate.
fn set_tray_tooltip(app: &AppHandle, text: &str) {
    if let Some(tray) = app.tray_by_id("main") {
        if let Err(e) = tray.set_tooltip(Some(text)) {
            eprintln!("tray tooltip update failed: {e}");
        }
    }
}

/// Format the tooltip exactly as the spec requires.
fn tooltip_text(repos_found: usize, state: &str, errors: usize) -> String {
    match (state, errors) {
        ("scanning", _) => format!("gitwink — scanning... {repos_found} found"),
        ("complete", 0) => format!("gitwink — {repos_found} repositories"),
        ("complete", n) => format!("gitwink — {repos_found} repositories, {n} issues"),
        _ => "gitwink".to_string(),
    }
}

/// Pull candidates from every tier, dedup, validate, upsert. Returns
/// (candidates_seen, candidates_valid).
fn run_pipeline_sync(
    app: &AppHandle,
    conn: &mut Connection,
    cancel: CancellationToken,
    is_first_run: bool,
) -> Result<(usize, usize)> {
    // Verification sweep BEFORE discovery. Active rows use only a cheap
    // existence probe to become missing. A missing row that appears again is
    // promoted only through the exact-worktree recovery pipeline (Git
    // validation → DB → cache → watcher → events), never by existence alone.
    if let Err(e) = verify_existing_repos(app, conn) {
        eprintln!("orchestrator verification sweep failed: {e:#}");
    }

    let mut candidates: Vec<Candidate> = Vec::new();

    // Tier 1: VS Code-family recents (VS Code / Insiders / Cursor / Windsurf).
    let recents_deadline = Instant::now() + Duration::from_millis(VSCODE_RECENTS_DEADLINE_MS);
    for (source, primary, _fallback) in discovery_sources::vscode_family_db_paths() {
        if cancel.is_cancelled() {
            return Ok((candidates.len(), 0));
        }
        candidates.extend(discovery_sources::read_vscode_recents(
            &primary,
            source,
            recents_deadline,
        ));
    }

    // Tier 1b: expand any .code-workspace candidates into folder entries.
    // Iterate over a snapshot — we mutate `candidates` while reading it.
    let workspaces: Vec<PathBuf> = candidates
        .iter()
        .filter(|c| c.source == DiscoverySource::CodeWorkspace)
        .map(|c| c.path.clone())
        .collect();
    for ws_path in workspaces {
        if cancel.is_cancelled() {
            return Ok((candidates.len(), 0));
        }
        candidates.extend(discovery_sources::expand_code_workspace(&ws_path));
    }

    // Tier 2: git config hints (safe.directory; includeIf root patterns
    // are noted but Tier 4 expansion of them is Phase 2).
    for hint in discovery_sources::read_git_config_hints() {
        if let GitConfigHint::RepoPath(path) = hint {
            candidates.push(Candidate {
                path,
                source: DiscoverySource::GitConfigSafe,
                confidence: 85,
                raw_hint: None,
            });
        }
    }

    // Tier 4: learned roots from previous runs. Read-only here; the
    // learning loop happens after we validate.
    let learned_roots = load_learned_roots(conn)?;
    for root in &learned_roots {
        if cancel.is_cancelled() {
            return Ok((candidates.len(), 0));
        }
        if !root.root_path.is_dir() {
            continue;
        }
        let mut acc: Vec<PathBuf> = Vec::new();
        discovery::scan_path(&root.root_path, |p| acc.push(p));
        for p in acc {
            candidates.push(Candidate {
                path: p,
                source: DiscoverySource::FsWalk,
                confidence: 55,
                raw_hint: Some(format!("learned:{}", root.root_path.display())),
            });
        }
    }

    // Tier 5: default filesystem fallback. Heavy — only on first run.
    // Subsequent runs rely on cache + IDE recents + learned roots +
    // watcher events to surface new repos.
    if is_first_run {
        for root in discovery::default_roots() {
            if cancel.is_cancelled() {
                return Ok((candidates.len(), 0));
            }
            let mut acc: Vec<PathBuf> = Vec::new();
            discovery::scan_path(&root, |p| acc.push(p));
            for p in acc {
                candidates.push(Candidate {
                    path: p,
                    source: DiscoverySource::FsWalk,
                    confidence: 46,
                    raw_hint: Some(format!("default-root:{}", root.display())),
                });
            }
        }
    }

    let seen = candidates.len();
    let dedup = dedup_candidates(candidates);

    // Cheap pre-validation exclusion gate. A candidate may be a subdirectory
    // of a repository, so this is only an optimization; the authoritative
    // gate runs again after git2 resolves the exact worktree root.
    let tombstoned = load_exclusion_identities(conn)?;
    let to_validate: Vec<Candidate> = dedup
        .into_iter()
        .filter(|c| {
            let key = repo_identity_match_key(&canonical_repo_identity(&c.path));
            !tombstoned.contains(&key)
        })
        .take(VALIDATION_CAP)
        .collect();

    let now = unix_now();
    let mut valid = 0usize;
    let mut parent_learn_count = 0usize;
    let mut last_tooltip_at = Instant::now() - Duration::from_secs(1);
    let mut emit_buffer: Vec<DiscoveredRepoPayload> = Vec::new();
    let mut identity_retired = Vec::new();
    let mut registered_identities = load_registered_identity_map(conn)?;

    set_tray_tooltip(app, &tooltip_text(0, "scanning", 0));
    emit_scan_progress(app, 0, "scanning", 0);

    // Git open/canonicalization/storage-identity probes can block on network
    // paths. Finish every one before opening SQLite's write transaction so a
    // dead candidate cannot convoy watcher, lifecycle, or history writers.
    let mut prepared = Vec::new();
    for candidate in to_validate {
        if cancel.is_cancelled() {
            break;
        }
        if let Some(validated) = validate_repo_candidate(&candidate.path) {
            prepared.push((candidate, validated));
        }
    }

    {
        let tx = conn.transaction()?;
        for (candidate, mut validated) in prepared {
            if cancel.is_cancelled() {
                break;
            }
            if repo_is_excluded(&tx, &validated.canonical_path)? {
                continue;
            }
            let identity_key = repo_identity_match_key(&validated.canonical_path);
            if let Some(stored) = registered_identities.get(&identity_key) {
                validated.canonical_path = stored.clone();
            } else {
                registered_identities.insert(identity_key, validated.canonical_path.clone());
            }
            if !upsert_discovered_repo(&tx, &validated, &candidate, now)? {
                // Missing rows have a stored physical identity. A candidate
                // at the same path may not reactivate one when that identity
                // differs; the explicit recovery flow owns that decision.
                identity_retired.push(validated.canonical_path.clone());
                continue;
            }
            record_repo_source(&tx, &validated, &candidate, now)?;
            record_path_alias(&tx, &validated, &candidate, now)?;

            if parent_learn_count < PARENT_LEARNING_CAP {
                for learned in learn_roots_from_repo(&validated, candidate.source) {
                    upsert_discovery_root(&tx, &learned, &validated.canonical_path, now)?;
                    parent_learn_count += 1;
                }
            }
            valid += 1;

            // Stage the payload — we emit it AFTER the transaction
            // commits so listeners never see a row that might roll back.
            emit_buffer.push(DiscoveredRepoPayload {
                path: validated.canonical_path.clone(),
                name: validated.name.clone(),
                source: candidate.source.as_str(),
                confidence: candidate.confidence,
            });

            // Tooltip + progress throttle: spec says max one update per
            // 500ms so a fast scan doesn't thrash the tray. The last
            // tick of the scan is emitted explicitly outside the loop.
            if last_tooltip_at.elapsed() >= Duration::from_millis(500) {
                set_tray_tooltip(app, &tooltip_text(valid, "scanning", 0));
                emit_scan_progress(app, valid, "scanning", 0);
                last_tooltip_at = Instant::now();
            }
        }
        tx.commit()?;
    }

    for path in identity_retired {
        reconcile_identity_retirement(app, &path);
    }

    // Flush per-repo events AFTER commit. Frontend can append these to
    // its row list — the cache row they reference is now durable. Attach
    // the file watcher here too: lib.rs only wires repos already cached at
    // startup, so without this every repo the scan finds stays unwatched
    // for the whole first session — no live updates exactly when the
    // first impression is formed. `add` is idempotent (canonical-path map
    // check), so re-announcing a known repo is harmless.
    let watcher_state = app.try_state::<watcher::RepoWatcher>();
    for payload in emit_buffer {
        let active = cache::open(app)
            .and_then(|conn| repo_is_excluded(&conn, &payload.path).map(|excluded| !excluded))
            .unwrap_or(false);
        if !active {
            continue;
        }
        if watcher_state.is_some() {
            watcher::reconcile_repo_registration(app, &payload.path);
        }
        let _ = app.emit("timeline://repo-discovered", &payload);
    }

    // Final tick.
    set_tray_tooltip(app, &tooltip_text(valid, "complete", 0));
    emit_scan_progress(app, valid, "complete", 0);

    Ok((seen, valid))
}

fn emit_scan_progress(app: &AppHandle, repos_found: usize, state: &'static str, errors: usize) {
    let _ = app.emit(
        "scan-progress",
        &ScanProgressPayload {
            repos_found,
            state,
            errors,
        },
    );
}

// ---------------------------------------------------------------------------
// Candidate dedup
// ---------------------------------------------------------------------------

/// Normalize a path string into the durable identity used by repos,
/// tombstones, discovery candidates and watcher dependents. Windows
/// `canonicalize` returns extended-length forms; both local (`\\?\C:\...`)
/// and UNC (`\\?\UNC\server\share\...`) variants collapse to the ordinary
/// path accepted by SQLite-era rows and frontend IPC.
pub(crate) fn normalize_repo_identity_text(raw: &str) -> String {
    let looks_windows = raw.starts_with("\\\\")
        || raw
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':');
    if !looks_windows {
        return raw.to_string();
    }

    let mut normalized = raw.replace('/', "\\");
    if normalized
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("\\\\?\\UNC\\"))
    {
        normalized = format!("\\\\{}", &normalized[8..]);
    } else if normalized.starts_with("\\\\?\\")
        && normalized
            .as_bytes()
            .get(5)
            .is_some_and(|separator| *separator == b':')
    {
        // Local drive path (`\\?\C:\...`). Preserve other device namespaces
        // such as `\\?\Volume{GUID}\...`; stripping those makes them invalid.
        normalized = normalized[4..].to_string();
    }

    if normalized
        .as_bytes()
        .get(1)
        .is_some_and(|separator| *separator == b':')
    {
        normalized.replace_range(0..1, &normalized[0..1].to_ascii_uppercase());
    }

    while normalized.ends_with('\\')
        && !(normalized.len() == 3 && normalized.as_bytes().get(1) == Some(&b':'))
        && normalized != "\\\\"
    {
        normalized.pop();
    }
    normalized
}

/// Best-effort canonical identity. Filesystem resolution is used when the
/// path is online; the same purely textual normalization is the fallback for
/// missing/offline rows, so their tombstone key remains stable.
pub(crate) fn canonical_repo_identity(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    normalize_repo_identity_text(&resolved.to_string_lossy())
}

pub(crate) fn repo_identity_match_key(raw: &str) -> String {
    let normalized = normalize_repo_identity_text(raw);
    let looks_windows = normalized.starts_with("\\\\")
        || normalized
            .as_bytes()
            .get(1)
            .is_some_and(|separator| *separator == b':');
    if looks_windows {
        normalized.to_ascii_lowercase()
    } else {
        normalized
    }
}

/// Dedupe candidates by best-effort canonical key, keeping the highest-
/// confidence one and merging the source list (the higher one wins).
fn dedup_candidates(mut input: Vec<Candidate>) -> Vec<Candidate> {
    input.sort_by(|a, b| b.confidence.cmp(&a.confidence));
    let mut seen: HashMap<String, Candidate> = HashMap::new();
    for c in input {
        let key = repo_identity_match_key(&canonical_repo_identity(&c.path));
        seen.entry(key).or_insert(c);
    }
    seen.into_values().collect()
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate a candidate path with git2 (read-only). Returns None for
/// non-repos, bare repos (v0.1 viewer scope), and unreadable paths.
fn validate_repo_candidate(candidate_path: &Path) -> Option<ValidatedRepo> {
    let probe = if candidate_path.is_file() {
        candidate_path.parent().unwrap_or(candidate_path)
    } else {
        candidate_path
    };

    let repo = git2::Repository::discover(probe).ok()?;
    if repo.is_bare() {
        return None;
    }
    let workdir = repo.workdir()?;
    let canonical = canonical_repo_identity(workdir);
    let gitdir = canonical_repo_identity(repo.path());
    let repo_storage_identity = git::repo_storage_identity(workdir).ok();

    let name = Path::new(&canonical)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .to_string();

    Some(ValidatedRepo {
        observed_path: candidate_path.to_string_lossy().into_owned(),
        canonical_path: canonical,
        gitdir_path: gitdir,
        repo_storage_identity,
        name,
    })
}

/// Resolve a candidate to its exact working-tree root identity. Used by the
/// legacy scanner and missing-repository recovery so a subdirectory that only
/// happens to sit under another Git repository can never impersonate the
/// original row.
pub(crate) fn exact_worktree_identity(candidate_path: &Path) -> Option<String> {
    validate_repo_candidate(candidate_path).map(|repo| repo.canonical_path)
}

// ---------------------------------------------------------------------------
// Cache upsert
// ---------------------------------------------------------------------------

fn upsert_discovered_repo(
    tx: &rusqlite::Transaction,
    repo: &ValidatedRepo,
    candidate: &Candidate,
    now: i64,
) -> Result<bool> {
    let existing_identity: Option<Option<String>> = tx
        .query_row(
            "SELECT repo_storage_identity FROM repos WHERE canonical_path = ?1",
            params![&repo.canonical_path],
            |row| row.get(0),
        )
        .optional()?;
    if matches!(
        (existing_identity.as_ref().and_then(Option::as_ref), repo.repo_storage_identity.as_ref()),
        (Some(existing), Some(observed)) if existing != observed
    ) {
        // The path now resolves to a different physical Git repository. Keep
        // the old catalog identity/preferences fail-closed as missing; the
        // user can exclude it and add the replacement explicitly, but it may
        // never inherit old coverage, branches, or watcher ownership.
        tx.execute(
            "UPDATE repos SET status = 'missing', missing_since = COALESCE(missing_since, ?1), \
             lifecycle_epoch = lifecycle_epoch + 1 WHERE canonical_path = ?2",
            params![now, &repo.canonical_path],
        )?;
        return Ok(false);
    }
    // First, try inserting fresh. If a row with the same canonical_path
    // already exists, the conflict path updates last_seen / last_verified
    // and bumps confidence ONLY if the new source is more confident.
    let changed = tx.execute(
        r#"
        INSERT INTO repos (
            path, name, discovered_at, last_seen_at,
            canonical_path, gitdir_path,
            repo_storage_identity,
            status, user_state, primary_source, confidence,
            first_seen_at, last_verified_at, repo_kind
        )
        VALUES (?1, ?2, ?3, ?3, ?4, ?5, ?6, 'active', 'normal', ?7, ?8, ?3, ?3, 'workdir')
        ON CONFLICT(canonical_path) DO UPDATE SET
            path = excluded.path,
            name = excluded.name,
            gitdir_path = excluded.gitdir_path,
            repo_storage_identity = COALESCE(repos.repo_storage_identity,
                                             excluded.repo_storage_identity),
            status = 'active',
            missing_since = NULL,
            last_seen_at = excluded.last_seen_at,
            last_verified_at = excluded.last_verified_at,
            primary_source = CASE
                WHEN excluded.confidence > repos.confidence THEN excluded.primary_source
                ELSE repos.primary_source
            END,
            confidence = MAX(repos.confidence, excluded.confidence)
        WHERE repos.status != 'missing'
           OR repos.repo_storage_identity IS NULL
           OR (excluded.repo_storage_identity IS NOT NULL
               AND repos.repo_storage_identity = excluded.repo_storage_identity)
        "#,
        params![
            &repo.canonical_path,
            &repo.name,
            now,
            &repo.canonical_path,
            &repo.gitdir_path,
            &repo.repo_storage_identity,
            candidate.source.as_str(),
            candidate.confidence as i64,
        ],
    )?;
    Ok(changed > 0)
}

fn record_repo_source(
    tx: &rusqlite::Transaction,
    repo: &ValidatedRepo,
    candidate: &Candidate,
    now: i64,
) -> Result<()> {
    tx.execute(
        r#"
        INSERT INTO repo_sources (
            repo_canonical_path, source, source_path, raw_hint,
            confidence, first_seen_at, last_seen_at, last_success_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?6)
        ON CONFLICT(repo_canonical_path, source) DO UPDATE SET
            source_path = excluded.source_path,
            raw_hint = excluded.raw_hint,
            confidence = MAX(repo_sources.confidence, excluded.confidence),
            last_seen_at = excluded.last_seen_at,
            last_success_at = excluded.last_success_at
        "#,
        params![
            &repo.canonical_path,
            candidate.source.as_str(),
            &repo.observed_path,
            &candidate.raw_hint,
            candidate.confidence as i64,
            now,
        ],
    )?;
    Ok(())
}

fn record_path_alias(
    tx: &rusqlite::Transaction,
    repo: &ValidatedRepo,
    candidate: &Candidate,
    now: i64,
) -> Result<()> {
    if repo.observed_path == repo.canonical_path {
        return Ok(());
    }
    tx.execute(
        r#"
        INSERT INTO path_aliases (
            observed_path, canonical_path, source, first_seen_at, last_seen_at
        )
        VALUES (?1, ?2, ?3, ?4, ?4)
        ON CONFLICT(observed_path, source) DO UPDATE SET
            canonical_path = excluded.canonical_path,
            last_seen_at = excluded.last_seen_at
        "#,
        params![
            &repo.observed_path,
            &repo.canonical_path,
            candidate.source.as_str(),
            now,
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Learned roots
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct LearnedRoot {
    root_path: PathBuf,
    #[allow(dead_code)]
    score: f64,
    max_depth: usize,
    entry_budget: usize,
}

#[derive(Debug, Clone, Copy)]
enum LearnedKind {
    ManualParent,
    IdeParent,
    LearnedSibling,
}

impl LearnedKind {
    fn as_str(self) -> &'static str {
        match self {
            LearnedKind::ManualParent => "manual_parent",
            LearnedKind::IdeParent => "ide_parent",
            LearnedKind::LearnedSibling => "learned_sibling",
        }
    }
}

#[derive(Debug, Clone)]
struct DiscoveryRootRow {
    kind: LearnedKind,
    root_path: PathBuf,
    score: f64,
    max_depth: usize,
    entry_budget: usize,
}

/// Walk up from a validated repo and propose parent / grandparent dirs
/// as learned scan roots. Refuses to learn drive roots or home dirs.
fn learn_roots_from_repo(repo: &ValidatedRepo, source: DiscoverySource) -> Vec<DiscoveryRootRow> {
    let mut out = Vec::new();
    let repo_path = Path::new(&repo.canonical_path);

    let Some(parent1) = repo_path.parent() else {
        return out;
    };
    let manual = matches!(source, DiscoverySource::Manual);

    if is_safe_learning_root(parent1) {
        out.push(DiscoveryRootRow {
            kind: if manual {
                LearnedKind::ManualParent
            } else {
                LearnedKind::IdeParent
            },
            root_path: parent1.to_path_buf(),
            score: if manual { 0.98 } else { 0.90 },
            max_depth: 3,
            entry_budget: 2_500,
        });
    }

    if let Some(parent2) = parent1.parent() {
        if is_safe_learning_root(parent2) && !is_drive_or_home_root(parent2) {
            out.push(DiscoveryRootRow {
                kind: LearnedKind::LearnedSibling,
                root_path: parent2.to_path_buf(),
                score: if manual { 0.72 } else { 0.60 },
                max_depth: 4,
                entry_budget: 5_000,
            });
        }
    }

    out
}

fn is_safe_learning_root(path: &Path) -> bool {
    if is_drive_or_home_root(path) {
        return false;
    }
    let s = path.to_string_lossy().to_ascii_lowercase();
    let banned = [
        "\\windows",
        "\\program files",
        "\\program files (x86)",
        "\\appdata",
        "/library",
        "/system",
        "/applications",
        "node_modules",
        "/target",
        "\\target",
    ];
    !banned.iter().any(|b| s.contains(b))
}

fn is_drive_or_home_root(path: &Path) -> bool {
    if path.parent().is_none() {
        return true;
    }
    if let Some(home) = home_dir() {
        if path == home {
            return true;
        }
    }
    false
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

fn upsert_discovery_root(
    tx: &rusqlite::Transaction,
    row: &DiscoveryRootRow,
    created_from: &str,
    now: i64,
) -> Result<()> {
    tx.execute(
        r#"
        INSERT INTO discovery_roots (
            root_path, root_kind, created_from_repo,
            score, max_depth, entry_budget,
            enabled, created_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?7)
        ON CONFLICT(root_path) DO UPDATE SET
            root_kind = excluded.root_kind,
            score = MAX(discovery_roots.score, excluded.score),
            max_depth = MAX(discovery_roots.max_depth, excluded.max_depth),
            entry_budget = MAX(discovery_roots.entry_budget, excluded.entry_budget),
            updated_at = excluded.updated_at
        "#,
        params![
            row.root_path.to_string_lossy(),
            row.kind.as_str(),
            created_from,
            row.score,
            row.max_depth as i64,
            row.entry_budget as i64,
            now,
        ],
    )?;
    Ok(())
}

fn load_learned_roots(conn: &Connection) -> Result<Vec<LearnedRoot>> {
    let now = unix_now();
    let mut stmt = conn.prepare(
        r#"
        SELECT root_path, score, max_depth, entry_budget
        FROM discovery_roots
        WHERE enabled = 1
          AND (cooldown_until IS NULL OR cooldown_until <= ?1)
        ORDER BY score DESC, last_scan_at ASC NULLS FIRST
        LIMIT 32
        "#,
    )?;
    let rows = stmt
        .query_map(params![now], |row| {
            let path: String = row.get(0)?;
            Ok(LearnedRoot {
                root_path: PathBuf::from(path),
                score: row.get(1)?,
                max_depth: row.get::<_, i64>(2)? as usize,
                entry_budget: row.get::<_, i64>(3)? as usize,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn load_exclusion_identities(conn: &Connection) -> Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare(
        "SELECT canonical_path FROM discovery_tombstones \
         UNION SELECT canonical_path FROM repos WHERE user_state = 'removed'",
    )?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .map(|path| repo_identity_match_key(&path))
        .collect())
}

fn load_registered_identity_map(conn: &Connection) -> Result<HashMap<String, String>> {
    // A malformed legacy database can contain both ordinary-UNC and
    // extended-UNC spellings for the same repository. Make the winner stable
    // across query plans and prefer a live catalog row over a removed twin.
    let mut stmt = conn.prepare(
        "SELECT canonical_path FROM repos \
         ORDER BY CASE WHEN user_state = 'removed' THEN 1 ELSE 0 END, \
                  canonical_path COLLATE BINARY",
    )?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut identities = HashMap::new();
    for path in rows {
        identities
            .entry(repo_identity_match_key(&path))
            .or_insert(path);
    }
    Ok(identities)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RegisteredRepoState {
    Any,
    NonRemoved,
    Active,
    Removed,
    Missing,
}

impl RegisteredRepoState {
    fn matches(self, user_state: &str, status: &str) -> bool {
        match self {
            Self::Any => true,
            Self::NonRemoved => user_state != "removed",
            Self::Active => user_state != "removed" && status == "active",
            Self::Removed => user_state == "removed",
            Self::Missing => user_state != "removed" && status == "missing",
        }
    }
}

fn registered_repo_identity_in_state(
    conn: &Connection,
    requested_path: &str,
    state: RegisteredRepoState,
) -> Result<Option<String>> {
    // Exact durable spelling wins, but only inside the requested lifecycle
    // state. If the exact row is the wrong twin (for example, removed while
    // excluding), fall back to an equivalent spelling in the right state.
    let exact = conn
        .query_row(
            "SELECT canonical_path, user_state, status FROM repos \
             WHERE canonical_path = ?1",
            params![requested_path],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .optional()?;
    if let Some((path, user_state, status)) = exact {
        if state.matches(&user_state, &status) {
            return Ok(Some(path));
        }
    }

    // Only legacy spelling/case aliases take the O(catalog) fallback. Normal
    // watcher and timeline paths stay on the primary-key fast path above.
    let mut stmt = conn.prepare(
        "SELECT canonical_path, user_state, status FROM repos \
         ORDER BY CASE WHEN user_state = 'removed' THEN 1 ELSE 0 END, \
                  canonical_path COLLATE BINARY",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let requested_key = repo_identity_match_key(requested_path);
    Ok(rows
        .into_iter()
        .find(|(path, user_state, status)| {
            state.matches(user_state, status) && repo_identity_match_key(path) == requested_key
        })
        .map(|(path, _, _)| path))
}

/// Return the already-stored spelling for an equivalent canonical identity.
/// This prevents a legacy extended-UNC row from acquiring a second ordinary-
/// UNC twin while allowing all new rows to use the normalized representation.
pub(crate) fn registered_repo_identity(
    conn: &Connection,
    canonical_path: &str,
) -> Result<Option<String>> {
    registered_repo_identity_in_state(conn, canonical_path, RegisteredRepoState::Any)
}

pub(crate) fn registered_active_repo_identity(
    conn: &Connection,
    canonical_path: &str,
) -> Result<Option<String>> {
    registered_repo_identity_in_state(conn, canonical_path, RegisteredRepoState::Active)
}

pub(crate) fn registered_non_removed_repo_identity(
    conn: &Connection,
    canonical_path: &str,
) -> Result<Option<String>> {
    registered_repo_identity_in_state(conn, canonical_path, RegisteredRepoState::NonRemoved)
}

pub(crate) fn registered_removed_repo_identity(
    conn: &Connection,
    canonical_path: &str,
) -> Result<Option<String>> {
    registered_repo_identity_in_state(conn, canonical_path, RegisteredRepoState::Removed)
}

pub(crate) fn registered_missing_repo_identity(
    conn: &Connection,
    canonical_path: &str,
) -> Result<Option<String>> {
    registered_repo_identity_in_state(conn, canonical_path, RegisteredRepoState::Missing)
}

/// Final exclusion gate after Git validation has resolved the exact worktree
/// root. The normalized fallback covers databases that predate UNC-prefix
/// normalization without probing their potentially offline network paths.
pub(crate) fn repo_is_excluded(conn: &Connection, canonical_path: &str) -> Result<bool> {
    let exact: i64 = conn.query_row(
        "SELECT EXISTS( \
             SELECT 1 FROM discovery_tombstones WHERE canonical_path = ?1 \
             UNION ALL \
             SELECT 1 FROM repos WHERE canonical_path = ?1 AND user_state = 'removed' \
         )",
        params![canonical_path],
        |row| row.get(0),
    )?;
    if exact != 0 {
        return Ok(true);
    }
    Ok(load_exclusion_identities(conn)?.contains(&repo_identity_match_key(canonical_path)))
}

// ---------------------------------------------------------------------------
// scan.log
// ---------------------------------------------------------------------------

/// Append a single line to <app_data>/scan.log, rotating to scan.log.old
/// once the file crosses SCAN_LOG_MAX_BYTES. Errors swallowed — logging
/// is observability, not correctness.
pub fn append_scan_log(app: &AppHandle, line: &str) -> Result<()> {
    use std::io::Write;
    let dir = app.path().app_config_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("scan.log");

    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() >= SCAN_LOG_MAX_BYTES {
            let old = dir.join("scan.log.old");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(&path, &old);
        }
    }

    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(f, "{line}")?;
    Ok(())
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

    /// Build an absolute, OS-native path from segments. `learn_roots`
    /// walks `Path::parent()`, which only splits on the host platform's
    /// real separator — a hard-coded Windows `C:\…` literal collapses to
    /// a single component on the Unix CI runners, so the walk finds no
    /// parents. Constructing the path per-OS keeps these tests honest on
    /// both Windows and macOS.
    fn os_path(segments: &[&str]) -> String {
        let mut p = if cfg!(windows) {
            PathBuf::from("C:\\")
        } else {
            PathBuf::from("/")
        };
        for s in segments {
            p.push(s);
        }
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn windows_extended_paths_share_one_durable_identity() {
        assert_eq!(
            normalize_repo_identity_text(r"\\?\C:\Projects\gitwink\"),
            r"C:\Projects\gitwink"
        );
        assert_eq!(
            normalize_repo_identity_text(r"\\?\UNC\server\share\repo\"),
            r"\\server\share\repo"
        );
        assert_eq!(
            normalize_repo_identity_text(r"\\?\Volume{abc}\repo\"),
            r"\\?\Volume{abc}\repo"
        );
        assert_eq!(
            repo_identity_match_key(r"c:/PROJECTS/gitwink"),
            repo_identity_match_key(r"\\?\C:\projects\GITWINK")
        );
    }

    #[test]
    fn exclusion_gate_matches_legacy_unc_prefixes() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE repos (
                canonical_path TEXT PRIMARY KEY,
                user_state TEXT NOT NULL DEFAULT 'normal',
                status TEXT NOT NULL DEFAULT 'active'
            );
            CREATE TABLE discovery_tombstones (
                canonical_path TEXT PRIMARY KEY
            );
            INSERT INTO discovery_tombstones(canonical_path)
            VALUES ('\\?\UNC\server\share\repo');
            INSERT INTO repos(canonical_path, user_state)
            VALUES ('\\?\UNC\server\share\active', 'normal');
            "#,
        )
        .unwrap();

        assert!(repo_is_excluded(&conn, r"\\server\share\repo").unwrap());
        assert_eq!(
            registered_repo_identity(&conn, r"\\server\share\active").unwrap(),
            Some(r"\\?\UNC\server\share\active".to_string())
        );
    }

    #[test]
    fn unc_twins_resolve_by_requested_lifecycle_state_and_exact_spelling() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE repos (
                canonical_path TEXT PRIMARY KEY,
                user_state TEXT NOT NULL,
                status TEXT NOT NULL
            );
            INSERT INTO repos(canonical_path, user_state, status) VALUES
                ('\\server\share\repo', 'normal', 'active'),
                ('\\?\UNC\server\share\repo', 'removed', 'active'),
                ('\\server\share\both', 'normal', 'active'),
                ('\\?\UNC\server\share\both', 'normal', 'active');
            "#,
        )
        .unwrap();

        // The exact extended spelling is removed, so an exclude operation
        // must cross the normalized identity to the ordinary active row.
        assert_eq!(
            registered_active_repo_identity(&conn, r"\\?\UNC\server\share\repo").unwrap(),
            Some(r"\\server\share\repo".to_string())
        );
        // Conversely, restore targets the removed twin even when Settings
        // supplied the ordinary spelling.
        assert_eq!(
            registered_removed_repo_identity(&conn, r"\\server\share\repo").unwrap(),
            Some(r"\\?\UNC\server\share\repo".to_string())
        );
        // When both twins satisfy the requested state, exact durable spelling
        // wins rather than depending on SQLite row order.
        assert_eq!(
            registered_active_repo_identity(&conn, r"\\?\UNC\server\share\both").unwrap(),
            Some(r"\\?\UNC\server\share\both".to_string())
        );
    }

    #[test]
    fn lifecycle_epoch_fence_rejects_active_excluded_restored_aba() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE repos (
                canonical_path TEXT PRIMARY KEY,
                user_state TEXT NOT NULL,
                status TEXT NOT NULL,
                lifecycle_epoch INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO repos(canonical_path, user_state, status)
            VALUES ('\\server\share\repo', 'normal', 'active');
            "#,
        )
        .unwrap();

        let fence = capture_active_repo_lifecycle_fence(&conn, r"\\server\share\repo")
            .unwrap()
            .unwrap();
        assert!(repo_lifecycle_fence_matches(&conn, &fence).unwrap());

        conn.execute(
            "UPDATE repos SET user_state = 'removed', lifecycle_epoch = lifecycle_epoch + 1 \
             WHERE canonical_path = ?1",
            [r"\\server\share\repo"],
        )
        .unwrap();
        conn.execute(
            "UPDATE repos SET user_state = 'normal', lifecycle_epoch = lifecycle_epoch + 1 \
             WHERE canonical_path = ?1",
            [r"\\server\share\repo"],
        )
        .unwrap();

        assert!(!repo_lifecycle_fence_matches(&conn, &fence).unwrap());
    }

    #[test]
    fn catalog_upsert_never_reuses_preferences_for_a_different_physical_repo() {
        let mut conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE repos (
                path TEXT PRIMARY KEY, name TEXT NOT NULL,
                discovered_at INTEGER NOT NULL, last_seen_at INTEGER NOT NULL,
                canonical_path TEXT UNIQUE, gitdir_path TEXT,
                repo_storage_identity TEXT, status TEXT NOT NULL,
                user_state TEXT NOT NULL, primary_source TEXT NOT NULL,
                confidence INTEGER NOT NULL, first_seen_at INTEGER,
                last_verified_at INTEGER, repo_kind TEXT NOT NULL,
                missing_since INTEGER, lifecycle_epoch INTEGER NOT NULL DEFAULT 0
            );
            INSERT INTO repos VALUES (
                '/repo', 'old', 1, 1, '/repo', '/repo/.git', 'disk:a',
                'active', 'pinned', 'manual', 100, 1, 1, 'workdir', NULL, 0
            );
            "#,
        )
        .unwrap();
        let candidate = Candidate {
            path: PathBuf::from("/repo"),
            source: DiscoverySource::FsWalk,
            confidence: 50,
            raw_hint: None,
        };
        let mut observed = ValidatedRepo {
            observed_path: "/repo".into(),
            canonical_path: "/repo".into(),
            gitdir_path: "/repo/.git".into(),
            repo_storage_identity: Some("disk:b".into()),
            name: "replacement".into(),
        };
        {
            let tx = conn.transaction().unwrap();
            assert!(!upsert_discovered_repo(&tx, &observed, &candidate, 10).unwrap());
            tx.commit().unwrap();
        }
        assert_eq!(
            conn.query_row(
                "SELECT status, user_state, repo_storage_identity FROM repos",
                [],
                |row| Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?
                ))
            )
            .unwrap(),
            ("missing".into(), "pinned".into(), "disk:a".into())
        );

        observed.repo_storage_identity = Some("disk:a".into());
        {
            let tx = conn.transaction().unwrap();
            assert!(upsert_discovered_repo(&tx, &observed, &candidate, 11).unwrap());
            tx.commit().unwrap();
        }
        assert_eq!(
            conn.query_row("SELECT status FROM repos", [], |row| row
                .get::<_, String>(0))
                .unwrap(),
            "active"
        );
    }

    #[test]
    fn exact_worktree_identity_resolves_subdir_but_not_nested_repo() {
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("outer");
        git2::Repository::init(&outer).unwrap();
        let child = outer.join("child");
        std::fs::create_dir(&child).unwrap();
        assert_eq!(
            exact_worktree_identity(&child),
            Some(canonical_repo_identity(&outer))
        );

        let nested = outer.join("nested");
        git2::Repository::init(&nested).unwrap();
        assert_eq!(
            exact_worktree_identity(&nested),
            Some(canonical_repo_identity(&nested))
        );
    }

    #[test]
    fn dedup_keeps_highest_confidence_per_path() {
        let p = PathBuf::from("/tmp/test-repo");
        let candidates = vec![
            Candidate {
                path: p.clone(),
                source: DiscoverySource::FsWalk,
                confidence: 46,
                raw_hint: None,
            },
            Candidate {
                path: p.clone(),
                source: DiscoverySource::Vscode,
                confidence: 82,
                raw_hint: None,
            },
            Candidate {
                path: p.clone(),
                source: DiscoverySource::Manual,
                confidence: 100,
                raw_hint: None,
            },
        ];
        let out = dedup_candidates(candidates);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].confidence, 100);
        assert!(matches!(out[0].source, DiscoverySource::Manual));
    }

    #[test]
    fn safe_learning_root_rejects_system_dirs() {
        // These would expand fs walk into noise/garbage
        assert!(!is_safe_learning_root(Path::new("C:\\Windows\\System32")));
        assert!(!is_safe_learning_root(Path::new("C:\\Program Files\\foo")));
        assert!(!is_safe_learning_root(Path::new(
            "/Users/me/Library/Caches"
        )));
        // Legitimate dev paths pass through
        assert!(is_safe_learning_root(Path::new("C:\\k2\\keymall")));
        assert!(is_safe_learning_root(Path::new("/Users/me/code")));
    }

    #[test]
    fn learn_roots_proposes_parent_and_grandparent() {
        let workspace = os_path(&["k2", "keymall", "workspace"]);
        let gitdir = os_path(&["k2", "keymall", "workspace", ".git"]);
        let repo = ValidatedRepo {
            observed_path: workspace.clone(),
            canonical_path: workspace,
            gitdir_path: gitdir,
            repo_storage_identity: None,
            name: "workspace".into(),
        };
        let roots = learn_roots_from_repo(&repo, DiscoverySource::Manual);
        // Expect parent (…/k2/keymall) + grandparent (…/k2)
        let paths: Vec<String> = roots
            .iter()
            .map(|r| r.root_path.to_string_lossy().into_owned())
            .collect();
        assert!(paths.iter().any(|p| p.ends_with("keymall")));
        assert!(paths.iter().any(|p| p.ends_with("k2")));
        // Manual-source should mark parent as ManualParent
        assert!(roots
            .iter()
            .any(|r| matches!(r.kind, LearnedKind::ManualParent)));
    }

    #[test]
    fn learn_roots_does_not_climb_into_drive_root() {
        // A repo sitting directly under the filesystem / drive root:
        // that root itself must NEVER be learned as a scan root.
        let foo = os_path(&["foo"]);
        let gitdir = os_path(&["foo", ".git"]);
        let repo = ValidatedRepo {
            observed_path: foo.clone(),
            canonical_path: foo,
            gitdir_path: gitdir,
            repo_storage_identity: None,
            name: "foo".into(),
        };
        let roots = learn_roots_from_repo(&repo, DiscoverySource::Vscode);
        let fs_root = if cfg!(windows) { "C:\\" } else { "/" };
        for r in &roots {
            assert_ne!(r.root_path.to_string_lossy().as_ref(), fs_root);
        }
    }
}
