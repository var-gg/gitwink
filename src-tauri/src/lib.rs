use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{Emitter, Listener, Manager, WindowEvent};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutState};

mod cache;
mod commands;
pub mod control;
mod discovery;
mod discovery_orchestrator;
mod discovery_scheduler;
mod discovery_sources;
mod fetch;
mod file_history_jobs;
mod git;
mod git_diff;
mod git_blobs;
#[cfg(test)]
mod diff_tests;
mod diff_commands;
mod history_decorations;
#[allow(dead_code)] // lazy search backfill integration follows the storage foundation
mod history_fts;
mod history_git;
mod history_indexer;
#[allow(dead_code)] // command search integration follows the storage foundation
mod history_query;
mod history_schema;
mod history_store;
mod locale;
mod panel_geometry;
mod panel_window_state;
mod repo_exclusions;
mod settings;
mod store_update;
mod tray;
mod update;
mod watcher;
mod window;

const BLUR_DEBOUNCE_MS: u64 = 80;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        // FIRST plugin: a second launch of the hidden tray app is the user
        // LOOKING for the window — summon the existing panel instead of
        // spawning a duplicate instance (second tray icon, silently-dead
        // global hotkey, doubled watchers/fetches, settings.json races).
        .plugin(tauri_plugin_single_instance::init(|app, args, cwd| {
            // A normal second launch still means "summon gitwink". A control
            // launch carries a typed show request; its resolver owns focus and
            // queues the exact target for the persistent panel webview.
            if !control::accept_app_argv(app, args, cwd) {
                window::show_panel(app);
            }
        }))
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
            app.manage(control::PendingControl::default());
            app.manage(commands::PanelSticky::default());
            app.manage(commands::PanelPinned::default());
            let saved_panel_maximized = settings::load(app.handle())
                .panel_maximized
                .unwrap_or(false);
            app.manage(Arc::new(panel_window_state::PanelWindowRuntime::new(
                saved_panel_maximized,
            )));
            // Seed the in-memory settings cache from disk BEFORE any
            // window mounts and calls get_settings — see D1 in the
            // GPT Pro review (newly-opened windows could otherwise read
            // stale disk while a debounced broadcast was in flight).
            app.manage(commands::LiveSettings::from_disk(app.handle()));
            app.manage(diff_commands::ChangedFilesCache::default());
            app.manage(fetch::FetchCoordinator::default());
            app.manage(discovery_scheduler::ScanState::default());
            app.manage(Arc::new(file_history_jobs::FileHistoryJobs::default()));

            // Full history is strictly demand-driven: the worker exists at
            // startup but performs no DB or Git work until an all-time search
            // or All time view requests a concrete repository scope.
            let history_app = app.handle().clone();
            let history_events = app.handle().clone();
            let history_indexer = Arc::new(history_indexer::HistoryIndexer::start(
                move || cache::open(&history_app),
                history_git::DEFAULT_HISTORY_BATCH_SIZE,
                move |repo_path, outcome| {
                    if let history_indexer::RunOutcome::Published {
                        generation,
                        deleted,
                        ..
                    } = outcome
                    {
                        let _ = history_events.emit(
                            "timeline://invalidated",
                            cache::TimelineInvalidated {
                                generation,
                                inserted: 0,
                                deleted,
                                repo_path: repo_path.to_string(),
                            },
                        );
                    }
                },
            ));
            app.manage(Arc::clone(&history_indexer));
            // Resume only durable 512-row maintenance left by a prior crash.
            // This reads a tiny job table, never commits/Git history, and each
            // queued turn remains lower priority than visible scan demand.
            let cleanup_app = app.handle().clone();
            std::thread::spawn(move || {
                let Ok(conn) = cache::open(&cleanup_app) else {
                    return;
                };
                let Ok(paths) = history_store::pending_cleanup_paths(&conn) else {
                    return;
                };
                for path in paths {
                    let _ = history_indexer.request_cleanup(path);
                }
            });

            // Spin up the .git watcher immediately, then attach cached active
            // repos off the setup thread. Resolving .git/common-dir paths can
            // block on an offline network drive; startup and first paint must
            // never wait for that I/O. Per-repo reconciliation fences each
            // attachment against an exclusion/restore racing this backfill.
            if let Ok(watcher_) = watcher::RepoWatcher::start(app.handle().clone()) {
                app.manage(watcher_);
                let watcher_app = app.handle().clone();
                tauri::async_runtime::spawn_blocking(move || {
                    let active_paths = cache::open(&watcher_app)
                        .and_then(|conn| cache::list_repos(&conn))
                        .map(|repos| {
                            repos
                                .into_iter()
                                .filter(|repo| repo.status == "active")
                                .map(|repo| repo.path)
                                .collect::<Vec<_>>()
                        });
                    match active_paths {
                        Ok(mut paths) => {
                            // Resolve a few paths concurrently: one dead SMB
                            // share must not postpone watcher coverage for
                            // every healthy local repo behind it. RepoWatcher
                            // serializes only the final map/notify mutation.
                            paths.sort_by_key(|path| {
                                path.starts_with(r"\\") || path.starts_with("//")
                            });
                            let worker_count = paths.len().min(4);
                            let queue = Arc::new(std::sync::Mutex::new(
                                paths.into_iter().collect::<std::collections::VecDeque<_>>(),
                            ));
                            let mut workers = Vec::with_capacity(worker_count);
                            for _ in 0..worker_count {
                                let worker_app = watcher_app.clone();
                                let queue = Arc::clone(&queue);
                                workers.push(std::thread::spawn(move || loop {
                                    let path = queue
                                        .lock()
                                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                                        .pop_front();
                                    let Some(path) = path else {
                                        break;
                                    };
                                    watcher::reconcile_repo_registration(&worker_app, &path);
                                }));
                            }
                            for worker in workers {
                                let _ = worker.join();
                            }
                        }
                        Err(error) => {
                            eprintln!("startup watcher backfill failed: {error:#}");
                        }
                    }
                });
            }

            tray::setup(app)?;

            // Restore normalized normal geometry and, independently, the
            // maximized/zoomed state before the first show.
            let saved_scale = settings::load(app.handle())
                .ui_scale
                .unwrap_or(1.0)
                .clamp(commands::UI_SCALE_MIN, commands::UI_SCALE_MAX);
            panel_window_state::restore_panel_window(app.handle(), saved_scale as f64);

            // Apply the saved panel pin state to the runtime atomic +
            // the panel's always_on_top flag before the first show — so
            // a pinned-at-quit panel comes back pinned. skipTaskbar is
            // intentionally not touched at any runtime point on Windows
            // (see PanelPinned doc); the panel always honours
            // tauri.conf.json's skipTaskbar=true.
            let saved_pinned = settings::load(app.handle()).panel_pinned.unwrap_or(false);
            if let Some(state) = app.try_state::<commands::PanelPinned>() {
                state
                    .0
                    .store(saved_pinned, std::sync::atomic::Ordering::SeqCst);
            }
            window::apply_panel_pinned(app.handle(), saved_pinned);

            // First-process control activation. Single-instance only invokes
            // its callback for later processes, so cold launches must feed
            // their own argv through the same ingress after panel geometry and
            // pin state are restored (avoids a base-size/always-on-top flash).
            let initial_args = std::env::args_os()
                .map(|argument| argument.to_string_lossy().into_owned())
                .collect::<Vec<_>>();
            let initial_cwd = std::env::current_dir()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            let _ = control::accept_app_argv(app.handle(), initial_args, initial_cwd);

            // Resolve update ownership once: standalone/Scoop use GitHub for
            // discovery, while packaged builds stay StoreContext-backed.
            // One check on startup, then every 24h; no policy auto-opens UI.
            app.manage(update::UpdateState::default());
            update::start(app.handle().clone());

            // Prewarm discovery in the background. Reads IDE recents + git
            // config + learned roots + (first-run only) the full fallback
            // walk. The managed scheduler also coalesces cooldown-bounded
            // panel-summon refreshes, so a long-lived tray process discovers
            // repositories created after startup without idle polling.
            let discovery_scheduler = discovery_scheduler::start(app.handle().clone());
            app.manage(discovery_scheduler);

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
                        // The "diff is visible" veto in the blur handler
                        // was holding the panel up while the user was
                        // reading the diff; re-evaluate now so a glance
                        // panel actually dismisses if focus moved
                        // somewhere outside the app.
                        window::maybe_hide_panel_for_blur(&handle);
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
                                    settings::save_diff_window_maximized(&h);
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
                panel_window_state::install_panel_window_events(app.handle(), &panel);
                let handle = app.handle().clone();
                let focus_generation = Arc::new(AtomicU64::new(0));

                panel.on_window_event(move |event| match event {
                    WindowEvent::Focused(false) => {
                        // Debounce hide: a momentary blur (e.g. OS-native drag,
                        // tray context menu) shouldn't dismiss the panel.
                        // All dismiss conditions (sticky, pinned, child
                        // visible, panel still focused) are in
                        // window::maybe_hide_panel_for_blur so the same rule
                        // can be re-applied from child CloseRequested paths.
                        let stamp = focus_generation
                            .fetch_add(1, Ordering::SeqCst)
                            .wrapping_add(1);
                        let gen_clone = Arc::clone(&focus_generation);
                        let handle_clone = handle.clone();
                        std::thread::spawn(move || {
                            std::thread::sleep(Duration::from_millis(BLUR_DEBOUNCE_MS));
                            if gen_clone.load(Ordering::SeqCst) != stamp {
                                return;
                            }
                            window::maybe_hide_panel_for_blur(&handle_clone);
                        });
                    }
                    WindowEvent::Focused(true) => {
                        // Cancel any pending hide.
                        focus_generation.fetch_add(1, Ordering::SeqCst);
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
            commands::refresh_recent_commits,
            commands::list_commits_window,
            commands::list_commits_around_anchor,
            commands::list_commits_at_rank,
            commands::count_commits,
            commands::get_timeline_generation,
            commands::request_history_index,
            commands::release_history_index,
            commands::get_history_index_status,
            commands::list_filter_facets,
            commands::list_branches,
            commands::count_branch_commits,
            commands::current_upstream_status,
            commands::explicit_add_repo,
            commands::list_excluded_repos,
            commands::exclude_repo,
            commands::restore_repo,
            commands::inspect_replacement_repo,
            commands::replace_excluded_repo,
            commands::repo_commits,
            diff_commands::changed_files,
            diff_commands::changed_files_batch,
            diff_commands::file_diff,
            diff_commands::commit_file_blobs,
            commands::file_history,
            commands::cancel_file_history,
            commands::open_file_history,
            commands::open_diff,
            commands::take_pending_diff_open,
            control::ingress::take_pending_control_deliveries,
            commands::dismiss_panel,
            commands::get_panel_window_state,
            commands::toggle_panel_maximized,
            commands::set_panel_size_preset,
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
            commands::update_refresh_indicator,
            commands::get_settings,
            commands::set_ui_scale,
            commands::set_diff_font,
            commands::set_panel_hotkey,
            commands::set_panel_pinned,
            commands::open_settings_window,
            commands::set_update_check,
            commands::set_locale,
            commands::set_auto_fetch_on_show,
            commands::ack_auto_fetch_notice,
            commands::maybe_fetch_repo,
            commands::repo_fetch_capability,
            commands::fetch_repo_now,
            commands::repo_refs_fingerprint,
            commands::open_settings_file,
            commands::set_live_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
