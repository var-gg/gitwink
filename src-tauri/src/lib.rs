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

            if let Some(panel) = app.get_webview_window("panel") {
                let handle = app.handle().clone();
                let focus_generation = Arc::new(AtomicU64::new(0));

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
                        settings::save_panel_position(&handle, pos.x, pos.y);
                    }
                    _ => {}
                });
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::ping])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
