use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::{
    cache, discovery, discovery_orchestrator, file_history_jobs, git, history_fts, history_git,
    history_indexer, history_query, history_store, panel_window_state, repo_exclusions, settings,
    update, watcher, window,
};

/// Per-repo walk cap for the windowed refill. Sized to swallow a whole
/// agent burst (30-commit rebase, squash-merge train) in one pass: the walk
/// result doubles as the live set `reconcile_repo_commits` trusts, and the
/// old cap of 10 both truncated bursts (only the newest 10 rewritten
/// commits landed in the cache) and left ghosts below the cap unreconciled.
const MAX_COMMITS_PER_REPO: usize = 100;
const MAX_COMMITS_PER_REPO_NO_WINDOW: usize = 1_000;
const TIMELINE_WINDOW_DAYS: i64 = 7;
/// Merged upsert cap across all repos per refill sweep. Raised from 50
/// alongside the per-repo cap — since the refill stopped returning rows
/// over IPC (`refresh_recent_commits`), the only cost is the SQLite upsert,
/// and 50 was silently dropping most of a busy multi-repo week from the
/// cache.
const TIMELINE_MAX_TOTAL: usize = 500;
const TIMELINE_MAX_TOTAL_NO_WINDOW: usize = 5_000;
/// In single-repo mode the user has explicitly drilled into one repo, so
/// the per-repo cap that protects the all-repos timeline (= 10) is way too
/// low. They expect the window to dominate: a 30d view should look like
/// 30d of work, not 10 rows of last week. These caps act as a safety net
/// against monorepos with thousands of commits per month, not as the
/// primary trimming knob.
const SINGLE_REPO_MAX_COMMITS_WINDOWED: usize = 500;
const SINGLE_REPO_MAX_COMMITS_NO_WINDOW: usize = 2_000;

fn cutoff_for(window_days: Option<i64>) -> i64 {
    match window_days {
        Some(d) if d > 0 => unix_now() - d * 86_400,
        _ => 0,
    }
}

fn per_repo_cap(window_days: Option<i64>) -> usize {
    if window_days.is_none() {
        MAX_COMMITS_PER_REPO_NO_WINDOW
    } else {
        MAX_COMMITS_PER_REPO
    }
}

/// Cap used by `repo_commits` (single-repo drill-in mode). Different from
/// `per_repo_cap` because the user has explicitly focused on one repo, so
/// the "one row per repo so the all-repos timeline stays scannable"
/// rationale doesn't apply. The window should dominate; this is just a
/// safety net against monorepos with thousands of commits in the period.
fn single_repo_cap(window_days: Option<i64>) -> usize {
    if window_days.is_none() {
        SINGLE_REPO_MAX_COMMITS_NO_WINDOW
    } else {
        SINGLE_REPO_MAX_COMMITS_WINDOWED
    }
}

fn total_cap(window_days: Option<i64>) -> usize {
    if window_days.is_none() {
        TIMELINE_MAX_TOTAL_NO_WINDOW
    } else {
        TIMELINE_MAX_TOTAL
    }
}

#[tauri::command]
pub fn ping() -> &'static str {
    "pong"
}

#[tauri::command]
pub fn list_repos(app: AppHandle) -> Result<Vec<cache::Repo>, String> {
    let conn = cache::open(&app).map_err(|e| e.to_string())?;
    cache::list_repos(&conn).map_err(|e| e.to_string())
}

#[derive(Clone, Serialize)]
struct ScanProgress {
    root: String,
    found: usize,
}

#[derive(Clone, Serialize)]
struct ScanComplete {
    count: usize,
}

#[tauri::command]
pub async fn list_recent_commits_cached(
    app: AppHandle,
    window_days: Option<i64>,
) -> Result<Vec<git::CommitSummary>, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<git::CommitSummary>, String> {
        let conn = cache::open(&app).map_err(|e| e.to_string())?;
        cache::list_recent_commits(&conn, cutoff_for(window_days), total_cap(window_days))
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Shared body of `recent_commits` / `refresh_recent_commits`: walk every
/// cached repo, reconcile each repo's cached window against its walk (so
/// amended/rebased-away commits leave the cache), then upsert the merged
/// result. A repo whose walk ERRORS (locked, deleted mid-scan) is skipped
/// entirely — an error must never be mistaken for "no live commits", which
/// would wrongly reconcile-delete its whole cached window.
fn scan_and_cache_recent_commits(
    app: &AppHandle,
    window_days: Option<i64>,
) -> Result<Vec<git::CommitSummary>, String> {
    let mut conn = cache::open(app).map_err(|e| e.to_string())?;
    let repos = cache::list_repos(&conn).map_err(|e| e.to_string())?;
    let cutoff = cutoff_for(window_days);
    let per_repo = per_repo_cap(window_days);
    let total = total_cap(window_days);

    let mut all: Vec<git::CommitSummary> = Vec::new();
    let mut write_fences = HashMap::new();
    let mut failed_active: Vec<String> = Vec::new();
    let mut history_changed_paths: Vec<String> = Vec::new();
    let mut network_paths: Vec<String> = Vec::new();
    for repo in repos {
        if history_indexer::is_network_path(&repo.path) {
            network_paths.push(repo.path.clone());
            continue;
        }
        // A repo already known missing (moved, disconnected network drive)
        // must not stall the sweep: Repository::open on a dead SMB path can
        // block 10-30s, and this loop is serial — one dead repo delayed
        // EVERY repo's fresh commits. Its cached rows stay visible (greyed);
        // the background reconciler below re-probes for recovery.
        if repo.status == "missing" {
            continue;
        }
        let lifecycle_fence = {
            let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
            match discovery_orchestrator::capture_active_repo_lifecycle_fence(&conn, &repo.path) {
                Ok(Some(fence)) => fence,
                _ => continue,
            }
        };
        let watcher_epoch = app
            .try_state::<watcher::RepoWatcher>()
            .map(|watcher| watcher.event_epoch(Path::new(&repo.path)));
        let refs_before = match git::repo_refs_fingerprint(Path::new(&repo.path), None) {
            Ok(fingerprint) => fingerprint,
            Err(_) => {
                failed_active.push(repo.path.clone());
                continue;
            }
        };
        let Ok(scan) = git::recent_commits_scan(Path::new(&repo.path), per_repo, cutoff) else {
            // Couldn't open an 'active' repo — let the detached reconciler
            // decide whether the path is actually gone (vs a transient git
            // error), off this hot path.
            failed_active.push(repo.path.clone());
            continue;
        };
        // A watcher write or force-push can land while the walk is running.
        // Never reconcile against a root set different from the one we read:
        // it could delete the just-inserted commit and then miss the event that
        // already fired. The next watcher/summon pass will retry.
        if git::repo_refs_fingerprint(Path::new(&repo.path), None)
            .ok()
            .as_deref()
            != Some(refs_before.as_str())
        {
            continue;
        }
        let can_reconcile = scan.can_reconcile();
        let commits: Vec<git::CommitSummary> = scan
            .rows
            .into_iter()
            .map(|mut c| {
                c.repo_name = repo.name.clone();
                c
            })
            .collect();
        let history_tracked = history_store::coverage(&conn, &repo.path)
            .ok()
            .flatten()
            .is_some_and(|coverage| coverage.published_epoch > 0);
        let observed_history_roots = history_tracked
            .then(|| history_git::snapshot_roots(Path::new(&repo.path)).ok())
            .flatten();
        if git::repo_refs_fingerprint(Path::new(&repo.path), None)
            .ok()
            .as_deref()
            != Some(refs_before.as_str())
        {
            continue;
        }
        let watcher_state = app.try_state::<watcher::RepoWatcher>();
        let expected_epoch = watcher_epoch.map(|epoch| vec![(repo.path.clone(), epoch)]);
        let event_guard = expected_epoch.as_ref().and_then(|expected| {
            watcher_state
                .as_ref()
                .and_then(|watcher| watcher.guard_event_epochs(expected))
        });
        let can_reconcile = can_reconcile && event_guard.is_some();
        let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
        if !discovery_orchestrator::repo_lifecycle_fence_matches(&conn, &lifecycle_fence)
            .unwrap_or(false)
        {
            continue;
        }
        let _ =
            repo_exclusions::clear_repo_unavailable_observation_in(&conn, &repo.path, unix_now());
        if can_reconcile {
            if let Ok(o) =
                cache::reconcile_repo_commits(&mut conn, &repo.path, &commits, cutoff, false)
            {
                // Deletions must be announced: nothing else tells a pinned
                // reader its count shrank (the generation pin doesn't mask
                // physical deletes), and the frontend's delete-burst path
                // does an in-place re-pull off this event.
                if o.deleted > 0 {
                    let _ = app.emit(
                        "timeline://invalidated",
                        cache::TimelineInvalidated {
                            generation: o.generation,
                            inserted: 0,
                            deleted: o.deleted,
                            repo_path: repo.path.clone(),
                        },
                    );
                }
            }
        }
        if history_tracked
            && history_store::mark_stale(&conn, &repo.path, observed_history_roots.as_ref())
                .unwrap_or(false)
        {
            history_changed_paths.push(repo.path.clone());
        }
        write_fences.insert(
            repo.path.clone(),
            (lifecycle_fence, refs_before, watcher_epoch),
        );
        all.extend(commits);
    }
    if let Some(indexer) = app.try_state::<Arc<history_indexer::HistoryIndexer>>() {
        for path in history_changed_paths {
            let _ = indexer.request(path);
        }
    }
    all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    all.truncate(total);

    // The scan itself is intentionally outside the lifecycle lock. Re-check
    // its small unique repo set only at the final cache edge: if Settings
    // excluded a repo during the walk, none of its collected rows can be
    // reinserted after exclusion purged them.
    let root_stable_paths = write_fences
        .iter()
        .filter_map(|(path, (_, refs, epoch))| {
            let epoch_ok = match (app.try_state::<watcher::RepoWatcher>(), epoch) {
                (Some(watcher), Some(epoch)) => watcher.event_epoch(Path::new(path)) == *epoch,
                (None, _) => true,
                (Some(_), None) => false,
            };
            (epoch_ok
                && git::repo_refs_fingerprint(Path::new(path), None)
                    .map(|current| current == *refs)
                    .unwrap_or(false))
            .then_some(path.clone())
        })
        .collect::<HashSet<_>>();
    let watcher_state = app.try_state::<watcher::RepoWatcher>();
    let expected_epochs = write_fences
        .iter()
        .filter(|(path, _)| root_stable_paths.contains(*path))
        .filter_map(|(path, (_, _, epoch))| epoch.map(|epoch| (path.clone(), epoch)))
        .collect::<Vec<_>>();
    let guarded_epochs = watcher_state
        .as_ref()
        .and_then(|watcher| watcher.guard_matching_event_epochs(&expected_epochs));
    let event_stable_paths = guarded_epochs
        .as_ref()
        .map(|(_, paths)| paths.clone())
        .unwrap_or_else(|| root_stable_paths.clone());
    let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
    let valid_paths = write_fences
        .iter()
        .filter_map(|(path, (fence, _, _))| {
            let lifecycle_ok =
                discovery_orchestrator::repo_lifecycle_fence_matches(&conn, fence).unwrap_or(false);
            let event_ok = watcher_state.is_none() || event_stable_paths.contains(path);
            (lifecycle_ok && event_ok && root_stable_paths.contains(path)).then_some(path.clone())
        })
        .collect::<HashSet<_>>();
    all.retain(|commit| valid_paths.contains(&commit.repo_path));
    let _ = cache::upsert_commits(&mut conn, &all).map_err(|e| e.to_string())?;
    drop(_lifecycle);
    drop(guarded_epochs);

    schedule_network_repo_refills(app.clone(), network_paths);

    // Detached, single-flight status reconciliation: flip walk-failed repos
    // whose directory is actually gone to 'missing' (so the NEXT sweep skips
    // them), and probe currently-missing repos for recovery. Runs on its own
    // thread because these probes are exactly the calls that can hang on a
    // dead share — they must never block a sweep or the blocking pool.
    reconcile_repo_statuses_detached(app.clone(), failed_active);

    Ok(all)
}

static NETWORK_REFILL_PENDING: std::sync::LazyLock<std::sync::Mutex<HashSet<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(HashSet::new()));
static NETWORK_REFILL_WORKER: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Network repositories never sit on the local fleet's critical path. One
/// low-concurrency lane coalesces their paths and may block on SMB without
/// delaying local cache publication or the command response.
fn schedule_network_repo_refills(app: AppHandle, paths: Vec<String>) {
    if paths.is_empty() {
        return;
    }
    if let Ok(mut pending) = NETWORK_REFILL_PENDING.lock() {
        pending.extend(paths);
    } else {
        return;
    }
    if NETWORK_REFILL_WORKER
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        )
        .is_err()
    {
        return;
    }
    std::thread::spawn(move || loop {
        let next = NETWORK_REFILL_PENDING.lock().ok().and_then(|mut pending| {
            let next = pending.iter().next().cloned();
            if let Some(path) = &next {
                pending.remove(path);
            }
            next
        });
        let Some(path) = next else {
            NETWORK_REFILL_WORKER.store(false, std::sync::atomic::Ordering::Release);
            // Close the enqueue-vs-exit race: a caller may have inserted just
            // before the flag changed. Claim the lane again in this same
            // worker rather than stranding that path until another summon.
            let has_pending = NETWORK_REFILL_PENDING
                .lock()
                .map(|pending| !pending.is_empty())
                .unwrap_or(false);
            if has_pending
                && NETWORK_REFILL_WORKER
                    .compare_exchange(
                        false,
                        true,
                        std::sync::atomic::Ordering::AcqRel,
                        std::sync::atomic::Ordering::Acquire,
                    )
                    .is_ok()
            {
                continue;
            }
            break;
        };
        let path_ref = Path::new(&path);
        if !path_ref.is_dir() {
            let _ = repo_exclusions::mark_repo_missing(&app, &path);
            continue;
        }
        watcher::refresh_repo_now(&app, path_ref);
    });
}

