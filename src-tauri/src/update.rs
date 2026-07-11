//! Channel-aware self-update orchestration.
//!
//! Direct and Scoop builds discover releases through Tauri's GitHub-backed
//! updater. Microsoft Store builds use StoreContext instead, so the badge is
//! shown only after the Store package is actually available to this device.
//! A found update lights the tray dot + panel badge; there is deliberately no
//! toast. "Skip" / "Later" state lives in `settings.json`.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Mutex,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::UpdaterExt;

use crate::settings::{self, UpdateCheckMode};
use crate::{store_update, tray, window};

const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
const SNOOZE_SECS: i64 = 24 * 60 * 60;

/// The latest update gitwink knows about. `None` ⇒ up to date (or not
/// checked yet). Tauri-managed so the tray handler + IPC commands all
/// read the same slot.
#[derive(Default)]
pub struct UpdateState {
    pub available: Mutex<Option<AvailableUpdate>>,
    check: Mutex<CheckCoordinator>,
    installing: AtomicBool,
}

#[derive(Default)]
struct CheckCoordinator {
    in_flight: bool,
    manual_requested: bool,
    invalidated: bool,
    rerun_requested: bool,
    rerun_manual_requested: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct CheckCompletion {
    valid: bool,
    manual_requested: bool,
    rerun_requested: bool,
    rerun_manual_requested: bool,
}

impl CheckCoordinator {
    /// Coalesce overlapping triggers into one channel request. A manual
    /// trigger upgrades an already-running background check so the eventual
    /// result is still surfaced to the user.
    fn request(&mut self, manual: bool) -> bool {
        if self.in_flight {
            if self.invalidated {
                self.rerun_requested = true;
                self.rerun_manual_requested |= manual;
            } else {
                self.manual_requested |= manual;
            }
            return false;
        }
        self.in_flight = true;
        self.invalidated = false;
        self.manual_requested = manual;
        true
    }

    /// Fence a running result after Updates is turned off. A later request is
    /// queued behind it rather than overlapping the Store/GitHub call.
    fn invalidate(&mut self) {
        if self.in_flight {
            self.invalidated = true;
            self.manual_requested = false;
        }
    }

    fn finish(&mut self) -> CheckCompletion {
        let completion = CheckCompletion {
            valid: !self.invalidated,
            manual_requested: !self.invalidated && self.manual_requested,
            rerun_requested: self.rerun_requested,
            rerun_manual_requested: self.rerun_manual_requested,
        };
        self.in_flight = false;
        self.manual_requested = false;
        self.invalidated = false;
        self.rerun_requested = false;
        self.rerun_manual_requested = false;
        completion
    }
}

/// Process-wide install lease. The modal disables its own button, but the
/// backend invariant also protects against duplicate IPC calls and multiple
/// webviews. Any error path releases the lease automatically.
pub struct InstallGuard {
    app: AppHandle,
}

impl InstallGuard {
    pub fn acquire(app: &AppHandle) -> Result<Self, &'static str> {
        let state = app.state::<UpdateState>();
        state
            .installing
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "An update is already being installed.")?;
        Ok(Self { app: app.clone() })
    }
}

impl Drop for InstallGuard {
    fn drop(&mut self) {
        self.app
            .state::<UpdateState>()
            .installing
            .store(false, Ordering::Release);
    }
}

pub fn install_in_progress(app: &AppHandle) -> bool {
    app.try_state::<UpdateState>()
        .is_some_and(|state| state.installing.load(Ordering::Acquire))
}

#[derive(Clone, Serialize)]
pub struct AvailableUpdate {
    pub version: String,
    /// Release notes or a channel-specific update explanation.
    pub notes: String,
    pub mandatory: bool,
}

fn can_suppress(update: &AvailableUpdate) -> bool {
    !update.mandatory
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    Direct,
    Scoop,
    Store,
}

pub fn install_channel() -> UpdateChannel {
    std::env::current_exe()
        .map(|path| classify_install_path(&path.to_string_lossy()))
        .unwrap_or(UpdateChannel::Direct)
}

