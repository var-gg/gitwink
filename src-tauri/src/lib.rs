mod cache;
mod commands;
mod discovery;
mod git;
mod tray;
mod window;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .setup(|_app| {
            #[cfg(target_os = "macos")]
            {
                use tauri::ActivationPolicy;
                _app.set_activation_policy(ActivationPolicy::Accessory);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::ping])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