/// One in-flight pass at a time; a sweep that fires while a previous pass is
/// still probing (hung on a dead share) simply skips — the next sweep retries.
static REPO_STATUS_RECONCILE_INFLIGHT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

fn reconcile_repo_statuses_detached(app: AppHandle, failed_active: Vec<String>) {
    use std::sync::atomic::Ordering;
    if REPO_STATUS_RECONCILE_INFLIGHT
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    std::thread::spawn(move || {
        let _release = ReleaseOnDrop;
        struct ReleaseOnDrop;
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                REPO_STATUS_RECONCILE_INFLIGHT.store(false, std::sync::atomic::Ordering::SeqCst);
            }
        }

        let Ok(conn) = cache::open(&app) else {
            return;
        };

        // Walk-failed 'active' repos: flip to missing ONLY when the directory
        // is really gone (a transient git error with the dir present stays
        // active — same semantics as the startup verify pass).
        for path in failed_active {
            if Path::new(&path).is_dir() {
                continue;
            }
            let _ = repo_exclusions::mark_repo_missing(&app, &path);
        }

        // Recovery: probe missing repos; a drive that came back flips to
        // active so the next sweep walks it again.
        let missing: Vec<String> = conn
            .prepare(
                "SELECT path FROM repos \
                 WHERE status = 'missing' AND user_state NOT IN ('removed')",
            )
            .and_then(|mut st| {
                st.query_map([], |r| r.get::<_, String>(0))
                    .and_then(|rows| rows.collect())
            })
            .unwrap_or_default();
        drop(conn);
        for path in missing {
            if !Path::new(&path).is_dir() {
                continue;
            }
            let _ = repo_exclusions::recover_missing_repo(&app, &path);
        }
    });
}

#[tauri::command]
pub async fn recent_commits(
    app: AppHandle,
    window_days: Option<i64>,
) -> Result<Vec<git::CommitSummary>, String> {
    tauri::async_runtime::spawn_blocking(move || scan_and_cache_recent_commits(&app, window_days))
        .await
        .map_err(|e| e.to_string())?
}

/// Summon-path refill: the exact scan + reconcile + upsert of
/// `recent_commits`, returning only the merged row count. The windowed UI
/// re-pulls everything it shows from the cache, so the full row array the
/// old refill shipped over IPC — up to 5,000 CommitSummary structs per
/// panel summon — was serialized, parsed, and immediately garbage.
/// Serialize fleet scans: rapid summon/remount bursts must not run CONCURRENT
/// serial scans convoying on SQLite's write lock — but a queued scan must
/// still RUN (a skip would silently drop e.g. a window-widen refill issued
/// while the summon scan is mid-flight). Same-window duplicates are already
/// deduped frontend-side (useTimelineWindow's lastRefilledKey).
static REFILL_SERIALIZE: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// The schema deliberately leaves legacy rolling-cache text unindexed so
/// application startup stays independent of cache size. A cold first search
/// reads through the exact LIKE fallback, then schedules this single-flight
/// catch-up only after its result is snapshot-validated. Search never waits
/// for maintenance, and FTS only becomes a hard candidate gate after the
/// explicit coverage ledger says every eligible row is indexed.
static SEARCH_FTS_BACKFILL_INFLIGHT: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

struct SearchFtsBackfillLease;

impl Drop for SearchFtsBackfillLease {
    fn drop(&mut self) {
        SEARCH_FTS_BACKFILL_INFLIGHT.store(false, std::sync::atomic::Ordering::Release);
    }
}

fn schedule_search_index_backfill(
    app: &AppHandle,
    conn: &rusqlite::Connection,
    filters: &cache::TimelineFilters,
) {
    let should_use_fts = filters
        .query
        .as_deref()
        .and_then(history_query::substring_expression)
        .is_some();
    if !should_use_fts || history_fts::is_ready(conn) {
        return;
    }

    if SEARCH_FTS_BACKFILL_INFLIGHT
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::AcqRel,
            std::sync::atomic::Ordering::Acquire,
        )
        .is_err()
    {
        return;
    }

    let app = app.clone();
    std::thread::spawn(move || {
        let _lease = SearchFtsBackfillLease;
        // Let the sibling cold LIKE count/page calls finish before the first
        // writer, then leave a real scheduling gap between every short slice.
        std::thread::sleep(std::time::Duration::from_millis(75));
        let Ok(mut conn) = cache::open(&app) else {
            return;
        };
        while let Ok(outcome) = history_fts::backfill_batch(&mut conn, 256) {
            if !outcome.has_more {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(75));
        }
    });
}

