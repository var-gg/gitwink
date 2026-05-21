use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::{cache, discovery, discovery_orchestrator, git, settings, update, watcher};

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
        cache::list_commits_around_anchor(&conn, &filters, &anchor, before, after)
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

/// Phase 3 windowed-pull API: distinct authors under `filters`, for the
/// AuthorsChip facet list. The windowed timeline no longer holds a full
/// client-side commit array to tally, so the author list is a backend
/// facet query.
#[tauri::command]
pub async fn list_timeline_authors(
    app: AppHandle,
    filters: cache::TimelineFilters,
) -> Result<Vec<cache::AuthorTally>, String> {
    tauri::async_runtime::spawn_blocking(
        move || -> Result<Vec<cache::AuthorTally>, String> {
            let conn = cache::open(&app).map_err(|e| e.to_string())?;
            cache::list_timeline_authors(&conn, &filters).map_err(|e| e.to_string())
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

#[tauri::command]
pub async fn changed_files(
    repo_path: String,
    hash: String,
) -> Result<Vec<git::ChangedFile>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git::changed_files(Path::new(&repo_path), &hash).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
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

#[tauri::command]
pub async fn file_diff(
    app: AppHandle,
    repo_path: String,
    hash: String,
    file_path: String,
) -> Result<String, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<String, String> {
        // 1. Cache hit?
        if let Ok(conn) = cache::open(&app) {
            if let Ok(Some(text)) = cache::get_diff(&conn, &repo_path, &hash, &file_path) {
                return Ok(text);
            }
        }
        // 2. Compute, persist, return.
        let text = git::file_diff(Path::new(&repo_path), &hash, &file_path)
            .map_err(|e| e.to_string())?;
        if let Ok(conn) = cache::open(&app) {
            let _ = cache::put_diff(&conn, &repo_path, &hash, &file_path, &text);
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
        // Give the panel its always-on-top back so a later summon isn't
        // left non-floating if a diff window was open at dismiss time.
        let _ = panel.set_always_on_top(true);
    }
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
        repo_path: repo_path.clone(),
        repo_name,
        hash: hash.clone(),
        short_hash,
        summary,
        file_path: file_path.clone(),
    };
    eprintln!("open_diff invoked: repo={repo_path} hash={hash} file={file_path}");

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

    if let Some(diff) = app.get_webview_window("diff") {
        eprintln!("open_diff: existing window, show + focus + emit");
        diff.show().map_err(|e| {
            eprintln!("open_diff: show failed: {e}");
            e.to_string()
        })?;
        diff.set_focus().map_err(|e| {
            eprintln!("open_diff: focus failed: {e}");
            e.to_string()
        })?;
        diff.unminimize().ok();
        app.emit_to("diff", "diff://open", payload)
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    eprintln!("open_diff: building new diff window");
    let saved = settings::load(&app).diff_window;
    let (init_w, init_h) = match saved {
        Some(s) if monitor_can_contain(&app, s.x, s.y, s.w, s.h) => {
            (s.w as f64, s.h as f64)
        }
        _ => default_diff_size(&app),
    };

    let mut builder = tauri::WebviewWindowBuilder::new(
        &app,
        "diff",
        tauri::WebviewUrl::App("index.html".into()),
    )
    .title("gitwink diff")
    .inner_size(init_w, init_h)
    .resizable(true)
    .decorations(true)
    .skip_taskbar(false)
    .always_on_top(false)
    .visible(true);

    if let Some(s) = saved {
        if monitor_can_contain(&app, s.x, s.y, s.w, s.h) {
            builder = builder.position(s.x as f64, s.y as f64);
        }
    }

    let window = match builder.build() {
        Ok(w) => {
            eprintln!("open_diff: window built ok");
            w
        }
        Err(e) => {
            eprintln!("open_diff: window build FAILED: {e:#}");
            return Err(e.to_string());
        }
    };

    if saved.map(|s| s.maximized).unwrap_or(false) {
        let _ = window.maximize();
    }

    Ok(())
}

/// Pick a sensible default diff-window size based on the user's primary
/// monitor — ~70% of its dimensions, clamped to [800x600 .. 1400x900] so
/// it doesn't sprawl across multiple monitors on the first open.
fn default_diff_size(app: &AppHandle) -> (f64, f64) {
    const MIN_W: f64 = 800.0;
    const MIN_H: f64 = 600.0;
    const MAX_W: f64 = 1400.0;
    const MAX_H: f64 = 900.0;
    if let Ok(Some(monitor)) = app.primary_monitor() {
        let size = monitor.size();
        let scale = monitor.scale_factor();
        let logical_w = size.width as f64 / scale;
        let logical_h = size.height as f64 / scale;
        let w = (logical_w * 0.70).clamp(MIN_W, MAX_W);
        let h = (logical_h * 0.70).clamp(MIN_H, MAX_H);
        return (w, h);
    }
    (1100.0, 750.0)
}

/// Sanity-check a saved (x, y, w, h) against the current monitor layout —
/// when a monitor is unplugged the saved coords can be in no-mans-land and
/// the window would open invisible.
fn monitor_can_contain(app: &AppHandle, x: i32, y: i32, w: u32, h: u32) -> bool {
    let Ok(monitors) = app.available_monitors() else {
        return false;
    };
    const VISIBLE_PAD: i32 = 80;
    let panel_x2 = x + w as i32;
    let panel_y2 = y + h as i32;
    monitors.iter().any(|m| {
        let mp = m.position();
        let ms = m.size();
        let mx2 = mp.x + ms.width as i32;
        let my2 = mp.y + ms.height as i32;
        let overlap_x = panel_x2.min(mx2) - x.max(mp.x);
        let overlap_y = panel_y2.min(my2) - y.max(mp.y);
        overlap_x >= VISIBLE_PAD && overlap_y >= VISIBLE_PAD
    })
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
    let available = app
        .state::<update::UpdateState>()
        .available
        .lock()
        .unwrap()
        .clone();
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
                    let outcome = cache::open(&app)
                        .ok()
                        .and_then(|mut conn| cache::upsert_commits(&mut conn, &commits).ok());
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
