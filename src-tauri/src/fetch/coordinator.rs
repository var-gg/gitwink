use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use super::{FetchOutcome, MAX_INFLIGHT};

#[derive(Default)]
struct CoordinatorState {
    last_attempt: HashMap<PathBuf, Instant>,
    inflight: HashMap<PathBuf, Arc<FetchFlight>>,
}

/// Managed Tauri state for fetch cooldown and concurrency. Paths are
/// canonicalized so aliases, linked worktrees, and Windows case variants join
/// the same flight.
#[derive(Default)]
pub struct FetchCoordinator {
    state: Arc<Mutex<CoordinatorState>>,
}

/// One shared terminal result. A manual refresh may wait on the flight started
/// by panel-open auto-fetch instead of spawning a competing Git process.
#[derive(Default)]
pub struct FetchFlight {
    result: Mutex<Option<FetchOutcome>>,
    ready: Condvar,
}

impl FetchFlight {
    pub fn wait_timeout(&self, timeout: Duration) -> Option<FetchOutcome> {
        let result = match self.result.lock() {
            Ok(result) => result,
            Err(poisoned) => poisoned.into_inner(),
        };
        let result = match self
            .ready
            .wait_timeout_while(result, timeout, |result| result.is_none())
        {
            Ok((result, _)) => result,
            Err(poisoned) => poisoned.into_inner().0,
        };
        *result
    }

    fn publish(&self, outcome: FetchOutcome) {
        let mut result = match self.result.lock() {
            Ok(result) => result,
            Err(poisoned) => poisoned.into_inner(),
        };
        if result.is_none() {
            *result = Some(outcome);
            self.ready.notify_all();
        }
    }
}

/// Owns a coordinator slot for exactly one Git process. Dropping an unfinished
/// guard publishes `failed`, so a panic or task cancellation cannot strand a
/// waiter or permanently consume the repository's single-flight slot.
pub struct FetchGuard {
    key: PathBuf,
    flight: Arc<FetchFlight>,
    state: Arc<Mutex<CoordinatorState>>,
    completed: bool,
}

impl FetchGuard {
    pub fn finish(mut self, outcome: FetchOutcome) -> FetchOutcome {
        self.complete(outcome);
        outcome
    }

    fn complete(&mut self, outcome: FetchOutcome) {
        if self.completed {
            return;
        }
        self.completed = true;
        self.flight.publish(outcome);

        let mut state = match self.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        let owns_slot = state
            .inflight
            .get(&self.key)
            .is_some_and(|current| Arc::ptr_eq(current, &self.flight));
        if owns_slot {
            state.inflight.remove(&self.key);
        }
    }
}

impl Drop for FetchGuard {
    fn drop(&mut self) {
        self.complete(FetchOutcome::Failed);
    }
}

pub enum ManualFetchClaim {
    Start(FetchGuard),
    Join(Arc<FetchFlight>),
    Busy,
}

impl FetchCoordinator {
    /// Claim an automatic fetch. Existing same-repository work, cooldown, and
    /// a full global cap are all silent skips for the fire-and-forget path.
    pub fn try_claim(&self, repo: &Path, interval: Duration) -> Option<FetchGuard> {
        let key = canonical_key(repo);
        let Ok(mut state) = self.state.lock() else {
            return None;
        };
        if state.inflight.contains_key(&key) {
            return None;
        }

        let now = Instant::now();
        if state
            .last_attempt
            .get(&key)
            .is_some_and(|last| now.duration_since(*last) < interval)
        {
            return None;
        }
        if state.inflight.len() >= MAX_INFLIGHT {
            return None;
        }

        Some(start_flight(&mut state, Arc::clone(&self.state), key, now))
    }

    /// Claim an explicit refresh. It bypasses cooldown, but joins an existing
    /// same-repository flight before considering the global cap. This ordering
    /// is what removes the auto/manual ref-lock race.
    pub fn claim_forced(&self, repo: &Path) -> ManualFetchClaim {
        let key = canonical_key(repo);
        let Ok(mut state) = self.state.lock() else {
            return ManualFetchClaim::Busy;
        };

        if let Some(flight) = state.inflight.get(&key) {
            return ManualFetchClaim::Join(Arc::clone(flight));
        }
        if state.inflight.len() >= MAX_INFLIGHT {
            return ManualFetchClaim::Busy;
        }

        ManualFetchClaim::Start(start_flight(
            &mut state,
            Arc::clone(&self.state),
            key,
            Instant::now(),
        ))
    }
}

fn canonical_key(repo: &Path) -> PathBuf {
    repository_common_dir(repo)
        .and_then(|path| std::fs::canonicalize(&path).ok().or(Some(path)))
        .unwrap_or_else(|| std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf()))
}

