use tauri::{
    image::Image,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Wry,
};
use tauri_plugin_opener::OpenerExt;

use crate::{settings, update, window};

const TRAY_ID: &str = "main";

pub fn setup(app: &App) -> tauri::Result<()> {
    let menu = build_menu(app.handle(), None)?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("default window icon".into()))?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip("gitwink")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(handle_menu_event)
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

/// Build the tray context menu. When `update_version` is `Some`, an
/// "Update available" item is prepended above a separator. The menu is
/// rebuilt wholesale (rather than toggling item visibility) whenever the
/// update state changes — see `set_update_indicator`.
fn build_menu(app: &AppHandle, update_version: Option<&str>) -> tauri::Result<Menu<Wry>> {
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
    let sep = PredefinedMenuItem::separator(app)?;

    // Microsoft Store installs update via the Store itself — gitwink shows
    // no in-app updater UI. Scoop and direct installs keep the
    // "Check for updates" item.
    if update::installed_via_msix() {
        return Menu::with_items(app, &[&reset, &open_settings, &sep, &quit]);
    }

    let check =
        MenuItem::with_id(app, "check_updates", "Check for updates", true, None::<&str>)?;
    match update_version {
        Some(v) => {
            let update_item = MenuItem::with_id(
                app,
                "update_available",
                format!("Update available: v{v}"),
                true,
                None::<&str>,
            )?;
            let sep_top = PredefinedMenuItem::separator(app)?;
            Menu::with_items(
                app,
                &[
                    &update_item,
                    &sep_top,
                    &check,
                    &reset,
                    &open_settings,
                    &sep,
                    &quit,
                ],
            )
        }
        None => Menu::with_items(app, &[&check, &reset, &open_settings, &sep, &quit]),
    }
}

fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        "quit" => app.exit(0),
        "reset_position" => settings::clear_panel_position(app),
        "open_settings" => open_settings_file(app),
        "check_updates" => update::manual_check(app),
        "update_available" => update::open_modal(app),
        _ => {}
    }
}

/// Rebuild the tray menu + swap the icon to reflect update availability.
/// `version = Some` ⇒ dot overlay + "Update available" item; `None` ⇒
/// plain icon, no item. Tray mutation is marshalled onto the main thread.
pub fn set_update_indicator(app: &AppHandle, version: Option<String>) {
    let app = app.clone();
    let handle = app.clone();
    let _ = handle.run_on_main_thread(move || {
        let Some(tray) = app.tray_by_id(TRAY_ID) else {
            return;
        };
        if let Ok(menu) = build_menu(&app, version.as_deref()) {
            let _ = tray.set_menu(Some(menu));
        }
        if let Some(base) = app.default_window_icon().cloned() {
            let icon = if version.is_some() {
                with_dot(&base)
            } else {
                base
            };
            let _ = tray.set_icon(Some(icon));
        }
        let _ = tray.set_tooltip(Some(if version.is_some() {
            "gitwink — update available"
        } else {
            "gitwink"
        }));
    });
}

/// Composite a small accent dot onto the top-right of the tray icon —
/// done in-memory from the base RGBA so no second icon asset is needed.
fn with_dot(base: &Image<'_>) -> Image<'static> {
    let w = base.width();
    let h = base.height();
    let mut rgba = base.rgba().to_vec();

    let side = w.min(h) as i32;
    let r = ((side as f32) * 0.30).round() as i32;
    let cx = w as i32 - r - 1;
    let cy = r + 1;
    let outer = (r + 1) * (r + 1);
    let inner = r * r;

    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let d2 = (x - cx).pow(2) + (y - cy).pow(2);
            if d2 > outer {
                continue;
            }
            let idx = ((y as u32 * w + x as u32) * 4) as usize;
            if idx + 3 >= rgba.len() {
                continue;
            }
            if d2 <= inner {
                // Accent fill (orange-red).
                rgba[idx] = 0xF0;
                rgba[idx + 1] = 0x52;
                rgba[idx + 2] = 0x3F;
                rgba[idx + 3] = 0xFF;
            } else {
                // 1px darker ring for contrast on light system trays.
                rgba[idx] = 0x7A;
                rgba[idx + 1] = 0x1A;
                rgba[idx + 2] = 0x12;
                rgba[idx + 3] = 0xFF;
            }
        }
    }
    Image::new_owned(rgba, w, h)
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
