//! Coalescing and cancellation for the expensive live file-history walk.
//!
//! A panel navigation is represented by an operation lease. Starting a newer
//! operation in the same UI flow supersedes the older lease immediately. Jobs
//! are shared by `(repo, path, refs fingerprint)`, so two consumers asking for
//! the same immutable Git snapshot do one revwalk. Removing the last subscriber
//! cancels that shared walk; cancellation is a normal `None` outcome, not an
//! error that should be surfaced to the user.

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::git::FileHistoryResult;

const FOLLOWER_CANCEL_POLL: Duration = Duration::from_millis(25);
const MAX_CLIENT_ID_BYTES: usize = 128;
const MAX_ACTIVE_FLOWS: usize = 32;

type SharedResult = Result<Option<FileHistoryResult>, String>;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct FileHistoryJobKey {
    repo_path: String,
    file_path: String,
    refs_fingerprint: String,
}

impl FileHistoryJobKey {
    pub fn new(repo_path: String, file_path: String, refs_fingerprint: String) -> Self {
        Self {
            repo_path,
            file_path,
            refs_fingerprint,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct OperationKey {
    flow_id: String,
    generation: u64,
}

#[derive(Clone)]
pub struct FileHistoryOperation {
    flow_id: String,
    operation_id: String,
    generation: u64,
    cancellation: CancellationToken,
}

impl FileHistoryOperation {
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    fn key(&self) -> OperationKey {
        OperationKey {
            flow_id: self.flow_id.clone(),
            generation: self.generation,
        }
    }
}

struct FlowEntry {
    operation_id: String,
    generation: u64,
    cancellation: CancellationToken,
    job_key: Option<FileHistoryJobKey>,
}

struct SharedJob {
    cancellation: CancellationToken,
    result: Mutex<Option<SharedResult>>,
    ready: Condvar,
}

impl SharedJob {
    fn new() -> Self {
        Self {
            cancellation: CancellationToken::new(),
            result: Mutex::new(None),
            ready: Condvar::new(),
        }
    }

    fn publish(&self, result: SharedResult) {
        let mut slot = self.result.lock().unwrap_or_else(|e| e.into_inner());
        *slot = Some(result);
        self.ready.notify_all();
    }

    /// A coalesced follower must still leave promptly when only its own UI
    /// operation is superseded, even if other subscribers keep the shared job
    /// alive. The 25 ms wait is well inside the 100 ms cancellation budget.
    fn wait(&self, subscriber: &CancellationToken) -> SharedResult {
        let mut slot = self.result.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if subscriber.is_cancelled() {
                return Ok(None);
            }
            if let Some(result) = slot.as_ref() {
                return result.clone();
            }
            let (next, _) = self
                .ready
                .wait_timeout(slot, FOLLOWER_CANCEL_POLL)
                .unwrap_or_else(|e| e.into_inner());
            slot = next;
        }
    }
}

struct JobEntry {
    job: Arc<SharedJob>,
    subscribers: HashSet<OperationKey>,
}

#[derive(Default)]
struct State {
    next_generation: u64,
    flows: HashMap<String, FlowEntry>,
    jobs: HashMap<FileHistoryJobKey, JobEntry>,
}

#[derive(Default)]
pub struct FileHistoryJobs {
    state: Mutex<State>,
}

impl FileHistoryJobs {
    /// Claim the current slot for a UI flow. This synchronously supersedes its
    /// previous operation, so cancellation does not depend on a best-effort
    /// frontend cleanup IPC arriving first.
    pub fn begin(
        &self,
        flow_id: String,
        operation_id: String,
    ) -> Result<FileHistoryOperation, String> {
        validate_client_id("flow id", &flow_id)?;
        validate_client_id("operation id", &operation_id)?;

        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if !state.flows.contains_key(&flow_id) && state.flows.len() >= MAX_ACTIVE_FLOWS {
            return Err(format!(
                "too many active file history flows (limit {MAX_ACTIVE_FLOWS})"
            ));
        }
        if let Some(previous) = state.flows.remove(&flow_id) {
            detach_flow(&mut state, &flow_id, previous);
        }

        state.next_generation = state.next_generation.wrapping_add(1).max(1);
        let generation = state.next_generation;
        let cancellation = CancellationToken::new();
        state.flows.insert(
            flow_id.clone(),
            FlowEntry {
                operation_id: operation_id.clone(),
                generation,
                cancellation: cancellation.clone(),
                job_key: None,
            },
        );
        Ok(FileHistoryOperation {
            flow_id,
            operation_id,
            generation,
            cancellation,
        })
    }

    /// Cancel exactly one operation. A late cleanup from an older React effect
    /// cannot cancel the newer request occupying the same flow.
    pub fn cancel(&self, flow_id: &str, operation_id: &str) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let matches = state
            .flows
            .get(flow_id)
            .is_some_and(|entry| entry.operation_id == operation_id);
        if matches {
            if let Some(entry) = state.flows.remove(flow_id) {
                detach_flow(&mut state, flow_id, entry);
            }
        }
    }

    /// Join or lead a shared immutable-snapshot job. `run` is invoked exactly
    /// once for all concurrent subscribers to `key`.
    pub fn execute<F>(
        &self,
        operation: FileHistoryOperation,
        key: FileHistoryJobKey,
        run: F,
    ) -> SharedResult
    where
        F: FnOnce(&CancellationToken) -> SharedResult,
    {
        let Some((job, is_leader)) = self.attach(&operation, &key) else {
            self.finish(&operation);
            return Ok(None);
        };

        let result = if is_leader {
            // Never strand coalesced followers on an unexpected worker panic.
            let result = catch_unwind(AssertUnwindSafe(|| run(&job.cancellation)))
                .unwrap_or_else(|_| Err("file history worker panicked".to_string()));
            job.publish(result.clone());
            self.remove_completed_job(&key, &job);
            result
        } else {
            job.wait(&operation.cancellation)
        };

        // The worker may have completed after this operation was superseded.
        // Suppress that stale success in the backend as a second line of
        // defense behind React's effect cleanup guard.
        let deliver = self.is_current(&operation) && !operation.is_cancelled();
        self.finish(&operation);
        if deliver {
            result
        } else {
            Ok(None)
        }
    }

    /// Finish a lease that failed before it acquired a shared job (for example
    /// while opening the repository to calculate its refs fingerprint).
    pub fn finish(&self, operation: &FileHistoryOperation) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let current = state
            .flows
            .get(&operation.flow_id)
            .is_some_and(|entry| entry.generation == operation.generation);
        if current {
            if let Some(entry) = state.flows.remove(&operation.flow_id) {
                detach_flow(&mut state, &operation.flow_id, entry);
            }
        }
    }

