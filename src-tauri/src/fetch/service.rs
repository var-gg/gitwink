use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};

use super::child_process::Deadline;
use super::coordinator::ManualFetchClaim;
use super::process::{git_fetch_one_shot, resolve_fetch_target};
use super::{
    record_fetch_outcome, should_record_fetch, FetchCapability, FetchCoordinator, FetchOutcome,
    AUTO_FETCH_TIMEOUT, CAPABILITY_TIMEOUT, FETCH_COOLDOWN, MANUAL_FETCH_TIMEOUT,
};

const FETCH_STATE_EVENT: &str = "fetch://state";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchCommandResult {
    pub flight_id: Option<String>,
    pub outcome: FetchOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct FetchStateEvent {
    flight_id: String,
    repo_path: String,
    phase: &'static str,
    trigger: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    outcome: Option<FetchOutcome>,
    #[serde(skip_serializing_if = "Option::is_none")]
    elapsed_ms: Option<u64>,
}

pub fn maybe_fetch_repo(app: AppHandle, repo_path: String) {
    let repo = PathBuf::from(repo_path);
    tauri::async_runtime::spawn_blocking(move || run_auto_fetch(app, repo));
}

pub async fn fetch_repo_now(
    app: AppHandle,
    repo_path: String,
) -> Result<FetchCommandResult, String> {
    if repo_path.trim().is_empty() {
        return Err("empty repo path".into());
    }
    let repo = PathBuf::from(repo_path);
    tauri::async_runtime::spawn_blocking(move || run_manual_fetch(app, repo))
        .await
        .map_err(|error| error.to_string())
}

pub async fn repo_fetch_capability(repo_path: String) -> Result<FetchCapability, String> {
    if repo_path.trim().is_empty() {
        return Ok(FetchCapability::Unavailable);
    }
    tauri::async_runtime::spawn_blocking(move || {
        super::process::repo_fetch_capability(
            Path::new(&repo_path),
            Deadline::after(CAPABILITY_TIMEOUT),
        )
    })
    .await
    .map_err(|error| error.to_string())
}

fn run_auto_fetch(app: AppHandle, repo: PathBuf) {
    let started = Instant::now();
    let deadline = Deadline::after(AUTO_FETCH_TIMEOUT);
    let target = match resolve_fetch_target(&repo, deadline) {
        Ok(target) => target,
        Err(outcome) => {
            record_if_needed(&app, "auto", None, &repo, outcome, started.elapsed(), 0);
            return;
        }
    };
    let Some(coordinator) = app.try_state::<FetchCoordinator>() else {
        record_if_needed(
            &app,
            "auto",
            None,
            &repo,
            FetchOutcome::Unavailable,
            started.elapsed(),
            0,
        );
        return;
    };
    let Some(guard) = coordinator.try_claim(target.key, FETCH_COOLDOWN) else {
        return;
    };
    let flight_id = guard.flight_id().to_owned();
    emit_started(&app, &flight_id, &repo, "auto");
    let run = git_fetch_one_shot(&repo, deadline);
    guard.finish(run.outcome);
    finish_owner(
        &app,
        "auto",
        &flight_id,
        &repo,
        run.outcome,
        started.elapsed(),
        run.attempts,
    );
}

fn run_manual_fetch(app: AppHandle, repo: PathBuf) -> FetchCommandResult {
    let started = Instant::now();
    let deadline = Deadline::after(MANUAL_FETCH_TIMEOUT);
    let target = match resolve_fetch_target(&repo, deadline) {
        Ok(target) => target,
        Err(outcome) => {
            record_if_needed(&app, "manual", None, &repo, outcome, started.elapsed(), 0);
            return FetchCommandResult {
                flight_id: None,
                outcome,
            };
        }
    };
    let Some(coordinator) = app.try_state::<FetchCoordinator>() else {
        let outcome = FetchOutcome::Unavailable;
        record_if_needed(&app, "manual", None, &repo, outcome, started.elapsed(), 0);
        return FetchCommandResult {
            flight_id: None,
            outcome,
        };
    };

    match coordinator.claim_forced(target.key) {
        ManualFetchClaim::Start(guard) => {
            let flight_id = guard.flight_id().to_owned();
            emit_started(&app, &flight_id, &repo, "manual");
            let run = git_fetch_one_shot(&repo, deadline);
            guard.finish(run.outcome);
            finish_owner(
                &app,
                "manual",
                &flight_id,
                &repo,
                run.outcome,
                started.elapsed(),
                run.attempts,
            );
            FetchCommandResult {
                flight_id: Some(flight_id),
                outcome: run.outcome,
            }
        }
        ManualFetchClaim::Join(flight) => {
            let flight_id = flight.id().to_owned();
            let outcome = deadline
                .remaining()
                .and_then(|remaining| flight.wait_timeout(remaining))
                .unwrap_or(FetchOutcome::InProgress);
            record_if_needed(
                &app,
                "manual_join",
                Some(&flight_id),
                &repo,
                outcome,
                started.elapsed(),
                0,
            );
            FetchCommandResult {
                flight_id: Some(flight_id),
                outcome,
            }
        }
        ManualFetchClaim::Busy => {
            let outcome = FetchOutcome::Busy;
            record_if_needed(&app, "manual", None, &repo, outcome, started.elapsed(), 0);
            FetchCommandResult {
                flight_id: None,
                outcome,
            }
        }
    }
}

fn finish_owner(
    app: &AppHandle,
    trigger: &'static str,
    flight_id: &str,
    repo: &Path,
    outcome: FetchOutcome,
    elapsed: Duration,
    attempts: u8,
) {
    if outcome == FetchOutcome::Ok {
        crate::watcher::schedule_repo_refresh(app, repo);
    }
    let elapsed_ms = elapsed.as_millis().min(u64::MAX as u128) as u64;
    let _ = app.emit(
        FETCH_STATE_EVENT,
        FetchStateEvent {
            flight_id: flight_id.to_owned(),
            repo_path: repo.to_string_lossy().into_owned(),
            phase: "completed",
            trigger,
            outcome: Some(outcome),
            elapsed_ms: Some(elapsed_ms),
        },
    );
    record_if_needed(
        app,
        trigger,
        Some(flight_id),
        repo,
        outcome,
        elapsed,
        attempts,
    );
}

fn emit_started(app: &AppHandle, flight_id: &str, repo: &Path, trigger: &'static str) {
    let _ = app.emit(
        FETCH_STATE_EVENT,
        FetchStateEvent {
            flight_id: flight_id.to_owned(),
            repo_path: repo.to_string_lossy().into_owned(),
            phase: "started",
            trigger,
            outcome: None,
            elapsed_ms: None,
        },
    );
}

fn record_if_needed(
    app: &AppHandle,
    trigger: &str,
    flight_id: Option<&str>,
    repo: &Path,
    outcome: FetchOutcome,
    elapsed: Duration,
    attempts: u8,
) {
    if should_record_fetch(trigger, outcome, elapsed, attempts) {
        record_fetch_outcome(app, trigger, flight_id, repo, outcome, elapsed, attempts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_payload_uses_stable_flight_and_camel_case_fields() {
        let event = FetchStateEvent {
            flight_id: "f2a".into(),
            repo_path: "C:/repo".into(),
            phase: "completed",
            trigger: "manual",
            outcome: Some(FetchOutcome::Ok),
            elapsed_ms: Some(42),
        };
        assert_eq!(
            serde_json::to_value(event).unwrap(),
            serde_json::json!({
                "flightId": "f2a",
                "repoPath": "C:/repo",
                "phase": "completed",
                "trigger": "manual",
                "outcome": "ok",
                "elapsedMs": 42
            })
        );
    }

    #[test]
    fn started_payload_omits_terminal_only_fields() {
        let event = FetchStateEvent {
            flight_id: "f1".into(),
            repo_path: "/repo".into(),
            phase: "started",
            trigger: "auto",
            outcome: None,
            elapsed_ms: None,
        };
        let value = serde_json::to_value(event).unwrap();
        assert!(value.get("outcome").is_none());
        assert!(value.get("elapsedMs").is_none());
    }
}
