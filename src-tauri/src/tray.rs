use tauri::{
    image::Image,
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    App, AppHandle, Wry,
};

use crate::locale::{self, AppLocale};
use crate::settings::{self, Settings, UpdateCheckMode};
use crate::{update, window};

const TRAY_ID: &str = "main";

/// Tray tooltip base. Debug builds (`tauri dev` or any from-source build)
/// carry a "(dev)" tag so a review build is distinguishable from an
/// installed release in the tray — both are otherwise just "gitwink".
fn tray_tooltip_base() -> &'static str {
    if cfg!(debug_assertions) {
        "gitwink (dev)"
    } else {
        "gitwink"
    }
}

pub fn setup(app: &App) -> tauri::Result<()> {
    let settings = settings::load(app.handle());
    let app_locale = locale::resolve_system(settings.locale);
    let menu = build_menu(app.handle(), None, &settings, app_locale)?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or_else(|| tauri::Error::AssetNotFound("default window icon".into()))?;

    TrayIconBuilder::with_id(TRAY_ID)
        .icon(icon)
        .tooltip(tray_tooltip_base())
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
///
/// The "Open settings file…" entry used to live here next to "Settings…"
/// but was demoted to a footer link inside the Settings window itself —
/// two "Settings*" items in a small tray menu were redundant. The same
/// affordance is still reachable: tray → Settings… → "Open settings.json".
fn build_menu(
    app: &AppHandle,
    update_version: Option<&str>,
    settings: &Settings,
    app_locale: AppLocale,
) -> tauri::Result<Menu<Wry>> {
    let strings = locale::native_catalog(app_locale);
    let reset = MenuItem::with_id(
        app,
        "reset_position",
        strings.reset_panel_position,
        true,
        None::<&str>,
    )?;
    let settings_item = MenuItem::with_id(app, "settings", strings.settings, true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", strings.quit, true, None::<&str>)?;
    let sep = PredefinedMenuItem::separator(app)?;

    // Every channel can check from the tray: Direct installs update in place,
    // Scoop points to its package-manager command, and Store builds use the
    // native StoreContext APIs. Only the explicit Off setting hides it.
    let hide_updater = settings.update_check == UpdateCheckMode::Disabled;
    if hide_updater {
        return Menu::with_items(app, &[&settings_item, &reset, &sep, &quit]);
    }

    let check = MenuItem::with_id(
        app,
        "check_updates",
        strings.check_for_updates,
        true,
        None::<&str>,
    )?;
    match update_version {
        Some(v) => {
            let update_item = MenuItem::with_id(
                app,
                "update_available",
                format!("{}: v{v}", strings.update_available_prefix),
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
                    &settings_item,
                    &reset,
                    &sep,
                    &quit,
                ],
            )
        }
        None => Menu::with_items(app, &[&check, &settings_item, &reset, &sep, &quit]),
    }
}

fn handle_menu_event(app: &AppHandle, event: MenuEvent) {
    match event.id().as_ref() {
        "quit" => app.exit(0),
        "reset_position" => settings::clear_panel_position(app),
        "settings" => {
            if let Err(e) = window::open_settings(app) {
                eprintln!("gitwink: tray Settings handler: {e}");
            }
        }
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
        let settings = settings::load(&app);
        let app_locale = locale::resolve_system(settings.locale);
        let strings = locale::native_catalog(app_locale);
        if let Ok(menu) = build_menu(&app, version.as_deref(), &settings, app_locale) {
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
        let tip = if version.is_some() {
            format!(
                "{} — {}",
                tray_tooltip_base(),
                strings.update_available_tooltip
            )
        } else {
            tray_tooltip_base().to_string()
        };
        let _ = tray.set_tooltip(Some(tip));
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