#[tauri::command]
pub async fn refresh_recent_commits(
    app: AppHandle,
    window_days: Option<i64>,
) -> Result<usize, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let _serialized = REFILL_SERIALIZE
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        scan_and_cache_recent_commits(&app, window_days).map(|rows| rows.len())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Phase 1 windowed-pull API: one keyset-paginated page of the timeline.
#[tauri::command]
pub async fn list_commits_window(
    app: AppHandle,
    filters: cache::TimelineFilters,
    cursor: Option<cache::Cursor>,
    direction: cache::WindowDirection,
    limit: usize,
) -> Result<cache::CommitWindow, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<cache::CommitWindow, String> {
        let conn = cache::open(&app).map_err(|e| e.to_string())?;
        cache::ensure_timeline_snapshot_valid(&conn, &filters).map_err(|e| e.to_string())?;
        let result = cache::list_commits_window(&conn, &filters, cursor.as_ref(), direction, limit)
            .map_err(|e| e.to_string())?;
        cache::ensure_timeline_snapshot_valid(&conn, &filters).map_err(|e| e.to_string())?;
        schedule_search_index_backfill(&app, &conn, &filters);
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Phase 1 windowed-pull API: rows centred on an anchor cursor.
#[tauri::command]
pub async fn list_commits_around_anchor(
    app: AppHandle,
    filters: cache::TimelineFilters,
    anchor: cache::Cursor,
    before: usize,
    after: usize,
) -> Result<cache::CommitAround, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<cache::CommitAround, String> {
        let conn = cache::open(&app).map_err(|e| e.to_string())?;
        cache::ensure_timeline_snapshot_valid(&conn, &filters).map_err(|e| e.to_string())?;
        let result =
            cache::list_commits_around_anchor(&conn, &filters, &anchor, before, after, None)
                .map_err(|e| e.to_string())?;
        cache::ensure_timeline_snapshot_valid(&conn, &filters).map_err(|e| e.to_string())?;
        schedule_search_index_backfill(&app, &conn, &filters);
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Phase 9 windowed-pull API: a window centred on a 0-based rank — the
/// random-access scrollbar's jump-load.
#[tauri::command]
pub async fn list_commits_at_rank(
    app: AppHandle,
    filters: cache::TimelineFilters,
    rank: i64,
    before: usize,
    after: usize,
) -> Result<cache::CommitAround, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<cache::CommitAround, String> {
        let conn = cache::open(&app).map_err(|e| e.to_string())?;
        cache::ensure_timeline_snapshot_valid(&conn, &filters).map_err(|e| e.to_string())?;
        let result = cache::list_commits_at_rank(&conn, &filters, rank, before, after)
            .map_err(|e| e.to_string())?;
        cache::ensure_timeline_snapshot_valid(&conn, &filters).map_err(|e| e.to_string())?;
        schedule_search_index_backfill(&app, &conn, &filters);
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Phase 1 windowed-pull API: filtered total commit count for the scrollbar.
#[tauri::command]
pub async fn count_commits(app: AppHandle, filters: cache::TimelineFilters) -> Result<i64, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<i64, String> {
        let conn = cache::open(&app).map_err(|e| e.to_string())?;
        cache::ensure_timeline_snapshot_valid(&conn, &filters).map_err(|e| e.to_string())?;
        let result = cache::count_commits(&conn, &filters).map_err(|e| e.to_string())?;
        cache::ensure_timeline_snapshot_valid(&conn, &filters).map_err(|e| e.to_string())?;
        schedule_search_index_backfill(&app, &conn, &filters);
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Phase 2 windowed-pull API: the current commit generation. The frontend
/// reads this once and pins it as its `view_generation` (a field on
/// `TimelineFilters`) so the background scanner's later inserts never
/// disturb the page sequence it is showing.
#[tauri::command]
pub async fn get_timeline_generation(app: AppHandle) -> Result<i64, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<i64, String> {
        let conn = cache::open(&app).map_err(|e| e.to_string())?;
        cache::current_generation(&conn).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Phase 3/7 windowed-pull API: the AuthorsChip + RepoChip filter facets
/// (author tallies + per-repo commit counts) under `filters`. The windowed
/// timeline holds no full client-side commit array to tally from.
#[tauri::command]
pub async fn list_filter_facets(
    app: AppHandle,
    filters: cache::TimelineFilters,
    include_authors: Option<bool>,
    include_repos: Option<bool>,
) -> Result<cache::FilterFacets, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<cache::FilterFacets, String> {
        let conn = cache::open(&app).map_err(|e| e.to_string())?;
        cache::ensure_timeline_snapshot_valid(&conn, &filters).map_err(|e| e.to_string())?;
        let result = cache::list_filter_facets(
            &conn,
            &filters,
            include_authors.unwrap_or(true),
            include_repos.unwrap_or(true),
        )
        .map_err(|e| e.to_string())?;
        cache::ensure_timeline_snapshot_valid(&conn, &filters).map_err(|e| e.to_string())?;
        schedule_search_index_backfill(&app, &conn, &filters);
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn list_branches(repo_path: String) -> Result<Vec<git::BranchInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git::list_branches(Path::new(&repo_path)).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Lazily count reachable commits for the given branch refs — called by the
/// BranchChip for the rows currently on screen so opening the chip in a
/// thousand-branch repo stays instant.
#[tauri::command]
pub async fn count_branch_commits(
    repo_path: String,
    ref_names: Vec<String>,
) -> Result<Vec<git::BranchCommitCount>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git::count_branch_commits(Path::new(&repo_path), &ref_names).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn current_upstream_status(
    repo_path: String,
    branch_name: Option<String>,
) -> Result<Option<git::UpstreamStatus>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git::current_upstream_status(Path::new(&repo_path), branch_name.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Add a repo by path (drag-drop / paste). Wraps
/// discovery_orchestrator::add_repo_explicit and turns the
/// validation/Repository::discover error into a frontend-friendly
/// "Not a Git working tree" string. On success the orchestrator has
/// already emitted `timeline://repo-discovered` — we return the same
/// payload so the caller can also handle it synchronously.
#[tauri::command]
pub async fn explicit_add_repo(
    app: AppHandle,
    path: String,
) -> Result<discovery_orchestrator::DiscoveredRepoPayload, String> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        discovery_orchestrator::add_repo_explicit(&app2, &path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// List recoverable repository exclusions for the Settings window.
#[tauri::command]
pub async fn list_excluded_repos(
    app: AppHandle,
) -> Result<Vec<repo_exclusions::ExcludedRepo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        repo_exclusions::list_excluded_repos(&app).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryIndexStatus {
    total_repos: usize,
    complete_repos: usize,
    published_repos: usize,
    indexing_repos: usize,
    failed_repos: usize,
    unavailable_repos: usize,
    queued_repos: usize,
    commits_scanned: u64,
    generation: i64,
    settled: bool,
    all_complete: bool,
    last_error: Option<String>,
}

fn history_repo_scope(
    conn: &rusqlite::Connection,
    repo_ids: Option<&[i64]>,
) -> anyhow::Result<Vec<(String, String)>> {
    if repo_ids.is_some_and(|ids| ids.is_empty()) {
        return Ok(Vec::new());
    }
    let mut sql = String::from("SELECT path, status FROM repos WHERE user_state != 'removed'");
    if let Some(ids) = repo_ids {
        // IDs originate from this same catalog and are integers, so an
        // inline list avoids SQLite's variable limit for very large scopes.
        let ids = ids.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
        sql.push_str(&format!(" AND rowid IN ({ids})"));
    }
    sql.push_str(" ORDER BY path");
    let mut statement = conn.prepare(&sql)?;
    let paths = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(paths)
}

fn history_index_status(
    conn: &rusqlite::Connection,
    indexer: &history_indexer::HistoryIndexer,
    scope: &[(String, String)],
) -> anyhow::Result<HistoryIndexStatus> {
    let pending = indexer.pending_paths();
    let mut complete_repos = 0usize;
    let mut published_repos = 0usize;
    let mut indexing_repos = 0usize;
    let mut failed_repos = 0usize;
    let mut unavailable_repos = 0usize;
    let mut commits_scanned = 0u64;
    let mut visible_generation = 0i64;
    let mut last_error = None;

    for (path, status) in scope {
        if status != "active" {
            unavailable_repos += 1;
            continue;
        }
        let Some(coverage) = history_store::coverage(conn, path)? else {
            continue;
        };
        if coverage.published_epoch > 0 {
            published_repos += 1;
            visible_generation = visible_generation.max(coverage.completed_generation.unwrap_or(0));
        }
        match coverage.state {
            history_store::CoverageState::Complete => {
                complete_repos += 1;
                commits_scanned = commits_scanned.saturating_add(coverage.commits_indexed as u64);
            }
            history_store::CoverageState::Indexing => {
                indexing_repos += 1;
                commits_scanned = commits_scanned.saturating_add(coverage.commits_indexed as u64);
            }
            history_store::CoverageState::Failed => {
                failed_repos += 1;
                if last_error.is_none() {
                    last_error = coverage.last_error;
                }
            }
            history_store::CoverageState::Stale | history_store::CoverageState::Cancelled => {}
        }
    }
    let queued_repos = scope
        .iter()
        .filter(|(path, status)| status == "active" && pending.contains(path))
        .count();
    let settled = queued_repos == 0;
    let total_repos = scope.len();
    Ok(HistoryIndexStatus {
        total_repos,
        complete_repos,
        published_repos,
        indexing_repos,
        failed_repos,
        unavailable_repos,
        queued_repos,
        commits_scanned,
        // Staging batches are deliberately invisible until publication.
        // Reporting the global cache generation here made every 250ms poll
        // reload both timelines while a large repo was still private. Only a
        // completed publication is a user-visible history generation.
        generation: visible_generation,
        settled,
        // A previously complete repo is still pending until this demand has
        // revalidated its current roots/lifecycle. Never paint "All history"
        // from yesterday's coverage while that verification is queued.
        all_complete: settled && unavailable_repos == 0 && complete_repos == total_repos,
        last_error,
    })
}

/// Demand-driven full-history expansion. An all-time search or All time view
/// requests exactly the current repository scope; bounded 7/30-day search
/// remains independent of repository history size.
#[tauri::command]
pub async fn request_history_index(
    app: AppHandle,
    indexer: tauri::State<'_, Arc<history_indexer::HistoryIndexer>>,
    repo_ids: Option<Vec<i64>>,
    demand_id: u64,
) -> Result<HistoryIndexStatus, String> {
    if demand_id == 0 {
        return Err("history demand id must be positive".to_string());
    }
    let indexer = Arc::clone(indexer.inner());
    tauri::async_runtime::spawn_blocking(move || -> Result<HistoryIndexStatus, String> {
        let conn = cache::open(&app).map_err(|error| error.to_string())?;
        let scope =
            history_repo_scope(&conn, repo_ids.as_deref()).map_err(|error| error.to_string())?;
        let _accepted = indexer.replace_demand_scope(
            demand_id,
            scope
                .iter()
                .filter(|(_, status)| status == "active")
                .map(|(path, _)| path.clone()),
        );
        history_index_status(&conn, &indexer, &scope).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Release only the matching panel lease. A delayed cleanup from an older
/// React effect cannot cancel a newer search/all-time demand.
#[tauri::command]
pub fn release_history_index(
    indexer: tauri::State<'_, Arc<history_indexer::HistoryIndexer>>,
    demand_id: u64,
) -> bool {
    indexer.release_demand_scope(demand_id)
}

#[tauri::command]
pub async fn get_history_index_status(
    app: AppHandle,
    indexer: tauri::State<'_, Arc<history_indexer::HistoryIndexer>>,
    repo_ids: Option<Vec<i64>>,
) -> Result<HistoryIndexStatus, String> {
    let indexer = Arc::clone(indexer.inner());
    tauri::async_runtime::spawn_blocking(move || -> Result<HistoryIndexStatus, String> {
        let conn = cache::open(&app).map_err(|error| error.to_string())?;
        let scope =
            history_repo_scope(&conn, repo_ids.as_deref()).map_err(|error| error.to_string())?;
        history_index_status(&conn, &indexer, &scope).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// Exclude a repo from discovery/watching without touching its folder or
/// durable preferences. The Settings window can restore it later.
#[tauri::command]
pub async fn exclude_repo(app: AppHandle, canonical_path: String) -> Result<(), String> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        repo_exclusions::exclude_repo(&app2, &canonical_path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Restore a previously excluded repo and resume discovery/watching. A
/// disconnected path returns as `missing` so it can recover when available.
#[tauri::command]
pub async fn restore_repo(app: AppHandle, canonical_path: String) -> Result<(), String> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        repo_exclusions::restore_repo(&app2, &canonical_path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Accept the repository currently occupying an excluded path as a fresh
/// identity. Only gitwink metadata/cache is replaced; the folder is untouched.
#[tauri::command]
pub async fn inspect_replacement_repo(
    app: AppHandle,
    canonical_path: String,
) -> Result<repo_exclusions::ReplacementConfirmation, String> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        repo_exclusions::inspect_replacement_repo(&app2, &canonical_path).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn replace_excluded_repo(
    app: AppHandle,
    confirmation: repo_exclusions::ReplacementConfirmation,
) -> Result<(), String> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        repo_exclusions::replace_excluded_repo(&app2, &confirmation).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Phase 6 detail-tier cache: an in-memory LRU of `changed_files` results,
/// so expanding (or re-expanding) a commit doesn't recompute the git diff.
/// Bounded by entry count; the file list of a very large commit is skipped
/// (rare, and recomputing it on demand is fine). Lost on restart — it is a
/// pure cache. Managed in `lib.rs` and shared by `changed_files` +
/// `changed_files_batch`.
pub struct ChangedFilesCache(std::sync::Mutex<ChangedFilesLru>);

struct ChangedFilesLru {
    /// key (`repo_path\0hash`) → (truncated file list, TOTAL file count,
    /// last-access tick)
    entries: std::collections::HashMap<String, (Vec<git::ChangedFile>, usize, u64)>,
    tick: u64,
}

/// Max cached commits before LRU eviction kicks in.
const CHANGED_FILES_CACHE_CAP: usize = 256;
/// Ship (and cache) at most this many files per commit; the TOTAL count
/// rides along so the UI can say "showing N of M". The old behaviour —
/// refusing to cache >1000-file commits at all — made every viewport slide
/// near a vendor-bump commit recompute its full tree diff forever.
const CHANGED_FILES_LIST_CAP: usize = 1_500;
/// Upper bound on commits one `changed_files_batch` call will process.
const CHANGED_FILES_PREFETCH_CAP: usize = 100;

/// `changed_files` response: the (possibly capped) list plus the true total.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFilesResult {
    pub files: Vec<git::ChangedFile>,
    pub total: usize,
}

impl Default for ChangedFilesCache {
    fn default() -> Self {
        Self(std::sync::Mutex::new(ChangedFilesLru {
            entries: std::collections::HashMap::new(),
            tick: 0,
        }))
    }
}

impl ChangedFilesCache {
    fn key(repo_path: &str, hash: &str) -> String {
        format!("{repo_path}\0{hash}")
    }

    fn get(&self, repo_path: &str, hash: &str) -> Option<ChangedFilesResult> {
        let mut lru = self.0.lock().ok()?;
        lru.tick += 1;
        let tick = lru.tick;
        let entry = lru.entries.get_mut(&Self::key(repo_path, hash))?;
        entry.2 = tick;
        Some(ChangedFilesResult {
            files: entry.0.clone(),
            total: entry.1,
        })
    }

    fn contains(&self, repo_path: &str, hash: &str) -> bool {
        self.0
            .lock()
            .map(|lru| lru.entries.contains_key(&Self::key(repo_path, hash)))
            .unwrap_or(false)
    }

    /// Cache a (pre-capped) list + the commit's true total file count.
    fn put(&self, repo_path: &str, hash: &str, files: &[git::ChangedFile], total: usize) {
        let Ok(mut lru) = self.0.lock() else {
            return;
        };
        lru.tick += 1;
        let tick = lru.tick;
        lru.entries
            .insert(Self::key(repo_path, hash), (files.to_vec(), total, tick));
        // Evict least-recently-used entries down to the cap.
        while lru.entries.len() > CHANGED_FILES_CACHE_CAP {
            let victim = lru
                .entries
                .iter()
                .min_by_key(|(_, (_, _, t))| *t)
                .map(|(k, _)| k.clone());
            match victim {
                Some(k) => {
                    lru.entries.remove(&k);
                }
                None => break,
            }
        }
    }
}

/// Compute a commit's changed files and cap the shipped list, keeping the
/// true total for the "showing N of M" footer.
fn changed_files_capped(repo_path: &str, hash: &str) -> Result<ChangedFilesResult, String> {
    let mut files = git::changed_files(Path::new(repo_path), hash).map_err(|e| e.to_string())?;
    let total = files.len();
    files.truncate(CHANGED_FILES_LIST_CAP);
    Ok(ChangedFilesResult { files, total })
}

#[tauri::command]
pub async fn changed_files(
    app: AppHandle,
    repo_path: String,
    hash: String,
) -> Result<ChangedFilesResult, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<ChangedFilesResult, String> {
        if let Some(cache) = app.try_state::<ChangedFilesCache>() {
            if let Some(hit) = cache.get(&repo_path, &hash) {
                return Ok(hit);
            }
        }
        let result = changed_files_capped(&repo_path, &hash)?;
        if let Some(cache) = app.try_state::<ChangedFilesCache>() {
            cache.put(&repo_path, &hash, &result.files, result.total);
        }
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One commit reference for `changed_files_batch`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRef {
    pub repo_path: String,
    pub hash: String,
}

/// Phase 6 detail-tier prefetch: warm the `changed_files` cache for a set
/// of commits — the rows in/near the timeline viewport — so expanding one
/// is instant. Already-cached commits are skipped; the batch is capped so
/// a huge request can't run unbounded git work.
#[tauri::command]
pub async fn changed_files_batch(app: AppHandle, commits: Vec<CommitRef>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(cache) = app.try_state::<ChangedFilesCache>() else {
            return;
        };
        for c in commits.into_iter().take(CHANGED_FILES_PREFETCH_CAP) {
            if cache.contains(&c.repo_path, &c.hash) {
                continue;
            }
            if let Ok(r) = changed_files_capped(&c.repo_path, &c.hash) {
                cache.put(&c.repo_path, &c.hash, &r.files, r.total);
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn commit_file_blobs(
    repo_path: String,
    hash: String,
    file_path: String,
    old_path: Option<String>,
) -> Result<git::CommitFileBlobs, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git::commit_file_blobs(
            Path::new(&repo_path),
            &hash,
            &file_path,
            old_path.as_deref(),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Default unified-diff context. Only this size is cached — it's the hot
/// path. Expanded-context requests (±25 / whole-file) are on-demand and
/// rare, so they skip the cache rather than bloating its key with the
/// context size.
const DEFAULT_DIFF_CONTEXT: u32 = 3;

#[tauri::command]
pub async fn file_diff(
    app: AppHandle,
    repo_path: String,
    hash: String,
    file_path: String,
    context_lines: u32,
) -> Result<String, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        let cacheable = context_lines == DEFAULT_DIFF_CONTEXT;
        // 1. Cache hit? (default context only)
        if cacheable {
            if let Ok(conn) = cache::open(&app) {
                if let Ok(Some(text)) = cache::get_diff(&conn, &repo_path, &hash, &file_path) {
                    return Ok(text);
                }
            }
        }
        // 2. Compute, persist (default context only), return.
        let text = git::file_diff(Path::new(&repo_path), &hash, &file_path, context_lines)
            .map_err(|e| e.to_string())?;
        if cacheable {
            if let Ok(conn) = cache::open(&app) {
                let _ = cache::put_diff(&conn, &repo_path, &hash, &file_path, &text);
            }
        }
        Ok(text)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiffOpenPayload {
    pub repo_path: String,
    pub repo_name: String,
    pub hash: String,
    pub short_hash: String,
    pub summary: String,
    pub file_path: String,
}

/// Tauri-managed slot: the diff window pulls this on mount, then we rely on
/// the "diff://open" event for subsequent file picks while it's already up.
pub type PendingDiff = std::sync::Mutex<Option<DiffOpenPayload>>;

/// Peek (not take) the latest payload. The state holds whatever was passed
/// to open_diff most recently; the diff window calls this on every mount
/// to seed its UI. React StrictMode in dev mounts useEffect twice, so a
/// destructive `take()` here would lose the payload on the second mount.
#[tauri::command]
pub fn take_pending_diff_open(state: tauri::State<'_, PendingDiff>) -> Option<DiffOpenPayload> {
    state.lock().ok().and_then(|s| s.clone())
}

/// Explicit "close everything" — hides the main panel AND the diff window
/// (the ✕ button and any global dismiss action both route here).
#[tauri::command]
pub fn dismiss_panel(app: AppHandle) {
    if let Some(panel) = app.get_webview_window("panel") {
        let _ = panel.hide();
    }
    // Restore the default always-on-top for the current mode (glance →
    // true; pinned → false) so a later summon lands in the right state
    // regardless of what open_diff / open_settings left behind. assert
    // no-ops if the panel window is somehow gone.
    window::assert_panel_always_on_top(&app);
    if let Some(diff) = app.get_webview_window("diff") {
        let _ = diff.hide();
    }
}

#[tauri::command]
pub fn get_panel_window_state(app: AppHandle) -> panel_window_state::PanelWindowStatePayload {
    panel_window_state::get_panel_window_state(&app)
}

#[tauri::command]
pub fn toggle_panel_maximized(
    app: AppHandle,
) -> Result<panel_window_state::PanelWindowStatePayload, String> {
    panel_window_state::toggle_panel_maximized(&app)
}

#[tauri::command]
pub fn set_panel_size_preset(
    app: AppHandle,
    preset: panel_window_state::PanelSizePreset,
) -> Result<panel_window_state::PanelWindowStatePayload, String> {
    panel_window_state::apply_panel_size_preset(&app, preset)
}

/// Whether the panel should resist blur-dismiss. The frontend sets this
/// true while the empty-state add-repo screen is showing or a native
/// folder picker is open — both legitimately move focus off the panel
/// without the user wanting it closed. The blur handler in `lib.rs`
/// checks this before hiding.
pub struct PanelSticky(pub std::sync::atomic::AtomicBool);

impl Default for PanelSticky {
    fn default() -> Self {
        Self(std::sync::atomic::AtomicBool::new(false))
    }
}

/// Whether the panel is "pinned" — true disables blur-dismiss and
/// drops always-on-top. The taskbar entry promise was dropped during
/// the WebView2 stability arc (runtime set_skip_taskbar destabilises
/// subsequent window builds on Windows); the panel always honours
/// tauri.conf.json's skipTaskbar=true regardless of pin state.
/// Source of truth for the runtime check in lib.rs's blur handler +
/// window::assert_panel_always_on_top; mirrors settings.panel_pinned
/// across launches.
pub struct PanelPinned(pub std::sync::atomic::AtomicBool);

impl Default for PanelPinned {
    fn default() -> Self {
        Self(std::sync::atomic::AtomicBool::new(false))
    }
}

/// In-memory mirror of the on-disk settings, seeded at startup and kept
/// up to date by every settings setter + the broadcast hot path. Solves
/// the "new window opens during the 250ms persist debounce and reads
/// stale settings.json" race (GPT Pro D1): `get_settings` reads this
/// snapshot instead of hitting disk, so the broadcast that already
/// updated live state wins even if the debounced disk write hasn't
/// fired yet. Disk is still the source of truth across restarts;
/// LiveSettings is just a write-through cache that closes the
/// in-session window.
pub struct LiveSettings(pub std::sync::RwLock<settings::Settings>);

impl LiveSettings {
    /// Build with the current disk state. Called once at startup from
    /// lib.rs setup; later sets mutate the same RwLock in place.
    pub fn from_disk(app: &AppHandle) -> Self {
        Self(std::sync::RwLock::new(settings::load(app)))
    }

    /// Read a clone of the cached Settings. Cloning under the read lock
    /// keeps the critical section tiny — callers project to AppSettings
    /// (or just inspect fields) without holding the lock.
    pub fn snapshot(&self) -> settings::Settings {
        self.0.read().map(|g| g.clone()).unwrap_or_default()
    }

    /// Mutate the cached Settings in place. Returns silently on a
    /// poisoned lock; the disk write is the durable path, so a poisoned
    /// in-memory cache just degrades to "next get_settings sees the
    /// pre-poison snapshot" which is acceptable.
    fn with_mut<F: FnOnce(&mut settings::Settings)>(&self, f: F) {
        if let Ok(mut g) = self.0.write() {
            f(&mut *g);
        }
    }
}

#[tauri::command]
pub fn set_panel_sticky(sticky: bool, state: tauri::State<'_, PanelSticky>) {
    state.0.store(sticky, std::sync::atomic::Ordering::SeqCst);
}

/// Whether a discovery run is currently in flight. The frontend pulls
/// this on startup so the "Scanning…" indicator is correct even when
/// the orchestrator finished before the scan-progress listener was
/// registered.
#[tauri::command]
pub fn get_scan_state(state: tauri::State<'_, discovery_orchestrator::ScanState>) -> bool {
    state.0.load(std::sync::atomic::Ordering::SeqCst)
}

/// Open (or focus) the diff window for one file revision. Thin
/// dispatcher: stores the payload in PendingDiff so the diff window's
/// initial mount can pick it up, drops the panel's always-on-top so
/// the diff isn't trapped behind it, then hands off to
/// `window::open_or_focus_diff` on the main thread via
/// `run_on_main_thread` + a oneshot so the IPC future resolves to the
/// build result instead of swallowing it. Mirrors `open_settings_window`
/// and addresses GPT Pro review B1 (wrong-thread WebView build + missing
/// single-flight guard for "diff" label).
#[tauri::command]
pub async fn open_diff(
    app: AppHandle,
    repo_path: String,
    repo_name: String,
    hash: String,
    short_hash: String,
    summary: String,
    file_path: String,
) -> Result<(), String> {
    let payload = DiffOpenPayload {
        repo_path,
        repo_name,
        hash,
        short_hash,
        summary,
        file_path,
    };

    if let Some(state) = app.try_state::<PendingDiff>() {
        if let Ok(mut s) = state.lock() {
            *s = Some(payload.clone());
        }
    }

    // The panel is always-on-top; drop that while the diff window is up
    // so the diff isn't trapped behind it. Restored when the diff hides
    // (lib.rs CloseRequested) or the panel is next summoned (window.rs).
    if let Some(panel) = app.get_webview_window("panel") {
        let _ = panel.set_always_on_top(false);
    }

    // capacity-1 mpsc as a one-shot — we want the main-thread closure
    // to send its Result back so this command's future resolves with
    // the build outcome rather than swallowing it. Tauri's
    // `async_runtime::channel` is tokio mpsc under the hood; try_send
    // never blocks because we send exactly once.
    let (tx, mut rx) = tauri::async_runtime::channel::<Result<(), String>>(1);
    let app2 = app.clone();
    if let Err(error) = app.run_on_main_thread(move || {
        let _ = tx.try_send(window::open_or_focus_diff(&app2, payload));
    }) {
        window::assert_panel_always_on_top(&app);
        return Err(error.to_string());
    }
    let Some(result) = rx.recv().await else {
        window::assert_panel_always_on_top(&app);
        return Err("diff dispatch dropped".to_string());
    };
    if result.is_err() {
        // If the child never became visible, restore glance stacking. If it
        // did become visible and only a later payload emit failed, the shared
        // invariant correctly keeps the panel lowered.
        window::assert_panel_always_on_top(&app);
    }
    result
}

/// Commits that changed `file_path` in `repo_path`, newest first. Live, capped
/// libgit2 walk (see `git::file_history`); off-UI. Backs the file-history scope
/// in the panel timeline.
#[tauri::command]
pub async fn file_history(
    app: AppHandle,
    jobs: tauri::State<'_, Arc<file_history_jobs::FileHistoryJobs>>,
    repo_path: String,
    file_path: String,
    flow_id: String,
    operation_id: String,
) -> Result<Option<git::FileHistoryResult>, String> {
    // Claim the flow before entering spawn_blocking. A new command therefore
    // cancels the old revwalk synchronously, even if the old React cleanup IPC
    // is delayed or dropped during navigation.
    let jobs = Arc::clone(jobs.inner());
    let operation = jobs.begin(flow_id, operation_id)?;
    tauri::async_runtime::spawn_blocking(move || {
        if operation.is_cancelled() {
            jobs.finish(&operation);
            return Ok(None);
        }
        let lifecycle_fence = {
            let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
            let conn = cache::open(&app).map_err(|error| error.to_string())?;
            discovery_orchestrator::capture_active_repo_lifecycle_fence(&conn, &repo_path)
                .map_err(|error| error.to_string())?
                .ok_or_else(|| "Repository is excluded or unavailable".to_string())?
        };
        // Notify's in-memory ref epoch is an O(1), cancellation-safe snapshot
        // key. Root equality is independently rechecked inside the walk, so
        // no slow repository-open preflight can pile up when navigation
        // supersedes an unavailable SMB path.
        let event_epoch = app
            .try_state::<watcher::RepoWatcher>()
            .map(|watcher| watcher.event_epoch(Path::new(&repo_path)))
            .unwrap_or(0);
        let key = file_history_jobs::FileHistoryJobKey::new(
            repo_path.clone(),
            file_path.clone(),
            format!("event:{event_epoch}"),
        );
        let result = jobs.execute(operation, key, |cancellation| {
            for attempt in 0..2 {
                let result =
                    git::file_history_cancellable(Path::new(&repo_path), &file_path, cancellation)
                        .map_err(|error| error.to_string())?;
                match result {
                    Some(result) if result.snapshot_changed && attempt == 0 => continue,
                    Some(result) if result.snapshot_changed => {
                        return Err(
                            "Repository changed repeatedly while loading file history; retry"
                                .to_string(),
                        )
                    }
                    other => return Ok(other),
                }
            }
            unreachable!("file-history retry loop returns from every iteration")
        });
        if matches!(result, Ok(Some(_))) {
            let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
            let conn = cache::open(&app).map_err(|error| error.to_string())?;
            if !discovery_orchestrator::repo_lifecycle_fence_matches(&conn, &lifecycle_fence)
                .map_err(|error| error.to_string())?
            {
                return Err("Repository lifecycle changed while loading; retry".to_string());
            }
        }
        result
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Best-effort cleanup for an effect that unmounted or navigated away. The
/// operation id makes this ABA-safe: a late cleanup cannot cancel the newer
/// request occupying the same UI flow.
#[tauri::command]
pub fn cancel_file_history(
    jobs: tauri::State<'_, Arc<file_history_jobs::FileHistoryJobs>>,
    flow_id: String,
    operation_id: String,
) {
    jobs.cancel(&flow_id, &operation_id);
}

/// Payload for `history://open` — tells the panel to scope its timeline to one
/// file's history.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileHistoryOpen {
    pub repo_path: String,
    pub file_path: String,
}

/// From the diff window: bring the panel forward and ask it to scope its
/// timeline to this file's history. The panel webview is always mounted, so a
/// direct emit reaches its listener; we show + focus so the user lands on it.
#[tauri::command]
pub fn open_file_history(app: AppHandle, repo_path: String, file_path: String) {
    if let Some(panel) = app.get_webview_window("panel") {
        let _ = panel.show();
        let _ = panel.set_focus();
    }
    let _ = app.emit_to(
        "panel",
        "history://open",
        FileHistoryOpen {
            repo_path,
            file_path,
        },
    );
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoCommitsResult {
    pub rows: Vec<git::CommitSummary>,
    pub truncated: bool,
    pub incomplete: bool,
    pub limit: usize,
    pub shallow: bool,
}

#[tauri::command]
pub async fn repo_commits(
    app: AppHandle,
    repo_path: String,
    branches: Option<Vec<String>>,
    window_days: Option<i64>,
) -> Result<RepoCommitsResult, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<RepoCommitsResult, String> {
        let lifecycle_fence = {
            let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
            let conn = cache::open(&app).map_err(|e| e.to_string())?;
            if discovery_orchestrator::repo_is_excluded(&conn, &repo_path).unwrap_or(true) {
                return Err("Repository is excluded".to_string());
            }
            discovery_orchestrator::capture_active_repo_lifecycle_fence(&conn, &repo_path)
                .map_err(|e| e.to_string())?
                .ok_or_else(|| "Repository is no longer registered".to_string())?
        };
        let cutoff = cutoff_for(window_days);
        let shallow = git::repo_is_shallow(Path::new(&repo_path)).map_err(|e| e.to_string())?;
        let cap = single_repo_cap(window_days);
        let branches_slice: Option<&[String]> = branches.as_deref();
        // Ask for one sentinel beyond the presentation cap. Without it,
        // `len == cap` cannot distinguish an exact scope from a silently
        // truncated one, and the UI could falsely call 2,000 rows "All time".
        let mut attempts = 0usize;
        let scan = loop {
            attempts += 1;
            let refs_before = git::repo_refs_fingerprint(Path::new(&repo_path), branches_slice)
                .map_err(|e| e.to_string())?;
            let scan = git::repo_commits(
                Path::new(&repo_path),
                branches_slice,
                cap.saturating_add(1),
                cutoff,
            )
            .map_err(|e| e.to_string())?;
            let refs_after = git::repo_refs_fingerprint(Path::new(&repo_path), branches_slice)
                .map_err(|e| e.to_string())?;
            if refs_before == refs_after {
                break scan;
            }
            if attempts >= 2 {
                return Err("Repository changed repeatedly while loading; retry".to_string());
            }
        };
        let mut commits = scan.rows;
        let truncated = commits.len() > cap;
        if truncated {
            commits.truncate(cap);
        }
        // Persist into the same commits cache so warm starts paint instantly
        // for this repo too.
        if !commits.is_empty() {
            let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
            if let Ok(mut conn) = cache::open(&app) {
                if !discovery_orchestrator::repo_lifecycle_fence_matches(&conn, &lifecycle_fence)
                    .unwrap_or(false)
                {
                    return Err("Repository lifecycle changed while loading; retry".to_string());
                }
                let _ = cache::upsert_commits(&mut conn, &commits);
            }
        }
        Ok(RepoCommitsResult {
            rows: commits,
            truncated,
            incomplete: scan.incomplete,
            limit: cap,
            shallow,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Cheap change probe for the single-repo view — see
/// `git::repo_refs_fingerprint`. The frontend compares this before a
/// background re-pull and skips the multi-MB commit re-ship when nothing
/// moved.
#[tauri::command]
pub async fn repo_refs_fingerprint(
    repo_path: String,
    branches: Option<Vec<String>>,
) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git::repo_refs_fingerprint(Path::new(&repo_path), branches.as_deref())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn get_pinned_repos(app: AppHandle) -> Vec<String> {
    settings::load(&app).pinned_repos
}

#[tauri::command]
pub fn set_pinned_repos(app: AppHandle, repos: Vec<String>) {
    settings::save_pinned_repos(&app, repos);
}

/// Saved branch selection for one repo — restored when the user
/// re-enters single-repo mode. An empty result means "all branches".
#[tauri::command]
pub fn get_branch_selection(app: AppHandle, repo_path: String) -> Vec<String> {
    settings::load(&app)
        .branch_selections
        .get(&repo_path)
        .cloned()
        .unwrap_or_default()
}

#[tauri::command]
pub fn set_branch_selection(app: AppHandle, repo_path: String, selection: Vec<String>) {
    settings::save_branch_selection(&app, &repo_path, selection);
}

// ----- app settings (settings window) -----

/// UI-scale bounds. The floor is 100% — the diff/timeline default is
/// already the most compact legible size, so going smaller would hurt
/// readability, which is the whole point of the control.
pub const UI_SCALE_MIN: f32 = 1.0;
pub const UI_SCALE_MAX: f32 = 1.6;

/// The user-facing slice of settings the Settings window reads + writes.
/// camelCase so it maps straight onto the TypeScript `AppSettings`.
/// Deserialize is for the `set_live_settings` command path, which
/// accepts an incoming AppSettings from the broadcast hot path.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    /// UI scale multiplier; 1.0 = default.
    pub ui_scale: f32,
    /// Diff font family, or `None` for the built-in monospace stack.
    pub diff_font_family: Option<String>,
    /// Effective global hotkey spec — the resolved default if unset.
    pub panel_hotkey: String,
    /// When true, the panel is "pinned" — no blur auto-hide and normal
    /// stacking. The tray/accessory panel remains absent from the taskbar.
    pub panel_pinned: bool,
    /// One-shot `git fetch` of the viewed repo on panel summon (single-repo
    /// mode only). Default ON. See `maybe_fetch_repo` / `fetch.rs`.
    pub auto_fetch_on_show: bool,
    /// Whether the one-time auto-fetch disclosure banner has been dismissed.
    /// Default false (show it once). Persisted via `ack_auto_fetch_notice`.
    pub auto_fetch_notice_seen: bool,
    /// Self-update behaviour. Serialized as "enabled" / "manual" /
    /// "disabled" via the enum's lowercase rename.
    pub update_check: settings::UpdateCheckMode,
    /// Stored choice (`system` / `en` / `ko` / `ja`).
    pub locale_preference: crate::locale::LocalePreference,
    /// Backend-resolved locale used to load one frontend catalog.
    pub resolved_locale: crate::locale::AppLocale,
    /// Computed update capabilities; accepted on the live-broadcast input but
    /// never trusted or persisted. Backend commands use their own runtime
    /// policy snapshot.
    pub update_policy: update::UpdatePolicyView,
}

#[tauri::command]
pub fn get_settings(app: AppHandle, live: tauri::State<'_, LiveSettings>) -> AppSettings {
    // Read the in-memory snapshot, NOT disk — the Settings window's
    // broadcast hot path already updated LiveSettings synchronously,
    // so a newly-opened window's initSettings sees the fresh value
    // even when the debounced disk write hasn't landed yet.
    let s = live.snapshot();
    let locale_preference = s.locale;
    AppSettings {
        ui_scale: s.ui_scale.unwrap_or(1.0).clamp(UI_SCALE_MIN, UI_SCALE_MAX),
        diff_font_family: s.diff_font_family,
        panel_hotkey: s
            .panel_hotkey
            .filter(|h| !h.trim().is_empty())
            .unwrap_or_else(|| settings::DEFAULT_PANEL_HOTKEY.to_string()),
        panel_pinned: s.panel_pinned.unwrap_or(false),
        auto_fetch_on_show: s.auto_fetch_on_show.unwrap_or(true),
        auto_fetch_notice_seen: s.auto_fetch_notice_seen.unwrap_or(false),
        update_check: s.update_check,
        locale_preference,
        resolved_locale: crate::locale::resolve_system(locale_preference),
        update_policy: update::policy_view(&app),
    }
}

/// Project an incoming AppSettings (the user-facing slice the frontend
/// passes via broadcastSettings) into the live Settings cache. Skips
/// `update_policy` (computed) and re-clamps `ui_scale` defensively
/// so an out-of-range value from a stale frontend can't poison the
/// cache. Hotkey persistence still goes through `set_panel_hotkey`
/// because re-binding the global shortcut has to succeed before we
/// commit; the broadcast just keeps the displayed text aligned.
#[tauri::command]
pub fn set_live_settings(next: AppSettings, live: tauri::State<'_, LiveSettings>) {
    live.with_mut(|s| {
        s.ui_scale = Some(next.ui_scale.clamp(UI_SCALE_MIN, UI_SCALE_MAX));
        s.diff_font_family = next.diff_font_family;
        s.panel_hotkey = Some(next.panel_hotkey);
        s.panel_pinned = Some(next.panel_pinned);
        s.auto_fetch_on_show = Some(next.auto_fetch_on_show);
        // One-way latch: a stale Settings window (opened before the banner
        // was dismissed) must never un-acknowledge it. Only false→true.
        if next.auto_fetch_notice_seen {
            s.auto_fetch_notice_seen = Some(true);
        }
        s.update_check = next.update_check;
        s.locale = next.locale_preference;
    });
}

/// Persist the UI scale and re-derive the normal panel allocation from its
/// independent 100%-scale size preference. A maximized panel keeps its work-
/// area geometry and only the web content scales until restore.
/// Clamped to [UI_SCALE_MIN, UI_SCALE_MAX] — the slider enforces the
/// same bounds; this is the hand-edit backstop. The resize lags slider
/// drags by Settings.tsx's persist debounce (a knob, easy to remove).
#[tauri::command]
pub fn set_ui_scale(app: AppHandle, scale: f32) {
    let clamped = scale.clamp(UI_SCALE_MIN, UI_SCALE_MAX);
    settings::save_ui_scale(&app, Some(clamped));
    if let Some(live) = app.try_state::<LiveSettings>() {
        live.with_mut(|s| s.ui_scale = Some(clamped));
    }
    panel_window_state::apply_ui_scale(&app, clamped as f64);
}

/// Persist the diff font family. An empty/whitespace value clears it,
/// falling back to the built-in monospace stack.
#[tauri::command]
pub fn set_diff_font(app: AppHandle, family: Option<String>) {
    let cleaned = family
        .map(|f| f.trim().to_string())
        .filter(|f| !f.is_empty());
    settings::save_diff_font_family(&app, cleaned.clone());
    if let Some(live) = app.try_state::<LiveSettings>() {
        live.with_mut(|s| s.diff_font_family = cleaned);
    }
}

/// Re-bind the global panel hotkey live — no restart. Validates the spec,
/// drops the old binding, registers the new one, and only then persists.
/// On failure (unparseable, or already held by another app — Windows
/// registers globally, first-bind wins) the previous binding is restored
/// and the error returned for inline feedback.
#[tauri::command]
pub fn set_panel_hotkey(app: AppHandle, spec: String) -> Result<(), String> {
    use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut};

    let trimmed = spec.trim();
    let new_shortcut: Shortcut = trimmed
        .parse()
        .map_err(|e| format!("Not a valid shortcut: {e}"))?;

    let gs = app.global_shortcut();
    // gitwink only ever holds one global shortcut — drop it wholesale.
    let _ = gs.unregister_all();
    match gs.register(new_shortcut) {
        Ok(()) => {
            settings::save_panel_hotkey(&app, Some(trimmed.to_string()));
            if let Some(live) = app.try_state::<LiveSettings>() {
                live.with_mut(|s| s.panel_hotkey = Some(trimmed.to_string()));
            }
            Ok(())
        }
        Err(e) => {
            // Re-bind the previous spec so the user is never left hotkey-less.
            let prev = settings::load(&app)
                .panel_hotkey
                .filter(|h| !h.trim().is_empty())
                .unwrap_or_else(|| settings::DEFAULT_PANEL_HOTKEY.to_string());
            if let Ok(s) = prev.parse::<Shortcut>() {
                let _ = gs.register(s);
            }
            Err(format!("Couldn't bind {trimmed} — {e}"))
        }
    }
}

/// Toggle the panel between glance (blur-dismiss + always-on-top —
/// the default) and pinned (no auto-hide + normal stacking). The
/// panel stays skipTaskbar=true in both modes; see the PanelPinned
/// struct doc above for why the taskbar promise was dropped.
/// Persisted so the choice survives across launches. The caller is
/// responsible for broadcastSettings — see App.tsx.
#[tauri::command]
pub fn set_panel_pinned(app: AppHandle, pinned: bool) -> Result<(), String> {
    // Persist FIRST. If the write fails (read-only config dir, disk
    // full), refuse the toggle entirely so the UI and the next launch
    // agree (GPT Pro review A3 caveat).
    settings::save_panel_pinned(&app, pinned).map_err(|e| e.to_string())?;
    if let Some(live) = app.try_state::<LiveSettings>() {
        live.with_mut(|s| s.panel_pinned = Some(pinned));
    }
    if let Some(state) = app.try_state::<PanelPinned>() {
        state.0.store(pinned, std::sync::atomic::Ordering::SeqCst);
    }
    // Runtime toggle: ONLY recompute always_on_top (safe Win32 toggle). We
    // deliberately do NOT touch set_skip_taskbar here — that mutates
    // WS_EX_TOOLWINDOW / WS_EX_APPWINDOW at runtime, which on Windows
    // is a known WebView2 destabiliser (subsequent window builds can
    // come up blank). The blur-dismiss behaviour change is live
    // regardless — it's just an atomic check.
    window::apply_panel_pinned(&app, pinned);
    Ok(())
}

/// Open (or focus) the settings window from the frontend — used by the
/// panel header's right-click context menu, mirroring the tray entry.
/// async + run_on_main_thread so window creation is explicitly
/// dispatched to the UI thread rather than running on whatever worker
/// the sync command was scheduled on (sync commands building windows
/// have been observed to hang on Windows / WebView2). Returns the
/// build Result so a frontend "Settings…" click that silently fails
/// surfaces in the console instead of looking successful
/// (GPT Pro review B3).
#[tauri::command]
pub async fn open_settings_window(app: AppHandle) -> Result<(), String> {
    let (tx, mut rx) = tauri::async_runtime::channel::<Result<(), String>>(1);
    let app2 = app.clone();
    app.run_on_main_thread(move || {
        let _ = tx.try_send(window::open_settings(&app2));
    })
    .map_err(|e| e.to_string())?;
    rx.recv()
        .await
        .ok_or_else(|| "settings dispatch dropped".to_string())?
}

/// Persist the self-update mode and refresh the tray dot immediately
/// (Disabled hides the indicator + tray "Check for updates" item via
/// build_menu; refresh_indicator drives the rebuild).
#[tauri::command]
pub fn set_update_check(app: AppHandle, mode: settings::UpdateCheckMode) {
    settings::save_update_check_mode(&app, mode);
    if let Some(live) = app.try_state::<LiveSettings>() {
        live.with_mut(|s| s.update_check = mode);
    }
    // E3: clear the cached available-update snapshot when switching
    // off so a stale version can't re-appear the moment the user
    // toggles back to Enabled/Manual.
    if mode == settings::UpdateCheckMode::Disabled {
        update::clear_cached_available(&app);
    }
    update::refresh_indicator(&app);
    // E1: wake the updater so flipping to Enabled doesn't silently
    // wait up to 24h for the background loop's next tick.
    if mode == settings::UpdateCheckMode::Enabled {
        update::check_now_background(&app);
    }
}

/// Persist and apply a language choice without restarting. The returned
/// resolved locale lets every webview load the right lazy catalog; rebuilding
/// the update indicator also rebuilds the native tray menu and tooltip.
#[tauri::command]
pub fn set_locale(
    app: AppHandle,
    preference: crate::locale::LocalePreference,
) -> crate::locale::AppLocale {
    settings::save_locale(&app, preference);
    if let Some(live) = app.try_state::<LiveSettings>() {
        live.with_mut(|s| s.locale = preference);
    }
    update::refresh_indicator(&app);
    crate::locale::resolve_system(preference)
}

/// Persist the auto-fetch-on-show toggle and update the live cache. No
/// runtime side effects — the fetch itself fires from `maybe_fetch_repo`
/// on the next panel summon.
#[tauri::command]
pub fn set_auto_fetch_on_show(app: AppHandle, enabled: bool) {
    settings::save_auto_fetch_on_show(&app, enabled);
    if let Some(live) = app.try_state::<LiveSettings>() {
        live.with_mut(|s| s.auto_fetch_on_show = Some(enabled));
    }
}

/// Mark the one-time auto-fetch disclosure banner as dismissed. Persists and
/// updates the live cache so it never reappears (and so a Settings window
/// opened later reflects it). No network or fetch side effects.
#[tauri::command]
pub fn ack_auto_fetch_notice(app: AppHandle) {
    settings::save_auto_fetch_notice_seen(&app, true);
    if let Some(live) = app.try_state::<LiveSettings>() {
        live.with_mut(|s| s.auto_fetch_notice_seen = Some(true));
    }
}

/// Called by the panel on summon (single-repo mode) with the viewed repo.
/// Gated on the `auto_fetch_on_show` setting + per-repo cooldown + a global
/// concurrency cap, then spawns a non-blocking system `git fetch origin`.
/// Returns immediately; the fetch runs detached and its
/// `refs/remotes/origin/*` + `FETCH_HEAD` update flows through the existing
/// watcher → timeline refresh. Silent no-op when disabled / on cooldown /
/// path empty. NOTE: this and `fetch_repo_now` (the manual ↻ button) are the
/// only places gitwink reaches a git remote — both run the same read-only
/// one-shot; it never merges, pushes, or rewrites, so it cannot alter your
/// work.
#[tauri::command]
pub fn maybe_fetch_repo(app: AppHandle, repo_path: String) {
    if repo_path.trim().is_empty() {
        return;
    }
    // Fail closed: if the live-settings state is somehow missing, do NOT
    // reach the network. An UNSET setting still resolves to the product
    // default (on); only managed-state absence is the closed case.
    let Some(live) = app.try_state::<LiveSettings>() else {
        return;
    };
    if !live.snapshot().auto_fetch_on_show.unwrap_or(true) {
        return;
    }
    let Some(cooldown) = app.try_state::<crate::fetch::FetchCooldown>() else {
        return;
    };
    let repo = std::path::PathBuf::from(&repo_path);
    // Claim a per-repo cooldown slot + a global concurrency slot. The guard
    // is moved into the blocking task and releases the slot when it returns.
    let Some(guard) = cooldown.try_claim(&repo, crate::fetch::FETCH_COOLDOWN) else {
        return;
    };
    tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        // Auto path: fire-and-forget — outcome intentionally ignored.
        let _ = crate::fetch::git_fetch_one_shot(&repo);
    });
}

/// Explicit, user-initiated fetch — the ↻ button next to the upstream badge.
/// Unlike `maybe_fetch_repo` this bypasses the `auto_fetch_on_show` setting
/// (a manual click IS the alternative for people who turned auto off) and the
/// per-repo cooldown (the user asked *now*), but still respects the global
/// concurrency cap and stamps the cooldown so the next auto pass waits a full
/// window. Runs the exact same hardened one-shot fetch, awaits completion,
/// and reports: "ok" (git exited 0), "failed" (it didn't), "busy" (cap full).
#[tauri::command]
pub async fn fetch_repo_now(app: AppHandle, repo_path: String) -> Result<String, String> {
    if repo_path.trim().is_empty() {
        return Err("empty repo path".into());
    }
    let Some(cooldown) = app.try_state::<crate::fetch::FetchCooldown>() else {
        return Err("fetch state unavailable".into());
    };
    let repo = std::path::PathBuf::from(&repo_path);
    let Some(guard) = cooldown.claim_forced(&repo) else {
        return Ok("busy".into());
    };
    let ok = tauri::async_runtime::spawn_blocking(move || {
        let _guard = guard;
        crate::fetch::git_fetch_one_shot(&repo)
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(if ok { "ok".into() } else { "failed".into() })
}

/// Reveal `settings.json` in the user's default editor (or the OS file
/// handler for `.json`). Exposed for the "Open settings.json" link in
/// the Settings window's footer — the tray menu used to host this but
/// it was demoted to keep the tray to one Settings entry.
#[tauri::command]
pub fn open_settings_file(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    // ensure_path writes a default file if missing so the editor never
    // opens to a "file not found" dialog on a fresh install.
    let path = settings::ensure_path(&app).map_err(|e| e.to_string())?;
    let path_str = path.to_string_lossy().into_owned();
    app.opener()
        .open_path(&path_str, None::<&str>)
        .map_err(|e| e.to_string())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ----- self-update -----

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatePayload {
    /// The pending update, or `None` when up to date / not yet checked.
    pub available: Option<update::AvailableUpdate>,
    pub policy: update::UpdatePolicyView,
}

/// Snapshot the updater state for the modal, including the authoritative
/// installer channel that owns the update action.
#[tauri::command]
pub fn update_get_state(app: AppHandle) -> UpdateStatePayload {
    // Never panic the IPC command on a poisoned mutex — a poisoned lock
    // still carries the last value; fall back to "no update" otherwise.
    let available = app
        .state::<update::UpdateState>()
        .available
        .lock()
        .map(|g| g.clone())
        .unwrap_or(None);
    UpdateStatePayload {
        available,
        policy: update::policy_view(&app),
    }
}

/// Execute the install action selected by the runtime update policy. The
/// provider layer owns Tauri/Store/external routing; this IPC surface cannot
/// accidentally bypass a package manager.
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<(), String> {
    update::install(&app)
        .await
        .map_err(|error| format!("{error:#}"))
}

/// "Skip vX" — suppress the update indicator for the current version.
#[tauri::command]
pub fn update_skip(app: AppHandle) {
    update::skip_current(&app);
}

/// "Later" — hide the update indicator for 24h.
#[tauri::command]
pub fn update_snooze(app: AppHandle) {
    update::snooze(&app);
}

/// Re-emit the current (gated) update indicator. The panel calls this on
/// mount so the header badge reflects skip / snooze / disabled state — the
/// same gating as the tray dot — rather than the raw availability snapshot.
#[tauri::command]
pub fn update_refresh_indicator(app: AppHandle) {
    update::refresh_indicator(&app);
}

#[tauri::command]
pub async fn discover_repos(app: AppHandle) -> Result<usize, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<usize, String> {
        let mut found: Vec<cache::Repo> = Vec::new();
        let roots = discovery::default_roots();

        let cutoff = unix_now() - TIMELINE_WINDOW_DAYS * 86_400;

        for root in &roots {
            let root_str = root.to_string_lossy().into_owned();
            discovery::scan_path(root, |path| {
                let Some(mut path_str) = discovery_orchestrator::exact_worktree_identity(&path)
                else {
                    return;
                };
                if found.iter().any(|repo| {
                    discovery_orchestrator::repo_identity_match_key(&repo.path)
                        == discovery_orchestrator::repo_identity_match_key(&path_str)
                }) {
                    return;
                }
                let name = Path::new(&path_str)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                // Git walking is read-only and potentially much more
                // expensive than the final state edge, so keep it outside the
                // lifecycle lock. The exact exclusion gate is repeated below
                // immediately before any cache/watch/event mutation.
                let mut commits =
                    git::recent_commits(Path::new(&path_str), MAX_COMMITS_PER_REPO, cutoff)
                        .unwrap_or_default()
                        .into_iter()
                        .map(|mut c| {
                            c.repo_name = name.clone();
                            c
                        })
                        .collect::<Vec<_>>();
                // The legacy manual scan shares the same final lifecycle gate
                // as the orchestrator. Resolve first, then serialize only the
                // DB/cache/watch/event edge so excluded rows cannot leak back.
                let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
                let Ok(mut conn) = cache::open(&app) else {
                    return;
                };
                if discovery_orchestrator::repo_is_excluded(&conn, &path_str).unwrap_or(true) {
                    return;
                }
                if let Ok(Some(stored)) =
                    discovery_orchestrator::registered_repo_identity(&conn, &path_str)
                {
                    path_str = stored;
                }
                let repo = cache::Repo {
                    id: 0,
                    path: path_str.clone(),
                    name: name.clone(),
                    status: "active".to_string(),
                };
                if cache::upsert_repos(&mut conn, std::slice::from_ref(&repo)).is_err() {
                    return;
                }
                // A legacy extended-UNC row may retain its stored spelling to
                // preserve settings keys. Cache rows still use that exact
                // owner identity so repo_id resolution never falls back to 0.
                for commit in &mut commits {
                    commit.repo_path = path_str.clone();
                }

                let outcome = if commits.is_empty() {
                    None
                } else {
                    cache::upsert_commits(&mut conn, &commits).ok()
                };

                // Attach the file watcher to this newly-discovered repo.
                if let Some(w) = app.try_state::<watcher::RepoWatcher>() {
                    let _ = w.add(Path::new(&path_str));
                }

                found.push(repo);
                let _ = app.emit(
                    "discovery://progress",
                    ScanProgress {
                        root: root_str.clone(),
                        found: found.len(),
                    },
                );
                // Lightweight windowed-pull signal — the frontend re-pulls
                // the affected windows from the cache.
                if let Some(o) = outcome {
                    let _ = app.emit(
                        "timeline://invalidated",
                        cache::TimelineInvalidated {
                            generation: o.generation,
                            inserted: o.inserted,
                            deleted: 0,
                            repo_path: path_str,
                        },
                    );
                }
            });
        }

        let count = found.len();
        let _ = app.emit("discovery://complete", ScanComplete { count });
        Ok(count)
    })
    .await
    .map_err(|e| e.to_string())?
}