    fn attach(
        &self,
        operation: &FileHistoryOperation,
        key: &FileHistoryJobKey,
    ) -> Option<(Arc<SharedJob>, bool)> {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if operation.is_cancelled()
            || !state.flows.get(&operation.flow_id).is_some_and(|entry| {
                entry.generation == operation.generation
                    && entry.operation_id == operation.operation_id
            })
        {
            return None;
        }

        // A last-subscriber cancellation may still be unwinding in its
        // blocking thread. Never join its already-cancelled job: replace the
        // map slot and let the old Arc finish independently. Pointer identity
        // in `remove_completed_job` prevents the old completion removing us.
        let cancelled_job = state
            .jobs
            .get(key)
            .is_some_and(|entry| entry.job.cancellation.is_cancelled());
        if cancelled_job {
            state.jobs.remove(key);
        }

        let operation_key = operation.key();
        let (job, is_leader) = if let Some(existing) = state.jobs.get_mut(key) {
            existing.subscribers.insert(operation_key);
            (Arc::clone(&existing.job), false)
        } else {
            let job = Arc::new(SharedJob::new());
            state.jobs.insert(
                key.clone(),
                JobEntry {
                    job: Arc::clone(&job),
                    subscribers: HashSet::from([operation_key]),
                },
            );
            (job, true)
        };
        if let Some(flow) = state.flows.get_mut(&operation.flow_id) {
            flow.job_key = Some(key.clone());
        }
        Some((job, is_leader))
    }

    fn is_current(&self, operation: &FileHistoryOperation) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .flows
            .get(&operation.flow_id)
            .is_some_and(|entry| {
                entry.generation == operation.generation
                    && entry.operation_id == operation.operation_id
            })
    }

    fn remove_completed_job(&self, key: &FileHistoryJobKey, completed: &Arc<SharedJob>) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let same_job = state
            .jobs
            .get(key)
            .is_some_and(|entry| Arc::ptr_eq(&entry.job, completed));
        if same_job {
            state.jobs.remove(key);
            for flow in state.flows.values_mut() {
                if flow.job_key.as_ref() == Some(key) {
                    flow.job_key = None;
                }
            }
        }
    }

    #[cfg(test)]
    fn counts(&self) -> (usize, usize) {
        let state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        (state.flows.len(), state.jobs.len())
    }
}

fn detach_flow(state: &mut State, flow_id: &str, entry: FlowEntry) {
    entry.cancellation.cancel();
    let Some(key) = entry.job_key else { return };
    let operation_key = OperationKey {
        flow_id: flow_id.to_string(),
        generation: entry.generation,
    };
    let should_cancel = state.jobs.get_mut(&key).is_some_and(|job| {
        job.subscribers.remove(&operation_key);
        job.subscribers.is_empty()
    });
    if should_cancel {
        if let Some(job) = state.jobs.get(&key) {
            job.job.cancellation.cancel();
        }
    }
}

