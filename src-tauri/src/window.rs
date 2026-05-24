use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

use crate::{commands, settings};

const PANEL_LABEL: &str = "panel";
/// Base panel size at scale 1.0 — mirrors tauri.conf.json. The UI scale
/// multiplies these for the actual window size.
const PANEL_BASE_W: f64 = 520.0;
const PANEL_BASE_H: f64 = 600.0;

pub fn toggle_panel(app: &AppHandle) {
    let Some(window) = app.get_webview_window(PANEL_LABEL) else {
        return;
    };

    let visible = window.is_visible().unwrap_or(false);
    if visible {
        let _ = window.hide();
    } else {
        position_panel(&window);
        // Re-assert always-on-top on every summon (open_diff drops it so
        // the diff window isn't trapped behind the panel, and it can
        // still be false from an earlier diff session). assert_* reads
        // the pinned state so glance summons re-float and pinned summons
        // stay un-topmost.
        assert_panel_always_on_top(app);
        let _ = window.show();
        let _ = window.set_focus();
        // The webview is only un-hidden on show, never re-created, so the
        // bootstrap commit fetch runs once per app launch. Nudge the
        // frontend to re-pull so a commit the file-watcher missed (or a
        // repo whose watcher never attached) still surfaces on summon.
        let _ = window.emit("panel://shown", ());
    }
}

pub fn hide_panel(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(PANEL_LABEL) {
        let _ = window.hide();
    }
}

/// Unconditionally show + focus the panel. Unlike `toggle_panel` this
/// never hides — used by the updater flow to surface the update modal
/// regardless of the panel's current visibility.
pub fn show_panel(app: &AppHandle) {
    let Some(window) = app.get_webview_window(PANEL_LABEL) else {
        return;
    };
    if !window.is_visible().unwrap_or(false) {
        position_panel(&window);
    }
    assert_panel_always_on_top(app);
    let _ = window.show();
    let _ = window.set_focus();
    let _ = window.emit("panel://shown", ());
}

const SETTINGS_LABEL: &str = "settings";

/// Serializes open_settings so two near-simultaneous triggers (tray
/// click + header right-click within a frame; double-fire MenuEvents)
/// can't both race into WebviewWindowBuilder::build() for the same
/// "settings" label — that crashed gitwink.exe on Windows (exit code
/// 0xcfffffff after two "building fresh settings window" eprintlns).
static OPEN_SETTINGS_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Open the settings window — or focus it if already open. Built lazily
/// off the shared index.html, like the diff window; main.tsx routes the
/// "settings" label to the Settings component. The panel is summoned
/// alongside so diff/timeline size + font changes preview live.
pub fn open_settings(app: &AppHandle) {
    // RAII release: clearing the flag in Drop guarantees we don't leak
    // a stuck "in flight" if any branch panics or returns early.
    struct ReleaseOnDrop;
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            OPEN_SETTINGS_IN_FLIGHT.store(false, Ordering::SeqCst);
        }
    }
    if OPEN_SETTINGS_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        eprintln!("gitwink: open_settings already in flight, ignoring duplicate call");
        let _ = std::io::Write::flush(&mut std::io::stderr());
        return;
    }
    let _guard = ReleaseOnDrop;
    // Reuse the existing settings window if there is one — close-on-hide
    // keeps it alive across X-clicks so a re-open is instant and there
    // is no Tauri registry race. (The previous destroy+rebuild attempt
    // tripped on Tauri's async destroy: the registry entry survived
    // long enough for the next build() to fail with "label already
    // exists", forcing the user to click Settings twice.)
    if let Some(win) = app.get_webview_window(SETTINGS_LABEL) {
        eprintln!("gitwink: re-using existing settings window");
        let _ = std::io::Write::flush(&mut std::io::stderr());
        let _ = win.unminimize();
        let _ = win.show();
        let _ = win.set_focus();
        return;
    }

    eprintln!("gitwink: building settings window");
    let _ = std::io::Write::flush(&mut std::io::stderr());
    let built = WebviewWindowBuilder::new(
        app,
        SETTINGS_LABEL,
        WebviewUrl::App("index.html".into()),
    )
    .title("gitwink settings")
    .inner_size(440.0, 560.0)
    .min_inner_size(360.0, 420.0)
    .resizable(true)
    .decorations(true)
    .skip_taskbar(false)
    .always_on_top(false)
    .visible(true)
    .build();
    eprintln!("gitwink: build() returned: {}", if built.is_ok() { "Ok" } else { "Err" });
    let _ = std::io::Write::flush(&mut std::io::stderr());
    match built {
        Ok(win) => {
            eprintln!("gitwink: settings window built ok, attaching close handler");
            let _ = std::io::Write::flush(&mut std::io::stderr());
            // Hide instead of destroy on close — same pattern as the diff
            // window. Two reasons: re-open is instant (no rebuild / no
            // re-mount), and the get_webview_window check at the top of
            // open_settings can never race with a slow first-time build.
            // On close, restore the panel's always-on-top for the
            // current mode (glance → true; pinned → false) so the panel
            // doesn't stay un-topmost after the user dismisses settings.
            let handle = app.clone();
            win.on_window_event(move |evt| match evt {
                WindowEvent::CloseRequested { api, .. } => {
                    eprintln!("gitwink: settings CloseRequested → prevent + hide");
                    api.prevent_close();
                    if let Some(w) = handle.get_webview_window(SETTINGS_LABEL) {
                        let _ = w.hide();
                    }
                    assert_panel_always_on_top(&handle);
                }
                _ => {}
            });
        }
        Err(e) => {
            eprintln!("gitwink: failed to build settings window: {e:#}");
        }
    }
}

