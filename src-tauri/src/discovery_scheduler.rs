//! Demand-driven discovery scheduling for the long-lived tray process.
//!
//! Repository enumeration is intentionally not a polling loop: learned roots
//! are rechecked when the user surfaces the panel. Requests are coalesced and
//! cooled down so rapid hotkey/tray toggles never start competing filesystem
//! walks.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

use crate::discovery_orchestrator;

const PANEL_REFRESH_COOLDOWN: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanTrigger {
    Startup,
    PanelShown,
}

impl ScanTrigger {
    fn cooldown(self) -> Duration {
        match self {
            Self::Startup => Duration::ZERO,
            Self::PanelShown => PANEL_REFRESH_COOLDOWN,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Startup => "startup",
            Self::PanelShown => "panel_shown",
        }
    }
}

#[derive(Debug, Default)]
struct ScanCoordinator {
    in_flight: bool,
    last_finished_at: Option<Instant>,
}

impl ScanCoordinator {
    fn request(&mut self, trigger: ScanTrigger, now: Instant) -> bool {
        if self.in_flight {
            return false;
        }
        if self
            .last_finished_at
            .is_some_and(|last| now.saturating_duration_since(last) < trigger.cooldown())
        {
            return false;
        }
        self.in_flight = true;
        true
    }

    fn finish(&mut self, now: Instant) {
        self.in_flight = false;
        self.last_finished_at = Some(now);
    }
}

/// Tauri-managed state read by the startup UI and every discovery trigger.
pub struct ScanState(Mutex<ScanCoordinator>);

impl Default for ScanState {
    fn default() -> Self {
        Self(Mutex::new(ScanCoordinator::default()))
    }
}

impl ScanState {
    pub fn is_scanning(&self) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .in_flight
    }
}

/// Owned cancellation root for discovery work scheduled during this process.
pub struct DiscoveryScheduler {
    cancel: CancellationToken,
}

impl Drop for DiscoveryScheduler {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// Start the initial cache prewarm. The returned handle must be managed by
/// Tauri before panel-surface requests can schedule later refreshes.
pub fn start(app: AppHandle) -> DiscoveryScheduler {
    let scheduler = DiscoveryScheduler {
        cancel: CancellationToken::new(),
    };
    request_scan(&app, scheduler.cancel.clone(), ScanTrigger::Startup);
    scheduler
}

/// Recheck IDE hints and learned roots after a long-lived tray process is
/// surfaced. This call is non-blocking and cheap when another/recent scan owns
/// the lease.
pub fn request_panel_refresh(app: &AppHandle) {
    let Some(scheduler) = app.try_state::<DiscoveryScheduler>() else {
        return;
    };
    request_scan(app, scheduler.cancel.clone(), ScanTrigger::PanelShown);
}

fn request_scan(app: &AppHandle, cancel: CancellationToken, trigger: ScanTrigger) {
    if cancel.is_cancelled() {
        return;
    }
    let should_start = {
        let state = app.state::<ScanState>();
        let mut coordinator = state
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        coordinator.request(trigger, Instant::now())
    };
    if !should_start {
        return;
    }

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Drop releases the coordinator even if the pipeline unwinds or the
        // future is cancelled while awaiting its blocking filesystem work.
        let _lease = ScanLease { app: app.clone() };
        if let Err(error) = discovery_orchestrator::run(&app, cancel, trigger.as_str()).await {
            eprintln!("discovery orchestrator: {error:#}");
        }
    });
}

struct ScanLease {
    app: AppHandle,
}

impl Drop for ScanLease {
    fn drop(&mut self) {
        if let Some(state) = self.app.try_state::<ScanState>() {
            state
                .0
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .finish(Instant::now());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_refresh_is_single_flight_and_cooldown_bounded() {
        let started = Instant::now();
        let mut coordinator = ScanCoordinator::default();
        assert!(coordinator.request(ScanTrigger::Startup, started));
        assert!(!coordinator.request(ScanTrigger::PanelShown, started + Duration::from_secs(1)));

        let finished = started + Duration::from_secs(10);
        coordinator.finish(finished);
        assert!(!coordinator.request(ScanTrigger::PanelShown, finished + Duration::from_secs(59)));
        assert!(coordinator.request(ScanTrigger::PanelShown, finished + Duration::from_secs(60)));
    }

    #[test]
    fn startup_is_not_suppressed_by_a_previous_completion_timestamp() {
        let now = Instant::now();
        let mut coordinator = ScanCoordinator {
            in_flight: false,
            last_finished_at: Some(now),
        };
        assert!(coordinator.request(ScanTrigger::Startup, now));
    }
}
