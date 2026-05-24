use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};

use crate::{commands, settings};

const PANEL_LABEL: &str = "panel";
const DIFF_LABEL: &str = "diff";
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
/// "settings" label to the Settings component. Returns `Err` if the
/// in-flight guard rejected the call (concurrent open) or the build
/// failed — the calling IPC command propagates this to the frontend
/// instead of silently looking successful (GPT Pro review B3).
pub fn open_settings(app: &AppHandle) -> Result<(), String> {
    // RAII release: clearing the flag in Drop guarantees we don't leak
    // a stuck "in flight" if any branch panics or returns early.
    struct ReleaseOnDrop;
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            OPEN_SETTINGS_IN_FLIGHT.store(false, Ordering::SeqCst);
        }
    }
    if OPEN_SETTINGS_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        // Diagnostic — kept always-on so the rare race surface stays
        // visible in production logs.
        eprintln!("gitwink: open_settings already in flight, ignoring duplicate call");
        return Err("settings open already in flight".into());
    }
    let _guard = ReleaseOnDrop;
    // Reuse the existing settings window if there is one — close-on-hide
    // keeps it alive across X-clicks so a re-open is instant and there
    // is no Tauri registry race. (The previous destroy+rebuild attempt
    // tripped on Tauri's async destroy: the registry entry survived
    // long enough for the next build() to fail with "label already
    // exists", forcing the user to click Settings twice.)
    if let Some(win) = app.get_webview_window(SETTINGS_LABEL) {
        let _ = win.unminimize();
        win.show().map_err(|e| e.to_string())?;
        win.set_focus().map_err(|e| e.to_string())?;
        return Ok(());
    }

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
    let win = built.map_err(|e| {
        eprintln!("gitwink: failed to build settings window: {e:#}");
        e.to_string()
    })?;
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
            api.prevent_close();
            if let Some(w) = handle.get_webview_window(SETTINGS_LABEL) {
                let _ = w.hide();
            }
            assert_panel_always_on_top(&handle);
            // The "settings is visible" veto in the blur handler
            // was holding the panel up while the user worked in
            // Settings; now that it's hiding, re-evaluate so a
            // glance panel actually dismisses if focus moved
            // somewhere outside the app.
            maybe_hide_panel_for_blur(&handle);
        }
        _ => {}
    });
    Ok(())
}

/// Serializes diff window creation so two near-simultaneous file opens
/// (rapid double-click on a file row, or the panel emitting open_diff
/// twice through some upstream race) can't both pass the
/// `get_webview_window("diff") == None` check and race into
/// WebviewWindowBuilder::build() for the same "diff" label — the same
/// failure class the settings window hit before
/// OPEN_SETTINGS_IN_FLIGHT was added.
static OPEN_DIFF_IN_FLIGHT: AtomicBool = AtomicBool::new(false);

/// Open (or focus) the diff window for `payload`. Mirrors open_settings:
/// single-flight guard, reuse pattern (existing window → show + focus +
/// emit), build-only-when-missing. Must be called on the UI thread —
/// the IPC command wrapper in commands.rs dispatches via
/// `app.run_on_main_thread` so the actual `build()` runs on the same
/// thread that owns the Tauri window registry. Returns Err with a
/// frontend-friendly string if either the in-flight guard rejects the
/// call or build itself fails.
pub fn open_or_focus_diff(
    app: &AppHandle,
    payload: commands::DiffOpenPayload,
) -> Result<(), String> {
    struct ReleaseOnDrop;
    impl Drop for ReleaseOnDrop {
        fn drop(&mut self) {
            OPEN_DIFF_IN_FLIGHT.store(false, Ordering::SeqCst);
        }
    }
    if OPEN_DIFF_IN_FLIGHT.swap(true, Ordering::SeqCst) {
        // Diagnostic — kept always-on so the rare race surface stays
        // visible in production logs.
        eprintln!("gitwink: open_or_focus_diff already in flight, ignoring duplicate call");
        return Err("diff window open already in flight".into());
    }
    let _guard = ReleaseOnDrop;

    if let Some(diff) = app.get_webview_window(DIFF_LABEL) {
        diff.unminimize().ok();
        diff.show().map_err(|e| e.to_string())?;
        diff.set_focus().map_err(|e| e.to_string())?;
        app.emit_to(DIFF_LABEL, "diff://open", payload)
            .map_err(|e| e.to_string())?;
        return Ok(());
    }

    let saved = settings::load(app).diff_window;
    // Always open at the modest default size. A remembered window size
    // restored across DPI scale factors was bloating the window (saved
    // in physical px, re-applied as logical); the user resizes from here.
    // Position + maximized state are still restored below.
    let (init_w, init_h) = default_diff_size(app);

    let mut builder = WebviewWindowBuilder::new(
        app,
        DIFF_LABEL,
        WebviewUrl::App("index.html".into()),
    )
    .title("gitwink diff")
    .inner_size(init_w, init_h)
    .resizable(true)
    .decorations(true)
    .skip_taskbar(false)
    .always_on_top(false)
    .visible(true);

    if let Some(s) = saved {
        if monitor_can_contain(app, s.x, s.y, s.w, s.h) {
            builder = builder.position(s.x as f64, s.y as f64);
        }
    }

    let window = builder.build().map_err(|e| {
        eprintln!("gitwink: failed to build diff window: {e:#}");
        e.to_string()
    })?;

    if saved.map(|s| s.maximized).unwrap_or(false) {
        let _ = window.maximize();
    }

    Ok(())
}

