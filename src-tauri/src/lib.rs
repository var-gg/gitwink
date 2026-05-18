use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{Manager, WindowEvent};

mod cache;
mod commands;
mod discovery;
mod git;
mod settings;
mod tray;
mod window;

const BLUR_DEBOUNCE_MS: u64 = 80;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|app| {
            #[cfg(target_os = "macos")]
            {
                use tauri::ActivationPolicy;
                app.set_activation_policy(ActivationPolicy::Accessory);
            }

            tray::setup(app)?;

            // Best-effort LRU GC of the diff cache on startup. Off the main
            // thread; ignore errors (cache may not exist yet on first run).
            let gc_handle = app.handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                if let Ok(mut conn) = cache::open(&gc_handle) {
                    let _ = cache::gc_diffs(&mut conn, cache::DIFF_CACHE_MAX_BYTES);
                }
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
                            if gen_clone.load(Ordering::SeqCst) == stamp {
                                window::hide_panel(&handle_clone);
                            }
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
            commands::get_pinned_repos,
            commands::set_pinned_repos,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
