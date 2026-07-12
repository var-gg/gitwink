use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, LogicalSize, Manager, PhysicalSize, WebviewWindow, WindowEvent};

use crate::panel_geometry::{self, LogicalWorkArea};
use crate::{settings, window};

const PANEL_LABEL: &str = "panel";
const WINDOW_EVENT_SETTLE: Duration = Duration::from_millis(220);
const PROGRAMMATIC_RESIZE_GUARD: Duration = Duration::from_millis(650);

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PanelWindowStatePayload {
    pub maximized: bool,
    pub chrome_double_click_maximize: bool,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PanelSizePreset {
    Default,
    Wide,
}

#[derive(Debug, Clone, Copy)]
struct NormalWindowSnapshot {
    position: settings::PanelPosition,
    inner_size: PhysicalSize<u32>,
    os_scale: f64,
}

/// Runtime-only coordination state. Durable geometry remains in settings;
/// this state identifies programmatic changes and preserves the last normal
/// rect across native maximize events that arrive before the disk debounce.
pub struct PanelWindowRuntime {
    last_maximized: AtomicBool,
    preserve_preference_until: Mutex<Option<Instant>>,
    last_normal_snapshot: Mutex<Option<NormalWindowSnapshot>>,
}

impl PanelWindowRuntime {
    pub fn new(maximized: bool) -> Self {
        Self {
            last_maximized: AtomicBool::new(maximized),
            preserve_preference_until: Mutex::new(None),
            last_normal_snapshot: Mutex::new(None),
        }
    }

    fn mark_programmatic_resize(&self) {
        if let Ok(mut guard) = self.preserve_preference_until.lock() {
            *guard = Some(Instant::now() + PROGRAMMATIC_RESIZE_GUARD);
        }
    }

    fn must_preserve_preference(&self) -> bool {
        let Ok(mut guard) = self.preserve_preference_until.lock() else {
            return true;
        };
        match *guard {
            Some(until) if Instant::now() < until => true,
            Some(_) => {
                *guard = None;
                false
            }
            None => false,
        }
    }

    fn remember_normal_snapshot(&self, snapshot: NormalWindowSnapshot) {
        if let Ok(mut guard) = self.last_normal_snapshot.lock() {
            *guard = Some(snapshot);
        }
    }

    fn normal_snapshot(&self) -> Option<NormalWindowSnapshot> {
        self.last_normal_snapshot
            .lock()
            .ok()
            .and_then(|guard| *guard)
    }
}

#[derive(Debug, Default, Clone, Copy)]
struct SettledWindowEvents {
    moved: bool,
    resized: bool,
    scale_changed: bool,
}

impl SettledWindowEvents {
    fn merge(&mut self, next: Self) {
        self.moved |= next.moved;
        self.resized |= next.resized;
        self.scale_changed |= next.scale_changed;
    }
}

fn runtime(app: &AppHandle) -> Option<Arc<PanelWindowRuntime>> {
    app.try_state::<Arc<PanelWindowRuntime>>()
        .map(|state| Arc::clone(state.inner()))
}

fn current_ui_scale(app: &AppHandle) -> f64 {
    settings::load(app)
        .ui_scale
        .unwrap_or(1.0)
        .clamp(crate::commands::UI_SCALE_MIN, crate::commands::UI_SCALE_MAX) as f64
}

fn work_area_logical(panel: &WebviewWindow) -> LogicalWorkArea {
    let monitor = panel
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| panel.primary_monitor().ok().flatten());
    let Some(monitor) = monitor else {
        return LogicalWorkArea {
            width: 10_000.0,
            height: 10_000.0,
        };
    };
    let scale = monitor.scale_factor().max(f64::EPSILON);
    LogicalWorkArea {
        width: monitor.work_area().size.width as f64 / scale,
        height: monitor.work_area().size.height as f64 / scale,
    }
}

fn normal_snapshot(panel: &WebviewWindow) -> Result<NormalWindowSnapshot, String> {
    let position = panel.outer_position().map_err(|error| error.to_string())?;
    let inner_size = panel.inner_size().map_err(|error| error.to_string())?;
    let os_scale = panel.scale_factor().map_err(|error| error.to_string())?;
    Ok(NormalWindowSnapshot {
        position: settings::PanelPosition {
            x: position.x,
            y: position.y,
        },
        inner_size,
        os_scale,
    })
}