/// The diff window's default size — a modest fixed size the user
/// resizes from. Returned in logical pixels. Clamped to fit the primary
/// monitor so it never opens larger than the screen on a small display.
fn default_diff_size(app: &AppHandle) -> (f64, f64) {
    const WANT_W: f64 = 1024.0;
    const WANT_H: f64 = 720.0;
    if let Ok(Some(monitor)) = app.primary_monitor() {
        let scale = monitor.scale_factor();
        let logical_w = monitor.size().width as f64 / scale;
        let logical_h = monitor.size().height as f64 / scale;
        let w = WANT_W.min(logical_w - 80.0).max(640.0);
        let h = WANT_H.min(logical_h - 80.0).max(480.0);
        return (w, h);
    }
    (WANT_W, WANT_H)
}

/// Sanity-check a saved (x, y, w, h) against the current monitor layout —
/// when a monitor is unplugged the saved coords can be in no-mans-land and
/// the window would open invisible.
fn monitor_can_contain(app: &AppHandle, x: i32, y: i32, w: u32, h: u32) -> bool {
    let Ok(monitors) = app.available_monitors() else {
        return false;
    };
    const VISIBLE_PAD: i32 = 80;
    let panel_x2 = x + w as i32;
    let panel_y2 = y + h as i32;
    monitors.iter().any(|m| {
        let mp = m.position();
        let ms = m.size();
        let mx2 = mp.x + ms.width as i32;
        let my2 = mp.y + ms.height as i32;
        let overlap_x = panel_x2.min(mx2) - x.max(mp.x);
        let overlap_y = panel_y2.min(my2) - y.max(mp.y);
        overlap_x >= VISIBLE_PAD && overlap_y >= VISIBLE_PAD
    })
}

/// Minimum panel size the window can fall to on tiny / portrait
/// monitors where even PANEL_BASE doesn't fit. The chip dropdowns
/// degrade gracefully under this; the panel header is still reachable
/// because we keep above zero.
const PANEL_MIN_W: f64 = 360.0;
const PANEL_MIN_H: f64 = 420.0;

/// Resize the panel window to PANEL_BASE × scale, clamped to the current
/// monitor minus a small pad so it never opens larger than the screen.
/// Called by set_ui_scale on every change and by lib.rs setup so a saved
/// scale's window size is applied before the first show.
///
/// On small / portrait / remote displays where even the BASE size won't
/// fit, we let the panel shrink to PANEL_MIN_* rather than clamping
/// back up to BASE (the old code did `.max(PANEL_BASE_*)` which
/// silently undid the monitor clamp and could clip the header off-screen
/// — GPT Pro review C1).
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
    // Floor against PANEL_MIN_* — but never above the monitor max, so
    // a 320px-wide remote display still gets a window that fits.
    let min_w = PANEL_MIN_W.min(max_w.max(1.0));
    let min_h = PANEL_MIN_H.min(max_h.max(1.0));
    let final_w = want_w.min(max_w).max(min_w);
    let final_h = want_h.min(max_h).max(min_h);
    let _ = panel.set_size(LogicalSize::new(final_w, final_h));
}

/// Re-evaluate whether the panel should auto-hide and do it if so. The
/// blur debounce, the Settings/Diff CloseRequested handlers, and any
/// future "may have removed the last reason to keep the panel up"
/// path all route through here so the dismiss rule lives in one place.
///
/// The rule: hide iff
///   - not sticky (add-repo / native picker), AND
///   - not pinned (pinned mode never auto-hides), AND
///   - no child window (settings or diff) is visible, AND
///   - the panel itself is no longer focused.
///
/// That last check is what the inline blur handler used to skip — and
/// why glance mode could leave the panel stuck visible after the user
/// closed a child window via X (focus went elsewhere, no fresh blur
/// event ever lands on the panel because the panel was already blurred
/// while the child was on top).
pub fn maybe_hide_panel_for_blur(app: &AppHandle) {
    use std::sync::atomic::Ordering;
    let sticky = app
        .try_state::<commands::PanelSticky>()
        .map(|s| s.0.load(Ordering::SeqCst))
        .unwrap_or(false);
    if sticky {
        return;
    }
    let pinned = app
        .try_state::<commands::PanelPinned>()
        .map(|s| s.0.load(Ordering::SeqCst))
        .unwrap_or(false);
    if pinned {
        return;
    }
    let child_visible = ["diff", "settings"].iter().any(|label| {
        app.get_webview_window(label)
            .and_then(|w| w.is_visible().ok())
            .unwrap_or(false)
    });
    if child_visible {
        return;
    }
    let panel_focused = app
        .get_webview_window(PANEL_LABEL)
        .and_then(|w| w.is_focused().ok())
        .unwrap_or(false);
    if !panel_focused {
        hide_panel(app);
    }
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
        return;
    };
    if let Err(e) = panel.set_always_on_top(!pinned) {
        eprintln!("gitwink: apply_panel_pinned set_always_on_top failed: {e}");
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
