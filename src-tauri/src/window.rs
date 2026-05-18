use tauri::{AppHandle, Manager, PhysicalPosition, WebviewWindow};

use crate::settings;

const PANEL_LABEL: &str = "panel";

pub fn toggle_panel(app: &AppHandle) {
    let Some(window) = app.get_webview_window(PANEL_LABEL) else {
        return;
    };

    let visible = window.is_visible().unwrap_or(false);
    if visible {
        let _ = window.hide();
    } else {
        position_panel(&window);
        let _ = window.show();
        let _ = window.set_focus();
    }
}

pub fn hide_panel(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(PANEL_LABEL) {
        let _ = window.hide();
    }
}

fn position_panel(window: &WebviewWindow) {
    let app = window.app_handle();
    let saved = settings::load(app).panel_position;
    let win_size = window.outer_size().unwrap_or_default();
    let win_w = win_size.width as i32;
    let win_h = win_size.height as i32;

    if let Some(pos) = saved {
        if point_visible_on_any_monitor(window, pos.x, pos.y, win_w, win_h) {
            let _ = window.set_position(PhysicalPosition::new(pos.x, pos.y));
            return;
        }
        // Saved position is off-screen (monitor unplugged, resolution change).
        // Fall through to cursor anchor.
    }

    position_near_cursor(window, win_w, win_h);
}

fn point_visible_on_any_monitor(
    window: &WebviewWindow,
    x: i32,
    y: i32,
    win_w: i32,
    win_h: i32,
) -> bool {
    let Ok(monitors) = window.available_monitors() else {
        return false;
    };
    // Require at least ~80px of the panel header to be on some monitor so the
    // user can grab it back if they nudged it almost off-screen.
    const VISIBLE_PAD: i32 = 80;
    monitors.iter().any(|m| {
        let mp = m.position();
        let ms = m.size();
        let mx2 = mp.x + ms.width as i32;
        let my2 = mp.y + ms.height as i32;
        let panel_x2 = x + win_w;
        let panel_y2 = y + win_h;
        let overlap_x = panel_x2.min(mx2) - x.max(mp.x);
        let overlap_y = panel_y2.min(my2) - y.max(mp.y);
        overlap_x >= VISIBLE_PAD && overlap_y >= VISIBLE_PAD
    })
}

fn position_near_cursor(window: &WebviewWindow, win_w: i32, win_h: i32) {
    let app = window.app_handle();
    let Ok(cursor) = app.cursor_position() else {
        return;
    };
    let cursor_x = cursor.x as i32;
    let cursor_y = cursor.y as i32;

    let monitor = window
        .available_monitors()
        .ok()
        .and_then(|monitors| {
            monitors.into_iter().find(|m| {
                let pos = m.position();
                let size = m.size();
                cursor_x >= pos.x
                    && cursor_x < pos.x + size.width as i32
                    && cursor_y >= pos.y
                    && cursor_y < pos.y + size.height as i32
            })
        })
        .or_else(|| window.primary_monitor().ok().flatten());

    let (mon_x, mon_y, mon_w, mon_h) = match monitor {
        Some(m) => {
            let p = m.position();
            let s = m.size();
            (p.x, p.y, s.width as i32, s.height as i32)
        }
        None => (0, 0, 1920, 1080),
    };

    // Anchor panel above-and-centered on the cursor for Windows trays (bottom),
    // below-and-centered for macOS menu bar (top).
    #[cfg(target_os = "macos")]
    let mut y = cursor_y + 8;
    #[cfg(not(target_os = "macos"))]
    let mut y = cursor_y - win_h - 8;

    let mut x = cursor_x - win_w / 2;

    let min_x = mon_x;
    let min_y = mon_y;
    let max_x = mon_x + mon_w - win_w;
    let max_y = mon_y + mon_h - win_h;

    if x < min_x {
        x = min_x;
    }
    if x > max_x {
        x = max_x;
    }
    if y < min_y {
        y = min_y;
    }
    if y > max_y {
        y = max_y;
    }

    let _ = window.set_position(PhysicalPosition::new(x, y));
}