fn preference_from_snapshot(
    snapshot: NormalWindowSnapshot,
    ui_scale: f64,
) -> settings::PanelSizePreference {
    panel_geometry::preference_from_physical_size(
        snapshot.inner_size.width,
        snapshot.inner_size.height,
        snapshot.os_scale,
        ui_scale,
    )
}

fn emit_window_state(app: &AppHandle, maximized: bool) {
    let _ = app.emit_to(PANEL_LABEL, "panel://window-state", payload(maximized));
}

fn payload(maximized: bool) -> PanelWindowStatePayload {
    PanelWindowStatePayload {
        maximized,
        chrome_double_click_maximize: cfg!(target_os = "windows"),
    }
}

pub fn get_panel_window_state(app: &AppHandle) -> PanelWindowStatePayload {
    let maximized = app
        .get_webview_window(PANEL_LABEL)
        .and_then(|panel| panel.is_maximized().ok())
        .unwrap_or_else(|| settings::load(app).panel_maximized.unwrap_or(false));
    payload(maximized)
}

/// Install one long-lived debounce worker instead of spawning a sleeping
/// thread for every ~60 Hz move/resize event. The worker coalesces both axes
/// into one settings snapshot after the window manager settles.
pub fn install_panel_window_events(app: &AppHandle, panel: &WebviewWindow) {
    let Some(runtime) = runtime(app) else {
        eprintln!("gitwink: panel window runtime missing during event setup");
        return;
    };
    if !panel.is_maximized().unwrap_or(false) {
        if let Ok(snapshot) = normal_snapshot(panel) {
            runtime.remember_normal_snapshot(snapshot);
        }
    }

    let (sender, receiver) = mpsc::channel::<SettledWindowEvents>();
    let worker_app = app.clone();
    let worker_runtime = Arc::clone(&runtime);
    if let Err(error) = std::thread::Builder::new()
        .name("gitwink-panel-window-state".into())
        .spawn(move || {
            while let Ok(mut pending) = receiver.recv() {
                loop {
                    match receiver.recv_timeout(WINDOW_EVENT_SETTLE) {
                        Ok(next) => pending.merge(next),
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                }
                process_settled_events(&worker_app, &worker_runtime, pending);
            }
        })
    {
        eprintln!("gitwink: failed to start panel window-state worker: {error}");
        return;
    }

    let observed_panel = panel.clone();
    panel.on_window_event(move |event| {
        let pending = match event {
            WindowEvent::Moved(_) => SettledWindowEvents {
                moved: true,
                ..Default::default()
            },
            WindowEvent::Resized(_) => SettledWindowEvents {
                resized: true,
                ..Default::default()
            },
            WindowEvent::ScaleFactorChanged { .. } => SettledWindowEvents {
                moved: true,
                resized: true,
                scale_changed: true,
            },
            _ => return,
        };

        // Capture normal geometry in memory before a native Win+Up/zoom event
        // can replace it with the maximized rect. No disk I/O happens here.
        if !observed_panel.is_maximized().unwrap_or(false) {
            if let Ok(snapshot) = normal_snapshot(&observed_panel) {
                runtime.remember_normal_snapshot(snapshot);
            }
        }
        let _ = sender.send(pending);
    });
}

fn process_settled_events(
    app: &AppHandle,
    runtime: &PanelWindowRuntime,
    pending: SettledWindowEvents,
) {
    let Some(panel) = app.get_webview_window(PANEL_LABEL) else {
        return;
    };
    let maximized = panel
        .is_maximized()
        .unwrap_or_else(|_| runtime.last_maximized.load(Ordering::SeqCst));
    let previous = runtime.last_maximized.swap(maximized, Ordering::SeqCst);
    let changed = previous != maximized;

    if changed {
        if maximized {
            if let Some(snapshot) = runtime.normal_snapshot() {
                let preference = preference_from_snapshot(snapshot, current_ui_scale(app));
                if let Err(error) =
                    settings::save_panel_before_maximize(app, snapshot.position, preference)
                {
                    eprintln!("settings: failed to persist native panel maximize: {error:#}");
                }
            } else if let Err(error) = settings::save_panel_maximized(app, true) {
                eprintln!("settings: failed to persist native panel maximize: {error:#}");
            }
        } else if let Err(error) = settings::save_panel_maximized(app, false) {
            eprintln!("settings: failed to persist native panel restore: {error:#}");
        }
        window::assert_panel_always_on_top(app);
        emit_window_state(app, maximized);
    }

    if maximized {
        return;
    }

    // Native restore and mixed-DPI transitions must re-derive the actual size
    // from the durable normalized preference. Their OS-reported rect is never
    // allowed to overwrite that preference.
    if (changed && previous) || pending.scale_changed {
        schedule_restore_normal_geometry(app.clone(), true);
        return;
    }

    let preserve_preference = runtime.must_preserve_preference();
    if pending.resized && !preserve_preference {
        if let Ok(snapshot) = normal_snapshot(&panel) {
            let preference = preference_from_snapshot(snapshot, current_ui_scale(app));
            settings::save_panel_size_preference(app, preference);
            runtime.remember_normal_snapshot(snapshot);
        }
    }

    if pending.moved {
        if let Ok(position) = panel.outer_position() {
            settings::save_panel_position(app, position.x, position.y);
        }
        // Moving between same-DPI monitors emits no scale event. Reconcile
        // anyway so a preference clamped on a small display expands again on
        // a larger one, and vice versa.
        if !pending.resized {
            let _ = apply_preferred_normal_geometry(app, current_ui_scale(app), true);
        }
    }
}

fn apply_minimum_size(panel: &WebviewWindow, ui_scale: f64) -> Result<(), String> {
    let minimum = panel_geometry::minimum_logical_size(ui_scale, work_area_logical(panel));
    panel
        .set_min_size(Some(LogicalSize::new(minimum.width, minimum.height)))
        .map_err(|error| error.to_string())
}

fn apply_preferred_normal_geometry(
    app: &AppHandle,
    ui_scale: f64,
    reposition_after: bool,
) -> Result<(), String> {
    let panel = app
        .get_webview_window(PANEL_LABEL)
        .ok_or_else(|| "panel window missing".to_string())?;
    apply_minimum_size(&panel, ui_scale)?;
    if panel.is_maximized().unwrap_or(false) {
        return Ok(());
    }
    let saved = settings::load(app);
    let preference = panel_geometry::sanitize_preference(saved.panel_size_at_100_percent);
    let target =
        panel_geometry::target_logical_size(preference, ui_scale, work_area_logical(&panel));
    if let Some(runtime) = runtime(app) {
        runtime.mark_programmatic_resize();
    }
    panel
        .set_size(LogicalSize::new(target.width, target.height))
        .map_err(|error| error.to_string())?;

    if reposition_after {
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            window::reposition_panel(&app);
        });
    }
    Ok(())
}