fn validate_client_id(label: &str, value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_CLIENT_ID_BYTES {
        return Err(format!(
            "{label} must be between 1 and {MAX_CLIENT_ID_BYTES} bytes"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::thread;
    use std::time::{Duration, Instant};

    fn result(label: &str) -> SharedResult {
        Ok(Some(FileHistoryResult {
            rows: Vec::new(),
            commits_scanned: label.len(),
            truncated: None,
            remote_refs_included: 0,
            follows_renames: false,
            snapshot_changed: false,
        }))
    }

    fn key() -> FileHistoryJobKey {
        FileHistoryJobKey::new("repo".into(), "a.txt".into(), "refs".into())
    }

    #[test]
    fn identical_concurrent_requests_share_one_worker() {
        let jobs = Arc::new(FileHistoryJobs::default());
        let runs = Arc::new(AtomicUsize::new(0));
        let first = jobs.begin("flow-a".into(), "op-a".into()).unwrap();
        let second = jobs.begin("flow-b".into(), "op-b".into()).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();

        let jobs_a = Arc::clone(&jobs);
        let runs_a = Arc::clone(&runs);
        let leader = thread::spawn(move || {
            jobs_a.execute(first, key(), |_| {
                runs_a.fetch_add(1, Ordering::SeqCst);
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                result("shared")
            })
        });
        started_rx.recv().unwrap();

        let jobs_b = Arc::clone(&jobs);
        let runs_b = Arc::clone(&runs);
        let follower = thread::spawn(move || {
            jobs_b.execute(second, key(), |_| {
                runs_b.fetch_add(1, Ordering::SeqCst);
                result("duplicate")
            })
        });
        thread::sleep(Duration::from_millis(20));
        release_tx.send(()).unwrap();

        assert!(leader.join().unwrap().unwrap().is_some());
        assert!(follower.join().unwrap().unwrap().is_some());
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(jobs.counts(), (0, 0));
    }

    #[test]
    fn supersession_cancels_last_subscriber_within_budget() {
        let jobs = Arc::new(FileHistoryJobs::default());
        let old = jobs.begin("panel".into(), "old".into()).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let jobs_old = Arc::clone(&jobs);
        let worker = thread::spawn(move || {
            jobs_old.execute(old, key(), |cancel| {
                started_tx.send(()).unwrap();
                while !cancel.is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                Ok(None)
            })
        });
        started_rx.recv().unwrap();
        let began = Instant::now();
        let replacement = jobs.begin("panel".into(), "new".into()).unwrap();
        assert!(worker.join().unwrap().unwrap().is_none());
        assert!(
            began.elapsed() < Duration::from_millis(100),
            "cancellation took {:?}",
            began.elapsed()
        );
        jobs.finish(&replacement);
        assert_eq!(jobs.counts(), (0, 0));
    }

    #[test]
    fn stale_worker_success_is_never_delivered() {
        let jobs = Arc::new(FileHistoryJobs::default());
        let old = jobs.begin("panel".into(), "old".into()).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let jobs_old = Arc::clone(&jobs);
        let worker = thread::spawn(move || {
            jobs_old.execute(old, key(), |_| {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                // Deliberately ignore cancellation to model a non-interruptible
                // filesystem call completing late.
                result("stale")
            })
        });
        started_rx.recv().unwrap();
        let replacement = jobs.begin("panel".into(), "new".into()).unwrap();
        release_tx.send(()).unwrap();
        assert!(worker.join().unwrap().unwrap().is_none());
        jobs.finish(&replacement);
        assert_eq!(jobs.counts(), (0, 0));
    }

    #[test]
    fn replacement_does_not_join_cancelled_same_key_job() {
        let jobs = Arc::new(FileHistoryJobs::default());
        let old = jobs.begin("panel".into(), "old".into()).unwrap();
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let jobs_old = Arc::clone(&jobs);
        let worker = thread::spawn(move || {
            jobs_old.execute(old, key(), |cancel| {
                started_tx.send(()).unwrap();
                while !cancel.is_cancelled() {
                    thread::sleep(Duration::from_millis(2));
                }
                // Keep the cancelled job in the map long enough for the
                // replacement to exercise the cancelled-slot path.
                release_rx.recv().unwrap();
                Ok(None)
            })
        });
        started_rx.recv().unwrap();

        let replacement = jobs.begin("panel".into(), "new".into()).unwrap();
        let current = jobs.execute(replacement, key(), |_| result("current"));
        assert!(current.unwrap().is_some());
        release_tx.send(()).unwrap();
        assert!(worker.join().unwrap().unwrap().is_none());
        assert_eq!(jobs.counts(), (0, 0));
    }

    #[test]
    fn late_cleanup_cannot_cancel_replacement() {
        let jobs = FileHistoryJobs::default();
        let old = jobs.begin("panel".into(), "old".into()).unwrap();
        let current = jobs.begin("panel".into(), "new".into()).unwrap();
        jobs.cancel("panel", "old");
        assert!(!current.is_cancelled());
        jobs.finish(&old);
        assert!(!current.is_cancelled());
        jobs.finish(&current);
        assert_eq!(jobs.counts(), (0, 0));
    }
}
