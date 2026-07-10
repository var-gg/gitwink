//! Orchestration for progressive full-history scans.
//!
//! `run_full_scan` is deterministic and testable. `HistoryIndexer` wraps it
//! in one bounded, coalescing worker so multiple UI/search hints cannot launch
//! competing O(history) walks.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{mpsc, Arc, LazyLock, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use rusqlite::Connection;
use tokio_util::sync::CancellationToken;

use crate::history_git::{self, WalkCompletion};
use crate::history_store::{self, FinalizeOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RunOutcome {
    AlreadyCurrent,
    Maintenance {
        deleted: usize,
        pending: bool,
    },
    Published {
        generation: i64,
        deleted: usize,
        commits: usize,
        maintenance_pending: bool,
    },
    Cancelled,
    RootsChanged,
}

#[derive(Default)]
struct MutationGate {
    detached: Mutex<bool>,
}

impl MutationGate {
    fn run<T>(
        &self,
        cancellation: &CancellationToken,
        operation: impl FnOnce() -> Result<T>,
    ) -> Result<Option<T>> {
        let detached = self
            .detached
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *detached || cancellation.is_cancelled() {
            return Ok(None);
        }
        let result = operation()?;
        drop(detached);
        Ok(Some(result))
    }

    fn detach(&self) {
        let mut detached = self
            .detached
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *detached = true;
    }
}

#[cfg(test)]
pub(crate) fn run_full_scan(
    conn: &mut Connection,
    repo_path: &Path,
    batch_size: usize,
    cancellation: &CancellationToken,
) -> Result<RunOutcome> {
    run_full_scan_with_gate(conn, repo_path, batch_size, cancellation, None)
}

fn run_full_scan_with_gate(
    conn: &mut Connection,
    repo_path: &Path,
    batch_size: usize,
    cancellation: &CancellationToken,
    mutation_gate: Option<&MutationGate>,
) -> Result<RunOutcome> {
    if cancellation.is_cancelled() {
        return Ok(RunOutcome::Cancelled);
    }
    let repo_path_string = repo_path.to_string_lossy().into_owned();
    let maintenance_pending = history_store::has_pending_cleanup(conn, &repo_path_string)?;
    let initial_roots = history_git::snapshot_roots(repo_path)?;
    if cancellation.is_cancelled() {
        return Ok(RunOutcome::Cancelled);
    }
    let existing = history_store::coverage(conn, &repo_path_string)?;
    let active_lifecycle = history_store::active_lifecycle_epoch(conn, &repo_path_string)?;

    let mut published_snapshot = None;
    if let Some(coverage) = &existing {
        if coverage.published_epoch > 0 {
            let roots = history_store::published_roots(conn, &repo_path_string)?;
            let snapshot = history_git::RootSnapshot::from_roots(roots);
            if coverage.indexed_root_fingerprint.as_deref() == Some(snapshot.fingerprint.as_str()) {
                published_snapshot = Some(snapshot);
            }
        }
        if coverage.state == history_store::CoverageState::Complete
            && coverage.target_lifecycle_epoch == active_lifecycle
            && coverage.indexed_root_fingerprint.as_deref()
                == Some(initial_roots.fingerprint.as_str())
            && published_snapshot
                .as_ref()
                .is_some_and(|published| published.roots == initial_roots.roots)
        {
            return if !maintenance_pending {
                Ok(RunOutcome::AlreadyCurrent)
            } else {
                Ok(RunOutcome::Maintenance {
                    deleted: 0,
                    pending: true,
                })
            };
        }
    }

    let delta_from = match published_snapshot {
        Some(published)
            if history_git::can_extend_incrementally(repo_path, &published, &initial_roots)? =>
        {
            Some(published)
        }
        _ => None,
    };
    if cancellation.is_cancelled() {
        return Ok(RunOutcome::Cancelled);
    }
    let lease = if let Some(gate) = mutation_gate {
        let Some(lease) = gate.run(cancellation, || {
            history_store::begin_scan(conn, &repo_path_string, initial_roots)
        })?
        else {
            return Ok(RunOutcome::Cancelled);
        };
        lease
    } else {
        history_store::begin_scan(conn, &repo_path_string, initial_roots)?
    };

    let walk_result = if let Some(published) = &delta_from {
        history_git::walk_history_delta(
            repo_path,
            &lease.target,
            published,
            batch_size,
            cancellation,
            |batch| {
                if let Some(gate) = mutation_gate {
                    if gate
                        .run(cancellation, || {
                            history_store::upsert_batch(conn, &lease, batch)
                        })?
                        .is_none()
                    {
                        anyhow::bail!("history mutation gate detached");
                    }
                } else {
                    history_store::upsert_batch(conn, &lease, batch)?;
                }
                Ok(())
            },
        )
    } else {
        history_git::walk_history(
            repo_path,
            &lease.target,
            batch_size,
            cancellation,
            |batch| {
                if let Some(gate) = mutation_gate {
                    if gate
                        .run(cancellation, || {
                            history_store::upsert_batch(conn, &lease, batch)
                        })?
                        .is_none()
                    {
                        anyhow::bail!("history mutation gate detached");
                    }
                } else {
                    history_store::upsert_batch(conn, &lease, batch)?;
                }
                Ok(())
            },
        )
    };
    let walk = match walk_result {
        Ok(completion) => completion,
        Err(error) => {
            if cancellation.is_cancelled() {
                return Ok(RunOutcome::Cancelled);
            }
            if let Some(gate) = mutation_gate {
                let _ = gate.run(cancellation, || {
                    history_store::fail_scan(conn, &lease, &error.to_string())
                });
            } else {
                let _ = history_store::fail_scan(conn, &lease, &error.to_string());
            }
            return Err(error);
        }
    };

    if matches!(walk, WalkCompletion::Cancelled(_)) || cancellation.is_cancelled() {
        return Ok(RunOutcome::Cancelled);
    }

    // This read is intentionally after EOF and immediately before the atomic
    // publication transaction. A changed full root vector keeps the old
    // published membership intact and schedules another pass; physical
    // reclamation remains a separate bounded maintenance job.
    let final_roots = match history_git::snapshot_roots(repo_path) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            if let Some(gate) = mutation_gate {
                let _ = gate.run(cancellation, || {
                    history_store::fail_scan(conn, &lease, &error.to_string())
                });
            } else {
                let _ = history_store::fail_scan(conn, &lease, &error.to_string());
            }
            return Err(error);
        }
    };
    if cancellation.is_cancelled() {
        return Ok(RunOutcome::Cancelled);
    }

    let mut publish = || match delta_from.as_ref() {
        Some(delta_from) => history_store::finalize_delta_scan(
            conn,
            &lease,
            delta_from,
            &final_roots,
            walk,
            Some(cancellation),
        ),
        None => history_store::finalize_scan(conn, &lease, &final_roots, walk, Some(cancellation)),
    };
    let finalized = if let Some(gate) = mutation_gate {
        let Some(finalized) = gate.run(cancellation, publish)? else {
            return Ok(RunOutcome::Cancelled);
        };
        finalized
    } else {
        publish()?
    };
    match finalized {
        FinalizeOutcome::Published {
            generation,
            deleted,
            commits,
            maintenance_pending,
        } => Ok(RunOutcome::Published {
            generation,
            deleted,
            commits,
            maintenance_pending,
        }),
        FinalizeOutcome::RootsChanged => Ok(RunOutcome::RootsChanged),
        FinalizeOutcome::Cancelled => Ok(RunOutcome::Cancelled),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RequestDisposition {
    Queued,
    Coalesced,
    QueueFull,
}

enum WorkerMessage {
    Wake,
    Shutdown,
}

pub(crate) fn is_network_path(path: &str) -> bool {
    if path.starts_with(r"\\") || path.starts_with("//") {
        return true;
    }
    #[cfg(windows)]
    {
        static DRIVE_CACHE: LazyLock<Mutex<HashMap<String, (Instant, bool)>>> =
            LazyLock::new(|| Mutex::new(HashMap::new()));
        let bytes = path.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && matches!(bytes[2], b'\\' | b'/')
        {
            let root = format!("{}:\\", (bytes[0] as char).to_ascii_uppercase());
            if let Ok(cache) = DRIVE_CACHE.lock() {
                if let Some((observed_at, remote)) = cache.get(&root) {
                    if observed_at.elapsed() < Duration::from_secs(5) {
                        return *remote;
                    }
                }
            }
            #[link(name = "kernel32")]
            extern "system" {
                fn GetDriveTypeW(root_path: *const u16) -> u32;
            }
            let wide = root
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect::<Vec<_>>();
            // DRIVE_REMOTE = 4. Root-only probing avoids touching the mapped
            // share itself and is cached for the process lifetime.
            let remote = unsafe { GetDriveTypeW(wide.as_ptr()) } == 4;
            if let Ok(mut cache) = DRIVE_CACHE.lock() {
                cache.insert(root, (Instant::now(), remote));
            }
            return remote;
        }
    }
    false
}

fn execute_worker_scan<F>(
    open_connection: &F,
    repo_path: &str,
    keep: usize,
    token: &CancellationToken,
    mutation_gate: &MutationGate,
) -> Option<RunOutcome>
where
    F: Fn() -> Result<Connection>,
{
    match open_connection() {
        Ok(mut conn) => match run_full_scan_with_gate(
            &mut conn,
            Path::new(repo_path),
            keep,
            token,
            Some(mutation_gate),
        ) {
            Ok(outcome) => Some(outcome),
            Err(error) => {
                if !token.is_cancelled() {
                    let _ = mutation_gate.run(token, || {
                        history_store::record_scan_failure(&conn, repo_path, &error.to_string())
                    });
                    eprintln!("gitwink: history index failed for {repo_path:?}: {error:#}");
                }
                None
            }
        },
        Err(error) => {
            if !token.is_cancelled() {
                eprintln!("gitwink: cannot open history cache for {repo_path:?}: {error:#}");
            }
            None
        }
    }
}

fn execute_maintenance<F>(open_connection: &F, repo_path: &str) -> Option<RunOutcome>
where
    F: Fn() -> Result<Connection>,
{
    match open_connection() {
        Ok(conn) => {
            if let Err(error) = history_store::retire_detached_scan(&conn, repo_path) {
                eprintln!(
                    "gitwink: cannot retire detached history scan for {repo_path:?}: {error:#}"
                );
                return None;
            }
            match history_store::resume_pending_cleanup(&conn, repo_path, None) {
                Ok(progress) => Some(RunOutcome::Maintenance {
                    deleted: progress.deleted,
                    pending: !progress.complete,
                }),
                Err(error) => {
                    eprintln!("gitwink: history maintenance failed for {repo_path:?}: {error:#}");
                    None
                }
            }
        }
        Err(error) => {
            eprintln!(
                "gitwink: cannot open history cache for maintenance {repo_path:?}: {error:#}"
            );
            None
        }
    }
}

#[derive(Default)]
struct WorkerState {
    pending: HashSet<String>,
    /// Background requests (watcher/ref invalidation) survive UI scope
    /// replacement. Pure UI demand is pruned/cancelled when its lease closes.
    background_pending: HashSet<String>,
    /// Paths queued only to retire/advance durable cleanup. These never touch
    /// Git (even for UNC paths) and rank below real scan demand.
    maintenance_only: HashSet<String>,
    scan_after_maintenance: HashSet<String>,
    /// A cancelled UNC child can remain blocked in the OS after the logical
    /// worker moves on. Keep one physical probe per path; normal re-runs wait
    /// for that child to exit while local maintenance remains allowed.
    detached_network: HashMap<String, bool>,
    demand_scope: HashSet<String>,
    demand_id: Option<u64>,
    latest_demand_id: u64,
    released_through_demand_id: u64,
    active: Option<ActiveJob>,
}

struct ActiveJob {
    repo_path: String,
    cancellation: CancellationToken,
    rerun_requested: bool,
    background_owned: bool,
    maintenance_only: bool,
    mutation_gate: Arc<MutationGate>,
}

fn pending_path_cmp(left: &String, right: &String) -> std::cmp::Ordering {
    let left_network = is_network_path(left);
    let right_network = is_network_path(right);
    left_network
        .cmp(&right_network)
        .then_with(|| left.cmp(right))
}

fn next_pending(state: &WorkerState) -> Option<String> {
    state
        .pending
        .iter()
        .filter(|path| {
            state.demand_scope.contains(*path)
                && !state.maintenance_only.contains(*path)
                && !state.detached_network.contains_key(*path)
        })
        .min_by(|left, right| pending_path_cmp(left, right))
        .cloned()
        .or_else(|| {
            state
                .pending
                .iter()
                .filter(|path| {
                    !state.maintenance_only.contains(*path)
                        && !state.detached_network.contains_key(*path)
                })
                .min_by(|left, right| pending_path_cmp(left, right))
                .cloned()
        })
        .or_else(|| {
            state
                .pending
                .iter()
                .filter(|path| state.maintenance_only.contains(*path))
                .min()
                .cloned()
        })
}

fn preempt_network_for_local_pending(state: &mut WorkerState) {
    let local_waiting = state.pending.iter().any(|path| {
        !state.maintenance_only.contains(path)
            && !state.detached_network.contains_key(path)
            && !is_network_path(path)
    });
    if !local_waiting {
        return;
    }
    if let Some(active) = state.active.as_mut() {
        if !active.maintenance_only && is_network_path(&active.repo_path) {
            active.rerun_requested = true;
            active.cancellation.cancel();
            active.mutation_gate.detach();
        }
    }
}

/// One process-wide history worker. Repository paths live once in the
/// coalescing set (bounded by the requested catalog scope); the channel is a
/// capacity-one wake edge, so a 1000-repo request cannot overflow and silently
/// strand its tail.
pub(crate) struct HistoryIndexer {
    sender: mpsc::SyncSender<WorkerMessage>,
    state: Arc<Mutex<WorkerState>>,
    shutdown: CancellationToken,
    worker: Mutex<Option<thread::JoinHandle<()>>>,
}

impl HistoryIndexer {
    pub(crate) fn start<F, C>(open_connection: F, batch_size: usize, on_outcome: C) -> Self
    where
        F: Fn() -> Result<Connection> + Send + Sync + 'static,
        C: Fn(&str, RunOutcome) + Send + Sync + 'static,
    {
        let (sender, receiver) = mpsc::sync_channel(1);
        let state = Arc::new(Mutex::new(WorkerState::default()));
        let worker_state = Arc::clone(&state);
        let worker_wake = sender.clone();
        let open_connection = Arc::new(open_connection);
        let on_outcome = Arc::new(on_outcome);
        let shutdown = CancellationToken::new();
        let worker_shutdown = shutdown.clone();
        let keep = if batch_size == 0 {
            history_git::DEFAULT_HISTORY_BATCH_SIZE
        } else {
            batch_size.clamp(1, history_git::MAX_HISTORY_BATCH_SIZE)
        };
        let worker = thread::Builder::new()
            .name("gitwink-history-index".to_string())
            .spawn(move || {
                'worker: while let Ok(message) = receiver.recv() {
                    if worker_shutdown.is_cancelled() || matches!(message, WorkerMessage::Shutdown)
                    {
                        break;
                    }
                    loop {
                        if worker_shutdown.is_cancelled() {
                            break 'worker;
                        }
                        let (repo_path, token, mutation_gate, maintenance_only) = {
                            let Ok(mut state) = worker_state.lock() else {
                                break 'worker;
                            };
                            // The current visible scope always jumps ahead of
                            // maintenance work, while lexical order keeps each
                            // class deterministic and starvation-free once the
                            // finite demand set drains.
                            let Some(repo_path) = next_pending(&state) else {
                                break;
                            };
                            state.pending.remove(&repo_path);
                            let background_owned = state.background_pending.remove(&repo_path);
                            let maintenance_only = state.maintenance_only.remove(&repo_path);
                            let token = CancellationToken::new();
                            let mutation_gate = Arc::new(MutationGate::default());
                            state.active = Some(ActiveJob {
                                repo_path: repo_path.clone(),
                                cancellation: token.clone(),
                                rerun_requested: false,
                                background_owned,
                                maintenance_only,
                                mutation_gate: Arc::clone(&mutation_gate),
                            });
                            (repo_path, token, mutation_gate, maintenance_only)
                        };
                        let outcome = if maintenance_only {
                            execute_maintenance(open_connection.as_ref(), &repo_path)
                        } else if is_network_path(&repo_path) {
                            let (result_sender, result_receiver) = mpsc::sync_channel(1);
                            let child_connection = Arc::clone(&open_connection);
                            let child_path = repo_path.clone();
                            let child_token = token.clone();
                            let child_gate = Arc::clone(&mutation_gate);
                            let child_finished =
                                Arc::new(std::sync::atomic::AtomicBool::new(false));
                            let completion_finished = Arc::clone(&child_finished);
                            let completion_state = Arc::clone(&worker_state);
                            let completion_wake = worker_wake.clone();
                            let completion_shutdown = worker_shutdown.clone();
                            let spawned = thread::Builder::new()
                                .name("gitwink-history-network".to_string())
                                .spawn(move || {
                                    let outcome = execute_worker_scan(
                                        child_connection.as_ref(),
                                        &child_path,
                                        keep,
                                        &child_token,
                                        child_gate.as_ref(),
                                    );
                                    completion_finished
                                        .store(true, std::sync::atomic::Ordering::Release);
                                    let _ = result_sender.send(outcome);
                                    let should_wake = if let Ok(mut state) = completion_state.lock()
                                    {
                                        let removed = state.detached_network.remove(&child_path);
                                        let rerun = removed.unwrap_or(false);
                                        if rerun && !completion_shutdown.is_cancelled() {
                                            state.pending.insert(child_path.clone());
                                        }
                                        removed.is_some() && state.pending.contains(&child_path)
                                    } else {
                                        false
                                    };
                                    if should_wake {
                                        let _ = completion_wake.try_send(WorkerMessage::Wake);
                                    }
                                });
                            if spawned.is_err() {
                                execute_worker_scan(
                                    open_connection.as_ref(),
                                    &repo_path,
                                    keep,
                                    &token,
                                    mutation_gate.as_ref(),
                                )
                            } else {
                                loop {
                                    match result_receiver
                                        .recv_timeout(std::time::Duration::from_millis(50))
                                    {
                                        Ok(outcome) => break outcome,
                                        Err(mpsc::RecvTimeoutError::Timeout)
                                            if token.is_cancelled()
                                                || worker_shutdown.is_cancelled() =>
                                        {
                                            mutation_gate.detach();
                                            let mut should_wake = false;
                                            if let Ok(mut state) = worker_state.lock() {
                                                let rerun =
                                                    state.active.as_ref().is_some_and(|active| {
                                                        active.rerun_requested
                                                            || active.background_owned
                                                            || state
                                                                .demand_scope
                                                                .contains(&repo_path)
                                                    });
                                                state
                                                    .detached_network
                                                    .insert(repo_path.clone(), rerun);
                                                if child_finished
                                                    .load(std::sync::atomic::Ordering::Acquire)
                                                {
                                                    let removed =
                                                        state.detached_network.remove(&repo_path);
                                                    if removed.unwrap_or(false) {
                                                        state.pending.insert(repo_path.clone());
                                                    }
                                                    should_wake = removed.is_some()
                                                        && state.pending.contains(&repo_path);
                                                }
                                            }
                                            if should_wake {
                                                let _ = worker_wake.try_send(WorkerMessage::Wake);
                                            }
                                            // The child may still be inside an
                                            // uncancellable SMB open. It owns
                                            // a cancelled token and checks it
                                            // before every DB mutation; detach
                                            // so visible local demand can run.
                                            break Some(RunOutcome::Cancelled);
                                        }
                                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                                        Err(mpsc::RecvTimeoutError::Disconnected) => break None,
                                    }
                                }
                            }
                        } else {
                            execute_worker_scan(
                                open_connection.as_ref(),
                                &repo_path,
                                keep,
                                &token,
                                mutation_gate.as_ref(),
                            )
                        };
                        if let Some(outcome) = outcome {
                            if !matches!(outcome, RunOutcome::Maintenance { .. }) {
                                on_outcome(&repo_path, outcome);
                            }
                        }
                        if let Ok(mut state) = worker_state.lock() {
                            let active = state.active.take();
                            let rerun_requested =
                                active.as_ref().is_some_and(|active| active.rerun_requested);
                            let background_owned = active
                                .as_ref()
                                .is_some_and(|active| active.background_owned);
                            let was_maintenance = active
                                .as_ref()
                                .is_some_and(|active| active.maintenance_only);
                            let roots_changed = matches!(outcome, Some(RunOutcome::RootsChanged));
                            let cancelled = matches!(outcome, Some(RunOutcome::Cancelled));
                            let still_demanded = state.demand_scope.contains(&repo_path);
                            let maintenance_pending = match outcome {
                                Some(RunOutcome::Published {
                                    maintenance_pending,
                                    ..
                                }) => maintenance_pending,
                                Some(RunOutcome::Maintenance { pending, .. }) => pending,
                                _ => false,
                            };
                            if !worker_shutdown.is_cancelled() {
                                if was_maintenance && rerun_requested {
                                    state.scan_after_maintenance.insert(repo_path.clone());
                                }
                                if cancelled {
                                    state.pending.insert(repo_path.clone());
                                    state.maintenance_only.insert(repo_path.clone());
                                    state.background_pending.insert(repo_path.clone());
                                    if rerun_requested || background_owned || still_demanded {
                                        state.scan_after_maintenance.insert(repo_path.clone());
                                    }
                                } else if outcome.is_none() && !was_maintenance {
                                    // Drain any staging lease left by the
                                    // failure once, but do not tight-loop an
                                    // unreadable repo. A new watcher/user hint
                                    // is the retry edge for real Git work.
                                    state.pending.insert(repo_path.clone());
                                    state.maintenance_only.insert(repo_path.clone());
                                    state.background_pending.insert(repo_path.clone());
                                } else if maintenance_pending {
                                    state.pending.insert(repo_path.clone());
                                    state.maintenance_only.insert(repo_path.clone());
                                    state.background_pending.insert(repo_path.clone());
                                    if rerun_requested {
                                        state.scan_after_maintenance.insert(repo_path.clone());
                                    }
                                } else if (was_maintenance
                                    && state.scan_after_maintenance.remove(&repo_path))
                                    || rerun_requested
                                    || (roots_changed && (background_owned || still_demanded))
                                {
                                    state.pending.insert(repo_path.clone());
                                    if background_owned {
                                        state.background_pending.insert(repo_path.clone());
                                    }
                                }
                            }
                        } else {
                            break 'worker;
                        }
                    }
                }
            })
            .expect("spawn history index worker");
        Self {
            sender,
            state,
            shutdown,
            worker: Mutex::new(Some(worker)),
        }
    }

    pub(crate) fn request(&self, repo_path: impl Into<String>) -> RequestDisposition {
        let repo_path = repo_path.into();
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return RequestDisposition::QueueFull,
        };
        if state.pending.contains(&repo_path) {
            if state.maintenance_only.contains(&repo_path) {
                state.scan_after_maintenance.insert(repo_path.clone());
            }
            if let Some(rerun) = state.detached_network.get_mut(&repo_path) {
                *rerun = true;
            }
            state.background_pending.insert(repo_path);
            preempt_network_for_local_pending(&mut state);
            return RequestDisposition::Coalesced;
        }
        if let Some(active) = state.active.as_mut() {
            if active.repo_path == repo_path {
                // Do not lose a watcher/search hint that lands after the
                // final root recheck but before this job clears `active`.
                active.rerun_requested = true;
                active.background_owned = true;
                return RequestDisposition::Coalesced;
            }
        }
        state.pending.insert(repo_path.clone());
        state.background_pending.insert(repo_path.clone());
        preempt_network_for_local_pending(&mut state);
        match self.sender.try_send(WorkerMessage::Wake) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => RequestDisposition::Queued,
            Err(mpsc::TrySendError::Disconnected(_)) => {
                state.pending.remove(&repo_path);
                state.background_pending.remove(&repo_path);
                RequestDisposition::QueueFull
            }
        }
    }

    pub(crate) fn request_cleanup(&self, repo_path: impl Into<String>) -> RequestDisposition {
        let repo_path = repo_path.into();
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return RequestDisposition::QueueFull,
        };
        state.pending.insert(repo_path.clone());
        state.maintenance_only.insert(repo_path.clone());
        state.background_pending.insert(repo_path.clone());
        match self.sender.try_send(WorkerMessage::Wake) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => RequestDisposition::Queued,
            Err(mpsc::TrySendError::Disconnected(_)) => {
                state.pending.remove(&repo_path);
                state.maintenance_only.remove(&repo_path);
                state.background_pending.remove(&repo_path);
                RequestDisposition::QueueFull
            }
        }
    }

    /// Replace the one panel-owned history scope. The lease id makes cleanup
    /// ABA-safe: a late React effect teardown cannot cancel the newer scope.
    /// Paths removed from pure UI demand are pruned before they consume Git;
    /// watcher-owned maintenance remains intact.
    pub(crate) fn replace_demand_scope<I>(&self, demand_id: u64, repo_paths: I) -> bool
    where
        I: IntoIterator<Item = String>,
    {
        let new_scope: HashSet<String> = repo_paths.into_iter().collect();
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return false,
        };
        // IPCs execute on independent blocking tasks. A released request can
        // finish after its successor; monotonic ids make both late request and
        // late cleanup edges harmless.
        if demand_id == 0
            || demand_id <= state.released_through_demand_id
            || demand_id < state.latest_demand_id
        {
            return false;
        }
        state.latest_demand_id = demand_id;
        state.demand_id = Some(demand_id);
        state.demand_scope = new_scope;
        let demand_scope = state.demand_scope.clone();
        let background_pending = state.background_pending.clone();
        let maintenance_only = state.maintenance_only.clone();
        state.pending.retain(|path| {
            demand_scope.contains(path)
                || background_pending.contains(path)
                || maintenance_only.contains(path)
        });
        if let Some(active) = state.active.as_mut() {
            if !demand_scope.contains(&active.repo_path) {
                active.cancellation.cancel();
                active.mutation_gate.detach();
            } else if demand_scope.contains(&active.repo_path) && active.cancellation.is_cancelled()
            {
                // Search can close and reopen while a slow Git call is still
                // unwinding the cancelled lease. The path is technically
                // active, but that worker can no longer satisfy the new
                // demand; reserve its replacement before dropping the lock.
                active.rerun_requested = true;
            }
        }
        let active_path = state.active.as_ref().map(|active| active.repo_path.clone());
        for path in &demand_scope {
            if active_path.as_ref() != Some(path) {
                state.pending.insert(path.clone());
                if state.maintenance_only.contains(path) {
                    state.scan_after_maintenance.insert(path.clone());
                }
                if let Some(rerun) = state.detached_network.get_mut(path) {
                    *rerun = true;
                }
            }
        }
        if let Some(active) = state.active.as_mut() {
            if active.maintenance_only && demand_scope.contains(&active.repo_path) {
                active.rerun_requested = true;
            }
        }
        preempt_network_for_local_pending(&mut state);
        let should_wake = !state.pending.is_empty();
        drop(state);
        if should_wake {
            let _ = self.sender.try_send(WorkerMessage::Wake);
        }
        true
    }

    pub(crate) fn release_demand_scope(&self, demand_id: u64) -> bool {
        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(_) => return false,
        };
        if demand_id == 0 {
            return false;
        }
        state.released_through_demand_id = state.released_through_demand_id.max(demand_id);
        if state.demand_id.is_none_or(|current| current > demand_id) {
            return false;
        }
        state.demand_id = None;
        state.demand_scope.clear();
        let background_pending = state.background_pending.clone();
        let maintenance_only = state.maintenance_only.clone();
        state
            .pending
            .retain(|path| background_pending.contains(path) || maintenance_only.contains(path));
        if let Some(active) = state.active.as_mut() {
            if !active.background_owned {
                active.cancellation.cancel();
                active.mutation_gate.detach();
            }
        }
        true
    }

    pub(crate) fn cancel(&self, repo_path: &str) -> bool {
        let Ok(mut state) = self.state.lock() else {
            return false;
        };
        let removed_pending = state.pending.remove(repo_path);
        state.background_pending.remove(repo_path);
        state.maintenance_only.remove(repo_path);
        state.scan_after_maintenance.remove(repo_path);
        state.demand_scope.remove(repo_path);
        if let Some(active) = state.active.as_mut() {
            if active.repo_path == repo_path {
                active.background_owned = false;
                active.rerun_requested = false;
                active.cancellation.cancel();
                active.mutation_gate.detach();
                return true;
            }
        }
        removed_pending
    }

    /// A single consistent snapshot of queued + active paths. Status IPCs
    /// use this instead of locking once per repository, which keeps a
    /// 1000-repo catalog query O(1) in synchronization edges.
    pub(crate) fn pending_paths(&self) -> HashSet<String> {
        let Ok(state) = self.state.lock() else {
            return HashSet::new();
        };
        let mut paths = state
            .pending
            .iter()
            .filter(|path| !state.maintenance_only.contains(*path))
            .cloned()
            .collect::<HashSet<_>>();
        paths.extend(state.scan_after_maintenance.iter().cloned());
        if let Some(active) = &state.active {
            if !active.maintenance_only || active.rerun_requested {
                paths.insert(active.repo_path.clone());
            }
        }
        paths
    }
}

