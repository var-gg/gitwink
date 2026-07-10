//! Self-update wiring on top of `tauri-plugin-updater`.
//!
//! A background loop checks GitHub Releases' `latest.json` once on
//! startup and every 24h afterwards. A found update lights the tray dot
//! + "Update available" menu item — there is deliberately no toast. The
//! modal is summoned only when the user clicks that menu item (or runs a
//! manual check). "Skip" / "Later" state lives in `settings.json`.

use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::settings::{self, UpdateCheckMode};
use crate::{tray, window};

const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const SNOOZE_SECS: i64 = 24 * 60 * 60;

/// The latest update gitwink knows about. `None` ⇒ up to date (or not
/// checked yet). Tauri-managed so the tray handler + IPC commands all
/// read the same slot.
#[derive(Default)]
pub struct UpdateState {
    pub available: Mutex<Option<AvailableUpdate>>,
}

#[derive(Clone, Serialize)]
pub struct AvailableUpdate {
    pub version: String,
    /// Release notes / changelog, carried through `latest.json`.
    pub notes: String,
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// True when this binary runs out of a Scoop install dir. Scoop manages
/// its own updates (`scoop update gitwink`); a self-update here would
/// overwrite files Scoop tracks and desync its manifest, so the in-app
/// updater is disabled entirely for Scoop installs.
pub fn installed_via_scoop() -> bool {
    std::env::current_exe()
        .map(|p| {
            p.to_string_lossy()
                .to_lowercase()
                .replace('/', "\\")
                .contains("\\scoop\\apps\\")
        })
        .unwrap_or(false)
}

/// True when this binary runs as an installed MSIX package — a Microsoft
/// Store install lives under `…\WindowsApps\`. The Store owns updates for
/// Store installs, so the in-app updater and its tray UI are disabled
/// entirely for MSIX, exactly as they are for Scoop.
pub fn installed_via_msix() -> bool {
    std::env::current_exe()
        .map(|p| {
            p.to_string_lossy()
                .to_lowercase()
                .replace('/', "\\")
                .contains("\\windowsapps\\")
        })
        .unwrap_or(false)
}

/// Spawn the background check loop: one check on startup, then every 24h.
/// The `update_check` mode is re-read each iteration so a `settings.json`
/// edit takes effect without a restart.
pub fn start(app: AppHandle) {
    // MSIX (Microsoft Store) updates are owned by the Store, not the
    // GitHub-releases updater, so the check loop skips MSIX. Scoop installs
    // DO run it: detection uses the same `latest.json`, so a Scoop user still
    // gets the badge; only the install action differs (the modal routes Scoop
    // to `scoop update gitwink`).
    if installed_via_msix() {
        return;
    }
    std::thread::spawn(move || loop {
        if settings::load(&app).update_check == UpdateCheckMode::Enabled {
            let app = app.clone();
            tauri::async_runtime::block_on(async move {
                if let Err(e) = run_check(&app, false).await {
                    eprintln!("gitwink: update check failed: {e}");
                }
            });
        }
        std::thread::sleep(CHECK_INTERVAL);
    });
}

/// Drop any cached "an update is available" snapshot so the indicator
/// goes dark immediately and a stale version can't re-appear when the
/// user toggles back to Enabled/Manual. Called when update_check flips
/// to Disabled (GPT Pro review E3).
pub fn clear_cached_available(app: &AppHandle) {
    if let Ok(mut slot) = app.state::<UpdateState>().available.lock() {
        *slot = None;
    }
}

/// Run one update check off the main thread. `manual = false` so the
/// no-update path stays silent — used to wake the updater after the
/// user toggles update_check to Enabled without waiting up to 24h for
/// the background loop's next tick (GPT Pro review E1).
pub fn check_now_background(app: &AppHandle) {
    if installed_via_msix() {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_check(&app, false).await {
            eprintln!("gitwink: triggered update check failed: {e}");
        }
    });
}

/// Tray "Check for updates" entry point. Surfaces the modal on a hit
/// regardless of skip/snooze state — the user explicitly asked. Scoop runs
/// a real check now (same `latest.json`); on a hit the modal shows its Scoop
/// branch (the `scoop update` hint) rather than an in-app install button.
pub fn manual_check(app: &AppHandle) {
    if installed_via_msix() {
        // Store-managed — no GitHub-releases check here.
        return;
    }
    if settings::load(app).update_check == UpdateCheckMode::Disabled {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_check(&app, true).await {
            eprintln!("gitwink: manual update check failed: {e}");
        }
    });
}

/// Summon the panel + tell the frontend to open the update modal. Wired
/// to the tray "Update available" menu item.
pub fn open_modal(app: &AppHandle) {
    window::show_panel(app);
    let _ = app.emit("update://show-modal", ());
}

/// Run one update check. `manual` ⇒ surface the modal on a hit and emit
/// `update://none` on a miss (an explicit user request wants feedback).
async fn run_check(app: &AppHandle, manual: bool) -> anyhow::Result<()> {
    let update = app.updater()?.check().await?;
    {
        let state = app.state::<UpdateState>();
        let mut slot = state.available.lock().unwrap();
        *slot = update.as_ref().map(|u| AvailableUpdate {
            version: u.version.clone(),
            notes: u.body.clone().unwrap_or_default(),
        });
    }
    refresh_indicator(app);
    match (update.is_some(), manual) {
        (true, true) => open_modal(app),
        (false, true) => {
            let _ = app.emit("update://none", ());
        }
        _ => {}
    }
    Ok(())
}

/// Recompute the tray dot + menu item from current state + settings.
/// Shown iff an update exists, isn't the skipped version, isn't
/// snoozed, and the user hasn't switched update_check to Disabled
/// (Disabled means "no tray affordances", per the enum doc).
pub fn refresh_indicator(app: &AppHandle) {
    let s = settings::load(app);
    let version = if s.update_check == UpdateCheckMode::Disabled {
        None
    } else {
        let state = app.state::<UpdateState>();
        let slot = state.available.lock().unwrap();
        slot.as_ref().and_then(|u| {
            let skipped = s.update_skipped_version.as_deref() == Some(u.version.as_str());
            let snoozed = s.update_snooze_until.is_some_and(|t| now() < t);
            (!skipped && !snoozed).then(|| u.version.clone())
        })
    };
    // Mirror the indicator to the panel so the header icon can badge it —
    // same gating as the tray dot (hidden when skipped / snoozed / disabled).
    // Payload is the version string, or null when there's nothing to show.
    let _ = app.emit("update://indicator", version.clone());
    tray::set_update_indicator(app, version);
}

/// Mark the current available version as skipped — the indicator hides
/// for this version; a newer release re-surfaces it.
pub fn skip_current(app: &AppHandle) {
    let version = app
        .state::<UpdateState>()
        .available
        .lock()
        .unwrap()
        .as_ref()
        .map(|u| u.version.clone());
    if version.is_some() {
        settings::save_update_skipped_version(app, version);
        refresh_indicator(app);
    }
}

/// Hide the indicator for 24h ("Later").
pub fn snooze(app: &AppHandle) {
    settings::save_update_snooze_until(app, Some(now() + SNOOZE_SECS));
    refresh_indicator(app);
}