/// Resize the panel window to PANEL_BASE × scale, clamped to the current
/// monitor minus a small pad so it never opens larger than the screen.
/// Called by set_ui_scale on every change and by lib.rs setup so a saved
/// scale's window size is applied before the first show.
pub fn resize_panel_for_scale(app: &AppHandle, scale: f32) {
    let Some(panel) = app.get_webview_window(PANEL_LABEL) else {
        return;
    };
    let want_w = PANEL_BASE_W * scale as f64;
    let want_h = PANEL_BASE_H * scale as f64;
    let (max_w, max_h) = panel
        .current_monitor()
        .ok()
        .flatten()
        .map(|m| {
            let s = m.scale_factor();
            let size = m.size();
            (size.width as f64 / s - 80.0, size.height as f64 / s - 80.0)
        })
        .unwrap_or((f64::INFINITY, f64::INFINITY));
    let final_w = want_w.min(max_w).max(PANEL_BASE_W);
    let final_h = want_h.min(max_h).max(PANEL_BASE_H);
    let _ = panel.set_size(LogicalSize::new(final_w, final_h));
}

/// Read the runtime PanelPinned state and set the panel's always-on-top
/// flag accordingly: glance mode → true (floats above all), pinned mode
/// → false (normal stacking). Call wherever a code path previously did
/// `panel.set_always_on_top(true)` to "restore the panel default".
pub fn assert_panel_always_on_top(app: &AppHandle) {
    let pinned = app
        .try_state::<commands::PanelPinned>()
        .map(|s| s.0.load(std::sync::atomic::Ordering::SeqCst))
        .unwrap_or(false);
    if let Some(panel) = app.get_webview_window(PANEL_LABEL) {
        let _ = panel.set_always_on_top(!pinned);
    }
}

/// Apply the panel's pin flag — currently only always_on_top, because
/// any set_skip_taskbar call (startup OR runtime) on this panel was
/// implicated in a blank-WebView2-on-next-build cascade on Windows. The
/// taskbar-entry promise of pinned mode is therefore dropped for now:
/// the panel always honours tauri.conf.json's skipTaskbar=true. Pinned
/// mode now means "blur-dismiss off + not always-on-top" — the user
/// still summons via tray / hotkey rather than alt-tab.
pub fn apply_panel_pinned(app: &AppHandle, pinned: bool) {
    let Some(panel) = app.get_webview_window(PANEL_LABEL) else {
        eprintln!("gitwink: apply_panel_pinned but panel window missing");
        let _ = std::io::Write::flush(&mut std::io::stderr());
        return;
    };
    let at = panel.set_always_on_top(!pinned);
    eprintln!("gitwink: apply_panel_pinned(pinned={pinned}) → always_on_top={at:?}");
    let _ = std::io::Write::flush(&mut std::io::stderr());
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