impl Drop for HistoryIndexer {
    fn drop(&mut self) {
        if let Ok(state) = self.state.lock() {
            if let Some(active) = &state.active {
                active.cancellation.cancel();
                active.mutation_gate.detach();
            }
        }
        self.shutdown.cancel();
        // Wake an idle blocking receiver without a periodic polling timer.
        // If the bounded queue is full the worker is necessarily runnable;
        // the shutdown token makes it stop at the next edge.
        let _ = self.sender.try_send(WorkerMessage::Shutdown);
        if let Ok(mut worker) = self.worker.lock() {
            if let Some(worker) = worker.take() {
                // Repository::open on a disconnected network path is an OS
                // call that cannot be cancelled. Never turn app shutdown into
                // a 10-30s join; a finished worker is reaped, while an active
                // one is safely detached and observes `shutdown` at its next
                // cancellation edge (the process may exit first).
                if worker.is_finished() {
                    let _ = worker.join();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_schema::test_support;
    use git2::{Repository, Signature, Time};
    use tempfile::TempDir;

    fn repo_with_commits(count: usize) -> (TempDir, Repository) {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        let tree_oid = repo.index().unwrap().write_tree().unwrap();
        let mut parent = None;
        for index in 0..count {
            let tree = repo.find_tree(tree_oid).unwrap();
            let signature = Signature::new(
                "Indexer",
                "indexer@example.com",
                &Time::new(1_700_100_000 + index as i64, 0),
            )
            .unwrap();
            let oid = match parent {
                Some(parent_oid) => {
                    let parent_commit = repo.find_commit(parent_oid).unwrap();
                    repo.commit(
                        Some("HEAD"),
                        &signature,
                        &signature,
                        &format!("indexed {index}"),
                        &tree,
                        &[&parent_commit],
                    )
                    .unwrap()
                }
                None => repo
                    .commit(
                        Some("HEAD"),
                        &signature,
                        &signature,
                        &format!("indexed {index}"),
                        &tree,
                        &[],
                    )
                    .unwrap(),
            };
            parent = Some(oid);
        }
        (dir, repo)
    }

    #[test]
    fn mutation_gate_linearizes_detach_after_an_inflight_write() {
        let gate = Arc::new(MutationGate::default());
        let token = CancellationToken::new();
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let write_gate = Arc::clone(&gate);
        let write_token = token.clone();
        let writer = std::thread::spawn(move || {
            write_gate
                .run(&write_token, || {
                    started_tx.send(()).unwrap();
                    release_rx.recv().unwrap();
                    Ok(7)
                })
                .unwrap()
        });
        started_rx.recv().unwrap();

        let detach_gate = Arc::clone(&gate);
        let (detached_tx, detached_rx) = std::sync::mpsc::channel();
        let detacher = std::thread::spawn(move || {
            detach_gate.detach();
            detached_tx.send(()).unwrap();
        });
        assert!(
            detached_rx
                .recv_timeout(std::time::Duration::from_millis(25))
                .is_err(),
            "detach waits only for the current bounded DB mutation"
        );
        release_tx.send(()).unwrap();
        assert_eq!(writer.join().unwrap(), Some(7));
        detached_rx.recv().unwrap();
        detacher.join().unwrap();
        assert_eq!(gate.run(&token, || Ok(9)).unwrap(), None);
    }

    fn append_commit(repo: &Repository, message: &str, timestamp: i64) {
        let tree_oid = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        let signature =
            Signature::new("Indexer", "indexer@example.com", &Time::new(timestamp, 0)).unwrap();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &[&parent],
        )
        .unwrap();
    }

    fn activate_repo(conn: &Connection, path: &Path) {
        let path = path.to_string_lossy();
        conn.execute(
            r#"
            INSERT INTO repos(
                path, name, discovered_at, last_seen_at, canonical_path,
                status, user_state, lifecycle_epoch
            ) VALUES (?1, 'repo', 1, 1, ?1, 'active', 'normal', 0)
            "#,
            rusqlite::params![path.as_ref()],
        )
        .unwrap();
    }

    #[test]
    fn visible_demand_preempts_background_queue() {
        let mut state = WorkerState::default();
        state
            .pending
            .extend(["a-background".to_string(), "z-visible".to_string()]);
        state.demand_scope.insert("z-visible".to_string());
        assert_eq!(next_pending(&state).as_deref(), Some("z-visible"));
        state.pending.remove("z-visible");
        assert_eq!(next_pending(&state).as_deref(), Some("a-background"));
    }

    #[test]
    fn local_history_publishes_before_unc_within_the_same_priority_class() {
        let mut state = WorkerState::default();
        state
            .pending
            .extend([r"\\server\repo".to_string(), r"C:\local\repo".to_string()]);
        state.demand_scope = state.pending.clone();
        assert_eq!(next_pending(&state).as_deref(), Some(r"C:\local\repo"));
    }

    #[test]
    fn demand_lease_cleanup_is_aba_safe_and_preserves_background_work() {
        let (sender, _receiver) = mpsc::sync_channel(1);
        let state = Arc::new(Mutex::new(WorkerState::default()));
        let indexer = HistoryIndexer {
            sender,
            state: Arc::clone(&state),
            shutdown: CancellationToken::new(),
            worker: Mutex::new(None),
        };

        assert!(indexer.replace_demand_scope(1, ["ui-old".into()]));
        assert_eq!(indexer.request("background"), RequestDisposition::Queued);
        assert!(indexer.replace_demand_scope(2, ["ui-current".into()]));
        {
            let state = state.lock().unwrap();
            assert!(!state.pending.contains("ui-old"));
            assert!(state.pending.contains("ui-current"));
            assert!(state.pending.contains("background"));
        }

        let cancellation = CancellationToken::new();
        {
            let mut state = state.lock().unwrap();
            state.pending.remove("ui-current");
            state.active = Some(ActiveJob {
                repo_path: "ui-current".into(),
                cancellation: cancellation.clone(),
                rerun_requested: false,
                background_owned: false,
                maintenance_only: false,
                mutation_gate: Arc::new(MutationGate::default()),
            });
        }
        assert!(!indexer.release_demand_scope(1));
        assert!(!cancellation.is_cancelled());
        assert!(indexer.release_demand_scope(2));
        assert!(cancellation.is_cancelled());
        assert!(indexer.replace_demand_scope(3, ["ui-current".into()]));
        assert!(
            state
                .lock()
                .unwrap()
                .active
                .as_ref()
                .is_some_and(|active| active.rerun_requested),
            "a reopened scope reserves a rerun while cancellation unwinds"
        );
        assert!(indexer.release_demand_scope(3));
        assert!(
            !indexer.replace_demand_scope(3, ["late-ui".into()]),
            "a request finishing after its cleanup must stay retired"
        );
        let state = state.lock().unwrap();
        assert!(state.pending.contains("background"));
        assert!(!state.pending.contains("late-ui"));
    }

    #[test]
    fn end_to_end_scan_only_completes_after_exhaustion() {
        let (dir, _repo) = repo_with_commits(17);
        let mut conn = test_support::v5_connection();
        activate_repo(&conn, dir.path());
        let outcome = run_full_scan(&mut conn, dir.path(), 4, &CancellationToken::new()).unwrap();
        let RunOutcome::Published { commits, .. } = outcome else {
            panic!("expected published scan");
        };
        assert_eq!(commits, 17);
        let coverage = history_store::coverage(&conn, &dir.path().to_string_lossy())
            .unwrap()
            .unwrap();
        assert_eq!(coverage.state, history_store::CoverageState::Complete);
        assert_eq!(coverage.commits_indexed, 17);
    }

    #[test]
    fn pre_cancelled_scan_never_publishes_complete() {
        let (dir, _repo) = repo_with_commits(4);
        let mut conn = test_support::v5_connection();
        activate_repo(&conn, dir.path());
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        let outcome = run_full_scan(&mut conn, dir.path(), 2, &cancellation).unwrap();
        assert_eq!(outcome, RunOutcome::Cancelled);
        assert!(
            history_store::coverage(&conn, &dir.path().to_string_lossy())
                .unwrap()
                .is_none(),
            "pre-cancellation must avoid both Git I/O and a disposable DB lease"
        );
    }

    #[test]
    fn repeat_scan_is_noop_and_fast_forward_walks_only_delta() {
        let (dir, repo) = repo_with_commits(8);
        let mut conn = test_support::v5_connection();
        activate_repo(&conn, dir.path());
        let first = run_full_scan(&mut conn, dir.path(), 4, &CancellationToken::new()).unwrap();
        let RunOutcome::Published { commits: 8, .. } = first else {
            panic!("expected initial full publication");
        };
        let first_coverage = history_store::coverage(&conn, &dir.path().to_string_lossy())
            .unwrap()
            .unwrap();
        history_store::drain_pending_cleanup(&conn, &dir.path().to_string_lossy()).unwrap();

        assert_eq!(
            run_full_scan(&mut conn, dir.path(), 4, &CancellationToken::new()).unwrap(),
            RunOutcome::AlreadyCurrent
        );

        append_commit(&repo, "fast forward", 1_700_200_000);
        let delta = run_full_scan(&mut conn, dir.path(), 4, &CancellationToken::new()).unwrap();
        assert!(matches!(
            delta,
            RunOutcome::Published {
                commits: 1,
                deleted: 0,
                ..
            }
        ));
        let after = history_store::coverage(&conn, &dir.path().to_string_lossy())
            .unwrap()
            .unwrap();
        assert_eq!(after.published_epoch, first_coverage.published_epoch);
        assert_eq!(after.commits_indexed, 9);
    }
}