/// Fetch writes shared refs and `FETCH_HEAD`, so linked worktrees must key on
/// their common Git directory rather than their distinct working directories.
/// Resolving the repository also makes a nested path join its repository root.
fn repository_common_dir(repo: &Path) -> Option<PathBuf> {
    let repository = git2::Repository::discover(repo).ok()?;
    let git_dir = repository.path();
    let common_dir = std::fs::read_to_string(git_dir.join("commondir"))
        .ok()
        .and_then(|text| {
            let raw = text.trim_end_matches(['\n', '\r']);
            if raw.is_empty() {
                return None;
            }
            let path = Path::new(raw);
            Some(if path.is_absolute() {
                path.to_path_buf()
            } else {
                git_dir.join(path)
            })
        })
        .unwrap_or_else(|| git_dir.to_path_buf());
    Some(common_dir)
}

fn start_flight(
    state: &mut CoordinatorState,
    shared_state: Arc<Mutex<CoordinatorState>>,
    key: PathBuf,
    now: Instant,
) -> FetchGuard {
    let flight = Arc::new(FetchFlight::default());
    state.last_attempt.insert(key.clone(), now);
    state.inflight.insert(key.clone(), Arc::clone(&flight));
    FetchGuard {
        key,
        flight,
        state: shared_state,
        completed: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    fn repository_with_initial_commit(path: &Path) -> git2::Repository {
        let repository = git2::Repository::init(path).unwrap();
        let mut index = repository.index().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repository.find_tree(tree_id).unwrap();
        let signature = git2::Signature::now("gitwink", "tests@gitwink.local").unwrap();
        repository
            .commit(Some("HEAD"), &signature, &signature, "initial", &tree, &[])
            .unwrap();
        drop(tree);
        repository
    }

    #[test]
    fn forced_claim_joins_same_repository_flight() {
        let dir = tempfile::tempdir().unwrap();
        let coordinator = FetchCoordinator::default();
        let guard = coordinator
            .try_claim(dir.path(), Duration::ZERO)
            .expect("auto flight should start");

        let ManualFetchClaim::Join(flight) = coordinator.claim_forced(dir.path()) else {
            panic!("manual fetch must join the automatic flight");
        };
        guard.finish(FetchOutcome::Ok);
        assert_eq!(
            flight.wait_timeout(Duration::from_millis(10)),
            Some(FetchOutcome::Ok)
        );
    }

    #[test]
    fn joined_waiter_is_notified_when_owner_finishes() {
        let dir = tempfile::tempdir().unwrap();
        let coordinator = FetchCoordinator::default();
        let guard = coordinator
            .try_claim(dir.path(), Duration::ZERO)
            .expect("flight should start");
        let ManualFetchClaim::Join(flight) = coordinator.claim_forced(dir.path()) else {
            panic!("manual fetch must join");
        };

        let waiter = thread::spawn(move || flight.wait_timeout(Duration::from_secs(1)));
        guard.finish(FetchOutcome::Network);
        assert_eq!(waiter.join().unwrap(), Some(FetchOutcome::Network));
    }

    #[test]
    fn different_repositories_still_respect_global_cap() {
        let root = tempfile::tempdir().unwrap();
        let repo_a = root.path().join("a");
        let repo_b = root.path().join("b");
        let repo_c = root.path().join("c");
        std::fs::create_dir_all(&repo_a).unwrap();
        std::fs::create_dir_all(&repo_b).unwrap();
        std::fs::create_dir_all(&repo_c).unwrap();

        let coordinator = FetchCoordinator::default();
        let _a = coordinator
            .try_claim(&repo_a, Duration::ZERO)
            .expect("first slot");
        let _b = coordinator
            .try_claim(&repo_b, Duration::ZERO)
            .expect("second slot");
        assert!(matches!(
            coordinator.claim_forced(&repo_c),
            ManualFetchClaim::Busy
        ));
    }

    #[test]
    fn manual_start_bypasses_completed_auto_cooldown() {
        let dir = tempfile::tempdir().unwrap();
        let coordinator = FetchCoordinator::default();
        coordinator
            .try_claim(dir.path(), Duration::from_secs(60))
            .expect("auto flight")
            .finish(FetchOutcome::Ok);

        assert!(coordinator
            .try_claim(dir.path(), Duration::from_secs(60))
            .is_none());
        assert!(matches!(
            coordinator.claim_forced(dir.path()),
            ManualFetchClaim::Start(_)
        ));
    }

    #[test]
    fn linked_worktrees_join_the_same_repository_flight() {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        let linked = root.path().join("linked");
        let repository = repository_with_initial_commit(&main);
        repository.worktree("linked", &linked, None).unwrap();

        let coordinator = FetchCoordinator::default();
        let guard = coordinator
            .try_claim(&main, Duration::ZERO)
            .expect("main worktree should start a flight");
        let ManualFetchClaim::Join(flight) = coordinator.claim_forced(&linked) else {
            panic!("linked worktree must join the shared ref flight");
        };

        guard.finish(FetchOutcome::Ok);
        assert_eq!(
            flight.wait_timeout(Duration::from_millis(10)),
            Some(FetchOutcome::Ok)
        );
    }
}