fn schedule_restore_normal_geometry(app: AppHandle, reposition_after: bool) {
    std::thread::spawn(move || {
        // Unmaximize is asynchronous on both AppKit and Win32. Wait until the
        // window manager reports the normal state, with a bounded fallback.
        for _ in 0..8 {
            std::thread::sleep(Duration::from_millis(40));
            let restored = app
                .get_webview_window(PANEL_LABEL)
                .and_then(|panel| panel.is_maximized().ok())
                .map(|maximized| !maximized)
                .unwrap_or(false);
            if restored {
                let _ =
                    apply_preferred_normal_geometry(&app, current_ui_scale(&app), reposition_after);
                window::assert_panel_always_on_top(&app);
                return;
            }
        }
        eprintln!("gitwink: panel restore did not settle before geometry timeout");
    });
}

pub fn restore_panel_window(app: &AppHandle, ui_scale: f64) {
    let saved = settings::load(app);
    let should_maximize = saved.panel_maximized.unwrap_or(false);
    // Establish the normal rect before asking the window manager to maximize;
    // it remains the durable restore target across this and future launches.
    if let Err(error) = apply_preferred_normal_geometry(app, ui_scale, false) {
        eprintln!("gitwink: failed to restore panel normal geometry: {error}");
    }
    let Some(panel) = app.get_webview_window(PANEL_LABEL) else {
        return;
    };
    if let Some(runtime) = runtime(app) {
        runtime
            .last_maximized
            .store(should_maximize, Ordering::SeqCst);
        if let Ok(snapshot) = normal_snapshot(&panel) {
            runtime.remember_normal_snapshot(snapshot);
        }
    }
    if should_maximize {
        let _ = panel.set_always_on_top(false);
        if let Err(error) = panel.maximize() {
            eprintln!("gitwink: failed to restore maximized panel: {error}");
            let _ = settings::save_panel_maximized(app, false);
            if let Some(runtime) = runtime(app) {
                runtime.last_maximized.store(false, Ordering::SeqCst);
            }
        }
    }
}

