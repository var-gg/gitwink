use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{Listener, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

mod cache;
mod commands;
mod discovery;
mod discovery_orchestrator;
mod discovery_sources;
mod git;
mod settings;
mod tray;
mod update;
mod watcher;
mod window;

const BLUR_DEBOUNCE_MS: u64 = 80;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    // Toggle on key-down only; ignore Released so a single
                    // press doesn't fire twice.
                    if event.state() == ShortcutState::Pressed {
                        window::toggle_panel(app);
                    }
                })
                .build(),
        )
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                use tauri::ActivationPolicy;
                app.set_activation_policy(ActivationPolicy::Accessory);
            }

            app.manage(commands::PendingDiff::default());
            app.manage(commands::PanelSticky::default());
            app.manage(commands::PanelPinned::default());
            app.manage(commands::ChangedFilesCache::default());
            app.manage(discovery_orchestrator::ScanState::default());

            // Spin up the .git watcher and attach it to every repo the
            // cache already knows about. discover_repos adds new ones
            // as they're found.
            if let Ok(watcher_) = watcher::RepoWatcher::start(app.handle().clone()) {
                if let Ok(conn) = cache::open(app.handle()) {
                    if let Ok(repos) = cache::list_repos(&conn) {
                        for r in &repos {
                            watcher_.add(std::path::Path::new(&r.path));
                        }
                    }
                }
                app.manage(watcher_);
            }

            tray::setup(app)?;

            // Apply the saved UI scale to the panel window size before
            // the first show — so a saved scale survives across launches
            // without a 520×600 flash of the base panel.
            let saved_scale = settings::load(app.handle())
                .ui_scale
                .unwrap_or(1.0)
                .clamp(commands::UI_SCALE_MIN, commands::UI_SCALE_MAX);
            window::resize_panel_for_scale(app.handle(), saved_scale);

            // Apply the saved panel pin state to the runtime atomic + the
            // panel's skip_taskbar / always_on_top flags before the first
            // show — so a pinned-at-quit panel comes back pinned.
            let saved_pinned = settings::load(app.handle())
                .panel_pinned
                .unwrap_or(false);
            if let Some(state) = app.try_state::<commands::PanelPinned>() {
                state
                    .0
                    .store(saved_pinned, std::sync::atomic::Ordering::SeqCst);
            }
            window::apply_panel_pinned(app.handle(), saved_pinned);

            // Self-update: managed state + background check loop (one
            // check on startup, then every 24h). update::start is a
            // no-op for Scoop and Microsoft Store (MSIX) installs — those
            // channels manage their own updates.
            app.manage(update::UpdateState::default());
            update::start(app.handle().clone());

            // Prewarm discovery in the background. Reads cache + IDE
            // recents + git config + (first-run only) full fs walk and
            // fills cache so the next panel-open paints cache hits
            // immediately. No UI events in this commit; commit 4 wires
            // those up. Returns a handle holding a CancellationToken
            // so we *could* cancel on app exit; the task is also
            // self-bounded by per-tier deadlines + caps.
            let _orchestrator = discovery_orchestrator::start(app.handle().clone());
            app.manage(_orchestrator);

            // Global hotkey to summon the panel from anywhere, even when
            // tray-hidden. Default CmdOrCtrl+Shift+G ("G" for gitwink),
            // overridable via `panel_hotkey` in settings.json. Best-effort:
            // if the user-supplied spec doesn't parse, or another running
            // app already holds the binding (Windows registers globally,
            // first-bind wins), we log and start up anyway — the tray
            // icon stays as the fallback entry point.
            let hotkey_spec = settings::load(app.handle())
                .panel_hotkey
                .filter(|s| !s.trim().is_empty())
                .unwrap_or_else(|| settings::DEFAULT_PANEL_HOTKEY.to_string());
            match hotkey_spec.parse::<Shortcut>() {
                Ok(panel_shortcut) => {
                    if let Err(e) = app.global_shortcut().register(panel_shortcut) {
                        eprintln!(
                            "gitwink: failed to register global hotkey {hotkey_spec:?} ({e}); \
                             the tray icon still works as a fallback"
                        );
                    }
                }
                Err(e) => {
                    eprintln!(
                        "gitwink: invalid global hotkey spec {hotkey_spec:?} ({e}); \
                         using fallback {default:?}",
                        default = settings::DEFAULT_PANEL_HOTKEY
                    );
                    if let Ok(fallback) = settings::DEFAULT_PANEL_HOTKEY.parse::<Shortcut>() {
                        let _ = app.global_shortcut().register(fallback);
                    }
                }
            }

            // Best-effort LRU GC of the diff cache on startup. Off the main
            // thread; ignore errors (cache may not exist yet on first run).
            let gc_handle = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Ok(mut conn) = cache::open(&gc_handle) {
                    let _ = cache::gc_diffs(&mut conn, cache::DIFF_CACHE_MAX_BYTES);
                }
            });

            // Wire the diff window's persist + Esc-hide behaviour the moment
            // it shows up (it's lazily built by open_diff).
            let diff_handle = app.handle().clone();
            let app_for_listener = app.handle().clone();
            app_for_listener.listen("tauri://window-created", move |event| {
                let payload = event.payload();
                if !payload.contains("\"label\":\"diff\"") {
                    return;
                }
                let Some(diff) = diff_handle.get_webview_window("diff") else {
                    return;
                };
                let move_gen = Arc::new(AtomicU64::new(0));
                let resize_gen = Arc::new(AtomicU64::new(0));
                let handle = diff_handle.clone();
                diff.on_window_event(move |evt| match evt {
                    WindowEvent::CloseRequested { api, .. } => {
                        api.prevent_close();
                        if let Some(w) = handle.get_webview_window("diff") {
                            let _ = w.hide();
                        }
                        // Diff window gone — restore the panel's
                        // always-on-top for the current mode (glance →
                        // true; pinned → stays false).
                        window::assert_panel_always_on_top(&handle);
                    }
                    WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
                        let stamp = if matches!(evt, WindowEvent::Moved(_)) {
                            move_gen.fetch_add(1, Ordering::SeqCst).wrapping_add(1)
                        } else {
                            resize_gen.fetch_add(1, Ordering::SeqCst).wrapping_add(1)
                        };
                        let move_clone = Arc::clone(&move_gen);
                        let resize_clone = Arc::clone(&resize_gen);
                        let h = handle.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(200));
                            if move_clone.load(Ordering::SeqCst) != stamp
                                && resize_clone.load(Ordering::SeqCst) != stamp
                            {
                                return;
                            }
                            if let Some(w) = h.get_webview_window("diff") {
                                let maximized = w.is_maximized().unwrap_or(false);
                                // While maximized, the OS-reported outer_position/size
                                // is the maximized geometry — useless for restoring
                                // a "windowed" size next time. Only persist geometry
                                // when not maximized; persist the flag either way.
                                if maximized {
                                    let mut s = settings::load(&h);
                                    let prev = s.diff_window.unwrap_or(settings::DiffWindowState {
                                        x: 200,
                                        y: 100,
                                        w: 1100,
                                        h: 750,
                                        maximized: false,
                                    });
                                    s.diff_window = Some(settings::DiffWindowState {
                                        maximized: true,
                                        ..prev
                                    });
                                    let _ = settings::save_replace(&h, &s);
                                } else if let (Ok(pos), Ok(size)) =
                                    (w.outer_position(), w.outer_size())
                                {
                                    settings::save_diff_window(
                                        &h,
                                        settings::DiffWindowState {
                                            x: pos.x,
                                            y: pos.y,
                                            w: size.width,
                                            h: size.height,
                                            maximized: false,
                                        },
                                    );
                                }
                            }
                        });
                    }
                    _ => {}
                });
            });

            if let Some(panel) = app.get_webview_window("panel") {
                let handle = app.handle().clone();
                let focus_generation = Arc::new(AtomicU64::new(0));
                let move_generation = Arc::new(AtomicU64::new(0));

                panel.on_window_event(move |event| match event {
                    WindowEvent::Focused(false) => {
                        // Debounce hide: a momentary blur (e.g. OS-native drag,
                        // tray context menu) shouldn't dismiss the panel.
                        let stamp =
                            focus_generation.fetch_add(1, Ordering::SeqCst).wrapping_add(1);
                        let gen_clone = Arc::clone(&focus_generation);
                        let handle_clone = handle.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(BLUR_DEBOUNCE_MS));
                            if gen_clone.load(Ordering::SeqCst) != stamp {
                                return;
                            }
                            // Sticky: the panel resists blur-dismiss while the
                            // empty-state add-repo screen is up or a native
                            // folder picker is open. Focus left the panel, but
                            // the user is mid add-repo flow, not dismissing.
                            let sticky = handle_clone
                                .try_state::<commands::PanelSticky>()
                                .map(|s| s.0.load(Ordering::SeqCst))
                                .unwrap_or(false);
                            if sticky {
                                return;
                            }
                            // Don't dismiss the panel just because the user
                            // clicked into our own diff window — that's still
                            // an in-app interaction.
                            // Pinned mode: the panel stays put regardless
                            // of blur — that's the whole point of pinning.
                            let pinned = handle_clone
                                .try_state::<commands::PanelPinned>()
                                .map(|s| s.0.load(Ordering::SeqCst))
                                .unwrap_or(false);
                            if pinned {
                                return;
                            }
                            let diff_visible = handle_clone
                                .get_webview_window("diff")
                                .and_then(|w| w.is_visible().ok())
                                .unwrap_or(false);
                            if diff_visible {
                                return;
                            }
                            // Same for the settings window — interacting
                            // with it must not dismiss the panel behind it.
                            let settings_visible = handle_clone
                                .get_webview_window("settings")
                                .and_then(|w| w.is_visible().ok())
                                .unwrap_or(false);
                            if settings_visible {
                                return;
                            }
                            window::hide_panel(&handle_clone);
                        });
                    }
                    WindowEvent::Focused(true) => {
                        // Cancel any pending hide.
                        focus_generation.fetch_add(1, Ordering::SeqCst);
                    }
                    WindowEvent::Moved(pos) => {
                        // Debounce: during a drag this fires ~60 Hz. Only
                        // persist the *settled* position to avoid
                        // hammering settings.json.
                        let stamp = move_generation
                            .fetch_add(1, Ordering::SeqCst)
                            .wrapping_add(1);
                        let gen_clone = Arc::clone(&move_generation);
                        let handle_clone = handle.clone();
                        let x = pos.x;
                        let y = pos.y;
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(200));
                            if gen_clone.load(Ordering::SeqCst) == stamp {
                                settings::save_panel_position(&handle_clone, x, y);
                            }
                        });
                    }
                    _ => {}
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::ping,
            commands::list_repos,
            commands::discover_repos,
            commands::list_recent_commits_cached,
            commands::recent_commits,
            commands::list_commits_window,
            commands::list_commits_around_anchor,
            commands::list_commits_at_rank,
            commands::count_commits,
            commands::get_timeline_generation,
            commands::list_filter_facets,
            commands::list_branches,
            commands::current_upstream_status,
            commands::explicit_add_repo,
            commands::hide_repo,
            commands::repo_commits,
            commands::changed_files,
            commands::changed_files_batch,
            commands::file_diff,
            commands::commit_file_blobs,
            commands::open_diff,
            commands::take_pending_diff_open,
            commands::dismiss_panel,
            commands::set_panel_sticky,
            commands::get_scan_state,
            commands::get_pinned_repos,
            commands::set_pinned_repos,
            commands::get_branch_selection,
            commands::set_branch_selection,
            commands::update_get_state,
            commands::update_install,
            commands::update_skip,
            commands::update_snooze,
            commands::get_settings,
            commands::set_ui_scale,
            commands::set_diff_font,
            commands::set_panel_hotkey,
            commands::set_panel_pinned,
            commands::open_settings_window,
            commands::set_update_check,
            commands::open_settings_file,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
