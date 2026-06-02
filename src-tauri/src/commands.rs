use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::{cache, discovery, discovery_orchestrator, git, settings, update, watcher, window};

const MAX_COMMITS_PER_REPO: usize = 10;
const MAX_COMMITS_PER_REPO_NO_WINDOW: usize = 1_000;
const TIMELINE_WINDOW_DAYS: i64 = 7;
const TIMELINE_MAX_TOTAL: usize = 50;
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

#[tauri::command]
pub async fn recent_commits(
    app: AppHandle,
    window_days: Option<i64>,
) -> Result<Vec<git::CommitSummary>, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<git::CommitSummary>, String> {
        let conn = cache::open(&app).map_err(|e| e.to_string())?;
        let repos = cache::list_repos(&conn).map_err(|e| e.to_string())?;
        let cutoff = cutoff_for(window_days);
        let per_repo = per_repo_cap(window_days);
        let total = total_cap(window_days);

        let mut all: Vec<git::CommitSummary> = Vec::new();
        for repo in repos {
            let commits =
                git::recent_commits(Path::new(&repo.path), per_repo, cutoff).unwrap_or_default();
            for mut c in commits {
                c.repo_name = repo.name.clone();
                all.push(c);
            }
        }
        all.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
        all.truncate(total);

        let mut conn = cache::open(&app).map_err(|e| e.to_string())?;
        let _ = cache::upsert_commits(&mut conn, &all).map_err(|e| e.to_string())?;
        Ok(all)
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
        cache::list_commits_window(&conn, &filters, cursor.as_ref(), direction, limit)
            .map_err(|e| e.to_string())
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
        cache::list_commits_around_anchor(&conn, &filters, &anchor, before, after, None)
            .map_err(|e| e.to_string())
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
        cache::list_commits_at_rank(&conn, &filters, rank, before, after)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Phase 1 windowed-pull API: filtered total commit count for the scrollbar.
#[tauri::command]
pub async fn count_commits(
    app: AppHandle,
    filters: cache::TimelineFilters,
) -> Result<i64, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<i64, String> {
        let conn = cache::open(&app).map_err(|e| e.to_string())?;
        cache::count_commits(&conn, &filters).map_err(|e| e.to_string())
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
) -> Result<cache::FilterFacets, String> {
    tauri::async_runtime::spawn_blocking(
        move || -> Result<cache::FilterFacets, String> {
            let conn = cache::open(&app).map_err(|e| e.to_string())?;
            cache::list_filter_facets(&conn, &filters).map_err(|e| e.to_string())
        },
    )
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
        discovery_orchestrator::add_repo_explicit(&app2, &path)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Hide a repo from the panel and prevent auto-rediscovery. Tombstoned;
/// the user can bring it back with explicit_add_repo. Used by the
/// "hide" affordance on missing-status rows in the RepoChip.
#[tauri::command]
pub async fn hide_repo(
    app: AppHandle,
    canonical_path: String,
) -> Result<(), String> {
    let app2 = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        discovery_orchestrator::hide_repo(&app2, &canonical_path)
            .map_err(|e| e.to_string())
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
    /// key (`repo_path\0hash`) → (file list, last-access tick)
    entries: std::collections::HashMap<String, (Vec<git::ChangedFile>, u64)>,
    tick: u64,
}

/// Max cached commits before LRU eviction kicks in.
const CHANGED_FILES_CACHE_CAP: usize = 256;
/// Skip caching a commit whose changed-file list exceeds this — one huge
/// entry would dominate the cache; recomputing it on demand is fine.
const CHANGED_FILES_CACHE_MAX_FILES: usize = 1_000;
/// Upper bound on commits one `changed_files_batch` call will process.
const CHANGED_FILES_PREFETCH_CAP: usize = 100;

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

    fn get(&self, repo_path: &str, hash: &str) -> Option<Vec<git::ChangedFile>> {
        let mut lru = self.0.lock().ok()?;
        lru.tick += 1;
        let tick = lru.tick;
        let entry = lru.entries.get_mut(&Self::key(repo_path, hash))?;
        entry.1 = tick;
        Some(entry.0.clone())
    }

    fn contains(&self, repo_path: &str, hash: &str) -> bool {
        self.0
            .lock()
            .map(|lru| lru.entries.contains_key(&Self::key(repo_path, hash)))
            .unwrap_or(false)
    }

    fn put(&self, repo_path: &str, hash: &str, files: &[git::ChangedFile]) {
        if files.len() > CHANGED_FILES_CACHE_MAX_FILES {
            return;
        }
        let Ok(mut lru) = self.0.lock() else {
            return;
        };
        lru.tick += 1;
        let tick = lru.tick;
        lru.entries
            .insert(Self::key(repo_path, hash), (files.to_vec(), tick));
        // Evict least-recently-used entries down to the cap.
        while lru.entries.len() > CHANGED_FILES_CACHE_CAP {
            let victim = lru
                .entries
                .iter()
                .min_by_key(|(_, (_, t))| *t)
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

#[tauri::command]
pub async fn changed_files(
    app: AppHandle,
    repo_path: String,
    hash: String,
) -> Result<Vec<git::ChangedFile>, String> {
    tauri::async_runtime::spawn_blocking(
        move || -> Result<Vec<git::ChangedFile>, String> {
            if let Some(cache) = app.try_state::<ChangedFilesCache>() {
                if let Some(hit) = cache.get(&repo_path, &hash) {
                    return Ok(hit);
                }
            }
            let files = git::changed_files(Path::new(&repo_path), &hash)
                .map_err(|e| e.to_string())?;
            if let Some(cache) = app.try_state::<ChangedFilesCache>() {
                cache.put(&repo_path, &hash, &files);
            }
            Ok(files)
        },
    )
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
pub async fn changed_files_batch(
    app: AppHandle,
    commits: Vec<CommitRef>,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(cache) = app.try_state::<ChangedFilesCache>() else {
            return;
        };
        for c in commits.into_iter().take(CHANGED_FILES_PREFETCH_CAP) {
            if cache.contains(&c.repo_path, &c.hash) {
                continue;
            }
            if let Ok(files) = git::changed_files(Path::new(&c.repo_path), &c.hash) {
                cache.put(&c.repo_path, &c.hash, &files);
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
pub fn take_pending_diff_open(
    state: tauri::State<'_, PendingDiff>,
) -> Option<DiffOpenPayload> {
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
    state
        .0
        .store(sticky, std::sync::atomic::Ordering::SeqCst);
}

/// Whether a discovery run is currently in flight. The frontend pulls
/// this on startup so the "Scanning…" indicator is correct even when
/// the orchestrator finished before the scan-progress listener was
/// registered.
#[tauri::command]
pub fn get_scan_state(
    state: tauri::State<'_, discovery_orchestrator::ScanState>,
) -> bool {
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
    app.run_on_main_thread(move || {
        let _ = tx.try_send(window::open_or_focus_diff(&app2, payload));
    })
    .map_err(|e| e.to_string())?;
    rx.recv()
        .await
        .ok_or_else(|| "diff dispatch dropped".to_string())?
}

#[tauri::command]
pub async fn repo_commits(
    app: AppHandle,
    repo_path: String,
    branches: Option<Vec<String>>,
    window_days: Option<i64>,
) -> Result<Vec<git::CommitSummary>, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<Vec<git::CommitSummary>, String> {
        let cutoff = cutoff_for(window_days);
        let cap = single_repo_cap(window_days);
        let branches_slice: Option<&[String]> = branches.as_deref();
        let commits = git::repo_commits(Path::new(&repo_path), branches_slice, cap, cutoff)
            .map_err(|e| e.to_string())?;
        // Persist into the same commits cache so warm starts paint instantly
        // for this repo too.
        if !commits.is_empty() {
            if let Ok(mut conn) = cache::open(&app) {
                let _ = cache::upsert_commits(&mut conn, &commits);
            }
        }
        Ok(commits)
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
    /// When true, the panel is "pinned" — no blur auto-hide, shows in
    /// the taskbar, not always-on-top. False = tray-glance default.
    pub panel_pinned: bool,
    /// Self-update behaviour. Serialized as "enabled" / "manual" /
    /// "disabled" via the enum's lowercase rename. Meaningful only when
    /// `updater_available` is true.
    pub update_check: settings::UpdateCheckMode,
    /// False for Scoop and Microsoft Store (MSIX) installs — those
    /// channels manage their own updates, so the Updates section in
    /// the Settings window is hidden and the `update_check` knob is
    /// inert. The frontend uses this flag to gate the UI.
    pub updater_available: bool,
}

#[tauri::command]
pub fn get_settings(
    live: tauri::State<'_, LiveSettings>,
) -> AppSettings {
    // Read the in-memory snapshot, NOT disk — the Settings window's
    // broadcast hot path already updated LiveSettings synchronously,
    // so a newly-opened window's initSettings sees the fresh value
    // even when the debounced disk write hasn't landed yet.
    let s = live.snapshot();
    AppSettings {
        ui_scale: s.ui_scale.unwrap_or(1.0).clamp(UI_SCALE_MIN, UI_SCALE_MAX),
        diff_font_family: s.diff_font_family,
        panel_hotkey: s
            .panel_hotkey
            .filter(|h| !h.trim().is_empty())
            .unwrap_or_else(|| settings::DEFAULT_PANEL_HOTKEY.to_string()),
        panel_pinned: s.panel_pinned.unwrap_or(false),
        update_check: s.update_check,
        updater_available: !update::installed_via_scoop() && !update::installed_via_msix(),
    }
}

/// Project an incoming AppSettings (the user-facing slice the frontend
/// passes via broadcastSettings) into the live Settings cache. Skips
/// `updater_available` (computed) and re-clamps `ui_scale` defensively
/// so an out-of-range value from a stale frontend can't poison the
/// cache. Hotkey persistence still goes through `set_panel_hotkey`
/// because re-binding the global shortcut has to succeed before we
/// commit; the broadcast just keeps the displayed text aligned.
#[tauri::command]
pub fn set_live_settings(
    next: AppSettings,
    live: tauri::State<'_, LiveSettings>,
) {
    live.with_mut(|s| {
        s.ui_scale = Some(next.ui_scale.clamp(UI_SCALE_MIN, UI_SCALE_MAX));
        s.diff_font_family = next.diff_font_family;
        s.panel_hotkey = Some(next.panel_hotkey);
        s.panel_pinned = Some(next.panel_pinned);
        s.update_check = next.update_check;
    });
}

/// Persist the UI scale and resize the panel window proportionally.
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
    window::resize_panel_for_scale(&app, clamped);
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
    // Runtime toggle: ONLY flip always_on_top (safe Win32 toggle). We
    // deliberately do NOT touch set_skip_taskbar here — that mutates
    // WS_EX_TOOLWINDOW / WS_EX_APPWINDOW at runtime, which on Windows
    // is a known WebView2 destabiliser (subsequent window builds can
    // come up blank). The blur-dismiss behaviour change is live
    // regardless — it's just an atomic check.
    if let Some(panel) = app.get_webview_window("panel") {
        if let Err(e) = panel.set_always_on_top(!pinned) {
            eprintln!("gitwink: set_panel_pinned set_always_on_top failed: {e}");
        }
    }
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
    /// True for Scoop installs — the modal shows a `scoop update` hint
    /// instead of an in-app "Update now" button.
    pub scoop: bool,
}

/// Snapshot the updater state for the modal: the pending update (if any)
/// plus whether this is a Scoop install.
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
        scoop: update::installed_via_scoop(),
    }
}

/// Download + install the pending update, then relaunch. The NSIS
/// installer runs in `passive` mode (progress UI, no prompts). Refuses
/// to run for Scoop installs.
#[tauri::command]
pub async fn update_install(app: AppHandle) -> Result<(), String> {
    if update::installed_via_scoop() {
        return Err("Installed via Scoop — run `scoop update gitwink` instead.".into());
    }
    if update::installed_via_msix() {
        return Err("Installed from the Microsoft Store — the Store manages updates.".into());
    }
    let updater = app.updater().map_err(|e| e.to_string())?;
    let pending = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "No update available.".to_string())?;
    pending
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| e.to_string())?;
    app.restart();
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
                let name = path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let path_str = path.to_string_lossy().into_owned();
                let repo = cache::Repo {
                    id: 0,
                    path: path_str.clone(),
                    name: name.clone(),
                    status: "active".to_string(),
                };
                found.push(repo.clone());
                let _ = app.emit(
                    "discovery://progress",
                    ScanProgress {
                        root: root_str.clone(),
                        found: found.len(),
                    },
                );

                // Fill the timeline incrementally: read this repo's recent
                // commits right now and stream them to the panel so rows
                // appear as repos are found. Errors are silently skipped.
                let commits = git::recent_commits(&path, MAX_COMMITS_PER_REPO, cutoff)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|mut c| {
                        c.repo_name = name.clone();
                        c
                    })
                    .collect::<Vec<_>>();

                if !commits.is_empty() {
                    let outcome = cache::open(&app).ok().and_then(|mut conn| {
                        // Upsert the repo row FIRST so upsert_commits can
                        // resolve a real repo_id — its COALESCE falls back
                        // to 0 when the repos row does not exist yet.
                        cache::upsert_repos(&mut conn, std::slice::from_ref(&repo)).ok()?;
                        cache::upsert_commits(&mut conn, &commits).ok()
                    });
                    // Lightweight windowed-pull signal — the frontend
                    // re-pulls the affected windows from the cache.
                    if let Some(o) = outcome {
                        let _ = app.emit(
                            "timeline://invalidated",
                            cache::TimelineInvalidated {
                                generation: o.generation,
                                inserted: o.inserted,
                                repo_path: path_str.clone(),
                            },
                        );
                    }
                }

                // Attach the file watcher to this newly-discovered repo.
                if let Some(w) = app.try_state::<watcher::RepoWatcher>() {
                    w.add(&path);
                }
            });
        }

        let mut conn = cache::open(&app).map_err(|e| e.to_string())?;
        cache::upsert_repos(&mut conn, &found).map_err(|e| e.to_string())?;

        let count = found.len();
        let _ = app.emit("discovery://complete", ScanComplete { count });
        Ok(count)
    })
    .await
    .map_err(|e| e.to_string())?
}
