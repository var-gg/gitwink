//! Safe, one-shot `origin` fetch orchestration.
//!
//! The process runner is deliberately separate from the coordinator: the
//! runner owns the hardened system-Git invocation, while the coordinator owns
//! cooldown, the global cap, and per-repository single-flight semantics.
//! Auto-fetch and the explicit refresh button therefore share one in-flight
//! result instead of racing over `FETCH_HEAD` and `refs/remotes/origin/*`.

mod coordinator;
mod process;

use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::AppHandle;

pub use coordinator::{FetchCoordinator, ManualFetchClaim};
pub use process::{git_fetch_one_shot, repo_fetch_capability};

/// Skip a repository's automatic fetch if it was attempted within this window.
pub const FETCH_COOLDOWN: Duration = Duration::from_secs(180);

/// Automatic refresh remains generous for a large, long-stale repository.
pub const AUTO_FETCH_TIMEOUT: Duration = Duration::from_secs(150);

/// An explicit UI action must resolve promptly. If it joins a longer automatic
/// fetch, this also caps how long the command waits before reporting that the
/// shared operation is still in progress.
pub const MANUAL_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// At most two different repositories may contact their remotes concurrently.
pub(super) const MAX_INFLIGHT: usize = 2;

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
    repo: &Path,
    outcome: FetchOutcome,
    elapsed: Duration,
) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let line = format!(
        "{now} fetch: trigger={trigger} status={} elapsed_ms={} path={}",
        outcome.as_str(),
        elapsed.as_millis(),
        repo.display()
    );
    let _ = crate::discovery_orchestrator::append_scan_log(app, &line);
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
}
