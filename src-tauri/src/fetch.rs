//! Safe, one-shot `origin` fetch orchestration.
//!
//! The process runner is deliberately separate from the coordinator: the
//! runner owns the hardened system-Git invocation, while the coordinator owns
//! cooldown, the global cap, and per-repository single-flight semantics.
//! Auto-fetch and the explicit refresh button therefore share one in-flight
//! result instead of racing over `refs/remotes/origin/*`.

mod child_process;
mod coordinator;
mod process;
mod service;

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::AppHandle;

pub use coordinator::FetchCoordinator;
pub use service::{fetch_repo_now, maybe_fetch_repo, repo_fetch_capability, FetchCommandResult};

/// Skip a repository's automatic fetch if it was attempted within this window.
pub const FETCH_COOLDOWN: Duration = Duration::from_secs(180);

/// Automatic refresh remains generous for a large, long-stale repository.
pub const AUTO_FETCH_TIMEOUT: Duration = Duration::from_secs(150);

/// An explicit UI action must resolve promptly. If it joins a longer automatic
/// fetch, this also caps how long the command waits before reporting that the
/// shared operation is still in progress.
pub const MANUAL_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// A local capability probe must never leave the header action indefinitely
/// disabled when a disconnected UNC path or removable drive stops responding.
pub const CAPABILITY_TIMEOUT: Duration = Duration::from_secs(3);

/// At most two different repositories may contact their remotes concurrently.
pub(super) const MAX_INFLIGHT: usize = 2;

/// Successful automatic fetches are normally silent. Record the slow tail so
/// the original "sometimes it takes forever" report remains diagnosable.
pub const SLOW_AUTO_FETCH: Duration = Duration::from_secs(5);

/// Stable result categories exposed to the frontend and written to the local
/// bounded activity log. Raw Git stderr is captured only for classification;
/// it is never returned or persisted, so credential-bearing remote URLs cannot
/// leak through diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchOutcome {
    Ok,
    InProgress,
    Busy,
    NoOrigin,
    Authentication,
    Network,
    RefLock,
    Timeout,
    Unavailable,
    Failed,
}

/// Local-only preflight for the refresh action. Keeping `no_origin` distinct
/// from a missing/unreadable repository prevents a disabled button from
/// presenting the wrong explanation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchCapability {
    Available,
    NoOrigin,
    Unavailable,
}

impl FetchOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::InProgress => "in_progress",
            Self::Busy => "busy",
            Self::NoOrigin => "no_origin",
            Self::Authentication => "authentication",
            Self::Network => "network",
            Self::RefLock => "ref_lock",
            Self::Timeout => "timeout",
            Self::Unavailable => "unavailable",
            Self::Failed => "failed",
        }
    }
}

/// Record only a bounded, non-secret outcome category. `scan.log` already owns
/// rotation and contains local repository paths from discovery; fetch entries
/// add no remote URL or stderr content.
pub fn record_fetch_outcome(
    app: &AppHandle,
    trigger: &str,
    flight_id: Option<&str>,
    repo: &Path,
    outcome: FetchOutcome,
    elapsed: Duration,
    attempts: u8,
) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let line = format_fetch_log_line(now, trigger, flight_id, repo, outcome, elapsed, attempts);
    let _ = crate::discovery_orchestrator::append_scan_log(app, &line);
}

fn format_fetch_log_line(
    now: u64,
    trigger: &str,
    flight_id: Option<&str>,
    repo: &Path,
    outcome: FetchOutcome,
    elapsed: Duration,
    attempts: u8,
) -> String {
    let safe_path = repo.to_string_lossy().replace(['\r', '\n'], "\u{fffd}");
    format!(
        "{now} fetch: trigger={trigger} flight={} status={} attempts={attempts} elapsed_ms={} path={}",
        flight_id.unwrap_or("none"),
        outcome.as_str(),
        elapsed.as_millis(),
        safe_path
    )
}

pub fn should_record_fetch(
    trigger: &str,
    outcome: FetchOutcome,
    elapsed: Duration,
    attempts: u8,
) -> bool {
    trigger != "auto" || outcome != FetchOutcome::Ok || attempts > 1 || elapsed >= SLOW_AUTO_FETCH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_serialize_as_stable_snake_case_values() {
        assert_eq!(
            serde_json::to_string(&FetchOutcome::RefLock).unwrap(),
            "\"ref_lock\""
        );
        assert_eq!(
            serde_json::to_string(&FetchOutcome::InProgress).unwrap(),
            "\"in_progress\""
        );
        assert_eq!(
            serde_json::to_string(&FetchCapability::NoOrigin).unwrap(),
            "\"no_origin\""
        );
    }

    #[test]
    fn automatic_logging_keeps_failures_retries_and_slow_successes() {
        assert!(!should_record_fetch(
            "auto",
            FetchOutcome::Ok,
            Duration::from_secs(1),
            1
        ));
        assert!(should_record_fetch(
            "auto",
            FetchOutcome::Ok,
            Duration::from_secs(1),
            2
        ));
        assert!(should_record_fetch(
            "auto",
            FetchOutcome::Ok,
            SLOW_AUTO_FETCH,
            1
        ));
        assert!(should_record_fetch(
            "auto",
            FetchOutcome::Network,
            Duration::from_millis(10),
            1
        ));
        assert!(should_record_fetch(
            "manual",
            FetchOutcome::Ok,
            Duration::from_millis(10),
            1
        ));
    }

    #[test]
    fn fetch_log_contains_only_bounded_structured_diagnostics() {
        let line = format_fetch_log_line(
            123,
            "manual",
            Some("f2a"),
            Path::new("C:/repo\nforged"),
            FetchOutcome::Authentication,
            Duration::from_millis(42),
            2,
        );
        assert_eq!(
            line,
            "123 fetch: trigger=manual flight=f2a status=authentication attempts=2 elapsed_ms=42 path=C:/repo�forged"
        );
        assert!(!line.contains('\n'));
        assert!(!line.contains("https://"));
    }
}