pub fn apply_ui_scale(app: &AppHandle, ui_scale: f64) {
    let Some(panel) = app.get_webview_window(PANEL_LABEL) else {
        return;
    };
    if let Err(error) = apply_minimum_size(&panel, ui_scale) {
        eprintln!("gitwink: failed to update panel minimum size: {error}");
    }
    if !panel.is_maximized().unwrap_or(false) {
        if let Err(error) = apply_preferred_normal_geometry(app, ui_scale, true) {
            eprintln!("gitwink: failed to apply panel UI scale: {error}");
        }
    }
}

pub fn toggle_panel_maximized(app: &AppHandle) -> Result<PanelWindowStatePayload, String> {
    let panel = app
        .get_webview_window(PANEL_LABEL)
        .ok_or_else(|| "panel window missing".to_string())?;
    let maximized = panel.is_maximized().map_err(|error| error.to_string())?;
    let runtime = runtime(app);

    if maximized {
        settings::save_panel_maximized(app, false).map_err(|error| error.to_string())?;
        if let Err(error) = panel.unmaximize() {
            let _ = settings::save_panel_maximized(app, true);
            return Err(error.to_string());
        }
        if let Some(runtime) = &runtime {
            runtime.last_maximized.store(false, Ordering::SeqCst);
        }
        window::assert_panel_always_on_top(app);
        emit_window_state(app, false);
        schedule_restore_normal_geometry(app.clone(), true);
        Ok(payload(false))
    } else {
        let snapshot = normal_snapshot(&panel)?;
        let preference = preference_from_snapshot(snapshot, current_ui_scale(app));
        settings::save_panel_before_maximize(app, snapshot.position, preference)
            .map_err(|error| error.to_string())?;
        if let Some(runtime) = &runtime {
            runtime.remember_normal_snapshot(snapshot);
        }
        let _ = panel.set_always_on_top(false);
        if let Err(error) = panel.maximize() {
            let _ = settings::save_panel_maximized(app, false);
            window::assert_panel_always_on_top(app);
            return Err(error.to_string());
        }
        if let Some(runtime) = &runtime {
            runtime.last_maximized.store(true, Ordering::SeqCst);
        }
        emit_window_state(app, true);
        Ok(payload(true))
    }
}

pub fn apply_panel_size_preset(
    app: &AppHandle,
    preset: PanelSizePreset,
) -> Result<PanelWindowStatePayload, String> {
    let (preference, reset_position) = match preset {
        PanelSizePreset::Default => (panel_geometry::default_preference(), true),
        PanelSizePreset::Wide => (panel_geometry::wide_preference(), false),
    };
    settings::save_panel_layout_preset(app, preference, reset_position)
        .map_err(|error| error.to_string())?;

    let panel = app
        .get_webview_window(PANEL_LABEL)
        .ok_or_else(|| "panel window missing".to_string())?;
    if panel.is_maximized().unwrap_or(false) {
        if let Err(error) = panel.unmaximize() {
            let _ = settings::save_panel_maximized(app, true);
            return Err(error.to_string());
        }
        if let Some(runtime) = runtime(app) {
            runtime.last_maximized.store(false, Ordering::SeqCst);
        }
        emit_window_state(app, false);
        schedule_restore_normal_geometry(app.clone(), !reset_position);
    } else {
        apply_preferred_normal_geometry(app, current_ui_scale(app), !reset_position)?;
    }
    window::assert_panel_always_on_top(app);
    Ok(payload(false))
}