fn classify_install_path(path: &str) -> UpdateChannel {
    let normalized = path.to_lowercase().replace('/', "\\");
    if normalized.contains("\\windowsapps\\") {
        UpdateChannel::Store
    } else if normalized.contains("\\scoop\\apps\\") {
        UpdateChannel::Scoop
    } else {
        UpdateChannel::Direct
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// True when this binary runs out of a Scoop install dir. Scoop owns the
/// install action (`scoop update gitwink`), while gitwink still performs
/// discovery so users receive the same update badge as other channels.
pub fn installed_via_scoop() -> bool {
    install_channel() == UpdateChannel::Scoop
}

/// True when this binary runs as an installed MSIX package — a Microsoft
/// Store install lives under `…\WindowsApps\`. Detection and installation
/// for this channel are routed through Windows.Services.Store instead of the
/// GitHub release updater.
pub fn installed_via_msix() -> bool {
    install_channel() == UpdateChannel::Store
}

/// Spawn the background check loop: one check on startup, then every 24h.
/// The `update_check` mode is re-read each iteration so a `settings.json`
/// edit takes effect without a restart.
pub fn start(app: AppHandle) {
    std::thread::spawn(move || loop {
        if settings::load(&app).update_check == UpdateCheckMode::Enabled {
            request_check(&app, false);
        }
        std::thread::sleep(CHECK_INTERVAL);
    });
}

/// Drop any cached "an update is available" snapshot so the indicator
/// goes dark immediately and a stale version can't re-appear when the
/// user toggles back to Enabled/Manual. Called when update_check flips
/// to Disabled (GPT Pro review E3).
pub fn clear_cached_available(app: &AppHandle) {
    let state = app.state::<UpdateState>();
    let mut check = state.check.lock().unwrap_or_else(|e| e.into_inner());
    check.invalidate();
    *state
        .available
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = None;
}

/// Run one update check off the main thread. `manual = false` so the
/// no-update path stays silent — used to wake the updater after the
/// user toggles update_check to Enabled without waiting up to 24h for
/// the background loop's next tick (GPT Pro review E1).
pub fn check_now_background(app: &AppHandle) {
    request_check(app, false);
}

/// Tray "Check for updates" entry point. Surfaces the modal on a hit
/// regardless of skip/snooze state — the user explicitly asked. Scoop runs
/// a real check now (same `latest.json`); on a hit the modal shows its Scoop
/// branch (the `scoop update` hint) rather than an in-app install button.
pub fn manual_check(app: &AppHandle) {
    if settings::load(app).update_check == UpdateCheckMode::Disabled {
        return;
    }
    // A tray action must always have visible feedback, including the
    // up-to-date and error paths.
    window::show_panel(app);
    let _ = app.emit("update://checking", ());
    request_check(app, true);
}

/// Summon the panel + tell the frontend to open the update modal. Wired
/// to the tray "Update available" menu item.
pub fn open_modal(app: &AppHandle) {
    window::show_panel(app);
    let _ = app.emit("update://show-modal", ());
}

fn request_check(app: &AppHandle, manual: bool) {
    let should_start = {
        let state = app.state::<UpdateState>();
        let mut check = state.check.lock().unwrap_or_else(|e| e.into_inner());
        check.request(manual)
    };
    if !should_start {
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = fetch_available(&app).await;

        // Turning the updater off while a request is in flight is a hard
        // publication fence: neither a late badge nor a user-facing result
        // may leak through after the setting changed.
        if settings::load(&app).update_check == UpdateCheckMode::Disabled {
            schedule_rerun(&app, finish_check(&app));
            return;
        }

        match result {
            Ok(available) => {
                let (completion, found) = finish_successful_check(&app, available);
                if let Some(found) = found {
                    refresh_indicator(&app);
                    if completion.manual_requested
                        && settings::load(&app).update_check != UpdateCheckMode::Disabled
                    {
                        surface_check_result(&app, found);
                    }
                }
                schedule_rerun(&app, completion);
            }
            Err(error) => {
                eprintln!("gitwink: update check failed: {error:#}");
                let completion = finish_check(&app);
                if completion.valid
                    && completion.manual_requested
                    && settings::load(&app).update_check != UpdateCheckMode::Disabled
                {
                    window::show_panel(&app);
                    let _ = app.emit("update://check-error", format!("{error:#}"));
                }
                schedule_rerun(&app, completion);
            }
        }
    });
}

fn finish_check(app: &AppHandle) -> CheckCompletion {
    let state = app.state::<UpdateState>();
    let mut check = state.check.lock().unwrap_or_else(|e| e.into_inner());
    check.finish()
}

fn schedule_rerun(app: &AppHandle, completion: CheckCompletion) {
    if completion.rerun_requested && settings::load(app).update_check != UpdateCheckMode::Disabled {
        request_check(app, completion.rerun_manual_requested);
    }
}

async fn fetch_available(app: &AppHandle) -> anyhow::Result<Option<AvailableUpdate>> {
    if installed_via_msix() {
        Ok(store_update::check(app)
            .await?
            .map(|update| AvailableUpdate {
                version: update.version,
                notes: if update.mandatory {
                    "Microsoft Store marked this update as required. gitwink will download and install it without leaving the app."
                        .to_string()
                } else {
                    "This update is ready from Microsoft Store. gitwink can download and install it without leaving the app."
                        .to_string()
                },
                mandatory: update.mandatory,
            }))
    } else {
        Ok(app.updater()?.check().await?.map(|update| AvailableUpdate {
            version: update.version,
            notes: update.body.unwrap_or_default(),
            mandatory: false,
        }))
    }
}

/// Finish and publish under one coordinator lock. `clear_cached_available`
/// takes the same lock order, so disabling Updates either wins first and
/// rejects this result, or wins second and clears it—never a stale ABA badge.
fn finish_successful_check(
    app: &AppHandle,
    available: Option<AvailableUpdate>,
) -> (CheckCompletion, Option<bool>) {
    let found = available.is_some();
    let state = app.state::<UpdateState>();
    let completion = {
        let mut check = state.check.lock().unwrap_or_else(|e| e.into_inner());
        let completion = check.finish();
        if completion.valid {
            let mut slot = state.available.lock().unwrap_or_else(|e| e.into_inner());
            *slot = available;
        }
        completion
    };
    let published = completion.valid.then_some(found);
    (completion, published)
}

fn surface_check_result(app: &AppHandle, found: bool) {
    if found {
        open_modal(app);
    } else {
        let _ = app.emit("update://none", ());
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProgressPayload {
    phase: String,
    progress: f64,
}

/// Emit normalized update progress for both GitHub and Store installers.
/// `progress` is always clamped to 0…1 before crossing the IPC boundary.
pub fn emit_progress(app: &AppHandle, phase: &str, progress: f64) {
    let _ = app.emit(
        "update://progress",
        UpdateProgressPayload {
            phase: phase.to_string(),
            progress: normalize_progress(progress),
        },
    );
}

fn normalize_progress(progress: f64) -> f64 {
    if progress.is_finite() {
        progress.clamp(0.0, 1.0)
    } else {
        0.0
    }
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
        let slot = state
            .available
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        slot.as_ref().and_then(|u| {
            // A Store-mandated package may never inherit an old per-version
            // skip preference from another channel or earlier check.
            let suppressible = can_suppress(u);
            let skipped =
                suppressible && s.update_skipped_version.as_deref() == Some(u.version.as_str());
            let snoozed = suppressible && s.update_snooze_until.is_some_and(|t| now() < t);
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
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .and_then(|u| can_suppress(u).then(|| u.version.clone()));
    if version.is_some() {
        settings::save_update_skipped_version(app, version);
        refresh_indicator(app);
    }
}

/// Hide an optional update for 24h ("Later"). Mandatory Store packages are
/// never suppressible, even through direct IPC.
pub fn snooze(app: &AppHandle) {
    let suppressible = app
        .state::<UpdateState>()
        .available
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .is_some_and(can_suppress);
    if !suppressible {
        return;
    }
    settings::save_update_snooze_until(app, Some(now() + SNOOZE_SECS));
    refresh_indicator(app);
}

#[cfg(test)]
mod tests {
    use super::{
        can_suppress, classify_install_path, normalize_progress, AvailableUpdate, CheckCoordinator,
        UpdateChannel,
    };

    #[test]
    fn install_channel_is_classified_case_and_separator_insensitively() {
        assert_eq!(
            classify_install_path(
                r"C:\Program Files\WindowsApps\var-gg.gitwink_0.11.0.0_x64\gitwink.exe"
            ),
            UpdateChannel::Store
        );
        assert_eq!(
            classify_install_path("C:/Users/me/SCOOP/apps/gitwink/current/gitwink.exe"),
            UpdateChannel::Scoop
        );
        assert_eq!(
            classify_install_path(r"C:\Program Files\gitwink\gitwink.exe"),
            UpdateChannel::Direct
        );
    }

    #[test]
    fn overlapping_checks_coalesce_without_losing_manual_feedback() {
        let mut coordinator = CheckCoordinator::default();
        assert!(coordinator.request(false));
        assert!(!coordinator.request(true));
        assert!(!coordinator.request(false));
        let completion = coordinator.finish();
        assert!(completion.valid);
        assert!(completion.manual_requested);
        assert!(!completion.rerun_requested);

        assert!(coordinator.request(false));
        let completion = coordinator.finish();
        assert!(completion.valid);
        assert!(!completion.manual_requested);
    }

    #[test]
    fn invalidated_check_is_discarded_and_a_new_trigger_waits_to_rerun() {
        let mut coordinator = CheckCoordinator::default();
        assert!(coordinator.request(true));
        coordinator.invalidate();
        assert!(!coordinator.request(true));

        let completion = coordinator.finish();
        assert!(!completion.valid);
        assert!(!completion.manual_requested);
        assert!(completion.rerun_requested);
        assert!(completion.rerun_manual_requested);

        assert!(coordinator.request(completion.rerun_manual_requested));
        let rerun = coordinator.finish();
        assert!(rerun.valid);
        assert!(rerun.manual_requested);
    }

    #[test]
    fn progress_payloads_are_always_finite_and_normalized() {
        assert_eq!(normalize_progress(-0.2), 0.0);
        assert_eq!(normalize_progress(0.42), 0.42);
        assert_eq!(normalize_progress(1.5), 1.0);
        assert_eq!(normalize_progress(f64::NAN), 0.0);
        assert_eq!(normalize_progress(f64::INFINITY), 0.0);
    }

    #[test]
    fn mandatory_updates_cannot_inherit_skip_or_snooze_suppression() {
        let mandatory = AvailableUpdate {
            version: "1.2.3".into(),
            notes: String::new(),
            mandatory: true,
        };
        assert!(!can_suppress(&mandatory));

        let optional = AvailableUpdate {
            mandatory: false,
            ..mandatory
        };
        assert!(can_suppress(&optional));
    }
}
