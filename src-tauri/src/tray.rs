use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle,
};
use tauri_plugin_opener::OpenerExt;

use crate::{settings, window};

pub fn setup(app: &App) -> tauri::Result<()> {
    let reset = MenuItem::with_id(
        app,
        "reset_position",
        "Reset panel position",
        true,
        None::<&str>,
    )?;
    let open_settings = MenuItem::with_id(
        app,
        "open_settings",
        "Open settings file…",
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, "quit", "Quit gitwink", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[&reset, &open_settings, &separator, &quit],
    )?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("default window icon".into()))?;

    TrayIconBuilder::with_id("main")
        .icon(icon)
        .tooltip("gitwink")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "quit" => app.exit(0),
            "reset_position" => settings::clear_panel_position(app),
            "open_settings" => open_settings_file(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                window::toggle_panel(tray.app_handle());
            }
        })
        .build(app)?;

    Ok(())
}

/// Reveal settings.json in the user's default editor (or the OS file
/// handler for `.json`). We `ensure_path` first so the file always exists
/// when the editor opens — otherwise the user would land on a blank
/// "file not found" dialog the first time they try this.
fn open_settings_file(app: &AppHandle) {
    let path = match settings::ensure_path(app) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("gitwink: failed to ensure settings file: {e:#}");
            return;
        }
    };
    let path_str = path.to_string_lossy().into_owned();
    if let Err(e) = app.opener().open_path(&path_str, None::<&str>) {
        eprintln!("gitwink: failed to open settings file {path_str:?}: {e}");
    }
}
