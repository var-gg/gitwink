use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use tauri::{AppHandle, Emitter, Manager};

use crate::window;

use super::model::{ControlDelivery, ControlSource};
use super::{parser, resolver};

pub const CONTROL_PENDING_EVENT: &str = "control://pending";
const RECENT_REQUEST_LIMIT: usize = 64;

#[derive(Default)]
struct PendingControlInner {
    latest_sequence: u64,
    deliveries: VecDeque<ControlDelivery>,
    recent_request_ids: VecDeque<String>,
}

/// Durable handoff queue between native activation and the persistent panel
/// webview. Events are wake signals only; the payload remains here until the
/// frontend drains it, closing both cold-start and listener-registration
/// races. Visual navigation is latest-wins so a slow old resolver can never
/// yank the user away after a newer request already landed.
pub struct PendingControl {
    next_sequence: AtomicU64,
    inner: Mutex<PendingControlInner>,
}

impl Default for PendingControl {
    fn default() -> Self {
        Self {
            next_sequence: AtomicU64::new(1),
            inner: Mutex::new(PendingControlInner::default()),
        }
    }
}

impl PendingControl {
    fn begin(&self, request_id: &str) -> Option<u64> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if inner
            .recent_request_ids
            .iter()
            .any(|existing| existing == request_id)
        {
            return None;
        }
        inner.recent_request_ids.push_back(request_id.to_string());
        while inner.recent_request_ids.len() > RECENT_REQUEST_LIMIT {
            inner.recent_request_ids.pop_front();
        }
        let sequence = self.next_sequence.fetch_add(1, Ordering::Relaxed);
        inner.latest_sequence = sequence;
        Some(sequence)
    }

    fn publish(&self, sequence: u64, delivery: ControlDelivery) -> bool {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if sequence != inner.latest_sequence {
            return false;
        }
        inner.deliveries.clear();
        inner.deliveries.push_back(delivery);
        true
    }

    fn take_all(&self) -> Vec<ControlDelivery> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .deliveries
            .drain(..)
            .collect()
    }
}

/// Accept initial-process or single-instance-forwarded argv. Returns true when
/// the invocation was a recognized control request (including malformed
/// requests that were queued as visible errors); false leaves normal tray
/// startup behavior untouched.
pub fn accept_app_argv(app: &AppHandle, argv: Vec<String>, cwd: String) -> bool {
    let parsed = parser::parse_app_argv(&argv, &cwd);
    let Some(state) = app.try_state::<PendingControl>() else {
        return false;
    };

    let request = match parsed {
        Ok(None) => return false,
        Ok(Some(request)) => request,
        Err(error) => {
            window::show_panel(app);
            let request_id = parser::new_request_id();
            let Some(sequence) = state.begin(&request_id) else {
                return true;
            };
            let delivery = ControlDelivery::Error {
                request_id,
                source: ControlSource::default(),
                error: error.failure(),
            };
            if state.publish(sequence, delivery) {
                let _ = app.emit_to("panel", CONTROL_PENDING_EVENT, ());
            }
            return true;
        }
    };

    window::show_panel(app);
    let Some(sequence) = state.begin(&request.request_id) else {
        return true;
    };
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let delivery = match resolver::resolve(&handle, &request) {
            Ok(target) => ControlDelivery::Ready {
                request_id: request.request_id,
                source: request.source,
                target,
            },
            Err(error) => ControlDelivery::Error {
                request_id: request.request_id,
                source: request.source,
                error: error.failure(),
            },
        };
        let Some(state) = handle.try_state::<PendingControl>() else {
            return;
        };
        if state.publish(sequence, delivery) {
            window::show_panel(&handle);
            let _ = handle.emit_to("panel", CONTROL_PENDING_EVENT, ());
        }
    });
    true
}

#[tauri::command]
pub fn take_pending_control_deliveries(
    state: tauri::State<'_, PendingControl>,
) -> Vec<ControlDelivery> {
    state.take_all()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::model::{ControlErrorCode, ControlFailure};

    fn error_delivery(request_id: &str) -> ControlDelivery {
        ControlDelivery::Error {
            request_id: request_id.to_string(),
            source: ControlSource::default(),
            error: ControlFailure {
                code: ControlErrorCode::InternalError,
                message: "test".to_string(),
            },
        }
    }

    #[test]
    fn latest_request_wins_even_when_old_resolution_finishes_late() {
        let state = PendingControl::default();
        let first = state.begin("first").unwrap();
        let second = state.begin("second").unwrap();
        assert!(!state.publish(first, error_delivery("first")));
        assert!(state.publish(second, error_delivery("second")));
        let deliveries = state.take_all();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].request_id(), "second");
    }

    #[test]
    fn duplicate_request_ids_are_ignored() {
        let state = PendingControl::default();
        assert!(state.begin("same").is_some());
        assert!(state.begin("same").is_none());
    }
}
