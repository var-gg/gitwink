// Watch each discovered repo's .git directory. Any change (a `git commit`,
// `git checkout`, branch update, fetch, etc.) bubbles up to a debounced
// refresh that re-reads that repo's recent commits and emits a
// `timeline://repo-fill` event the frontend already knows how to merge.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use tauri::{AppHandle, Emitter, Manager};

use crate::{
    cache, discovery_orchestrator, git, history_git, history_indexer, history_store,
    repo_exclusions,
};

/// How recent a commit has to be to enter the streamed payload. Matches the
/// panel's default time window so the merge in the frontend is meaningful.
const REFRESH_WINDOW_DAYS: i64 = 7;
/// Per-refresh revwalk cap. Must comfortably exceed any realistic agent
/// burst (a 30-commit rebase, a squash-merge train): the walk result is
/// also the live set `reconcile_repo_commits` trusts, and a capped walk
/// can only reconcile the span it actually covered — ghosts below the cap
/// boundary would linger.
const REFRESH_MAX_PER_REPO: usize = 100;
const DEBOUNCE_MS: u64 = 500;

/// Per-repo trailing-edge debounce state. `generation` ticks on every
/// event; the worker reads it after sleeping and only fires if no new
/// event has arrived. `pending` makes sure we have at most one worker
/// thread per repo, no matter how chatty the burst is.
#[derive(Default)]
struct DebounceEntry {
    generation: u64,
    pending: bool,
}

type DebounceMap = Arc<Mutex<HashMap<PathBuf, DebounceEntry>>>;

/// canonical watched dir → every repo path that depends on it. A linked
/// worktree shares its main repo's common gitdir (refs/remotes, packed-refs,
/// FETCH_HEAD live there), so one watched dir can serve several repos:
/// events fan out to ALL of them, and the underlying notify watch is only
/// dropped when the LAST dependent repo is removed.
type DirMap = HashMap<PathBuf, HashSet<PathBuf>>;

/// Observable result of a watcher mutation. Callers can log partial failures
/// while tests can assert ref-count behavior without depending on OS notify
/// delivery timing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WatcherMutation {
    pub links_added: usize,
    pub links_removed: usize,
    pub watches_added: usize,
    pub watches_removed: usize,
    pub failures: usize,
}

impl WatcherMutation {
    pub fn fully_applied(self) -> bool {
        self.failures == 0
    }
}

pub struct RepoWatcher {
    inner: Arc<Mutex<RecommendedWatcher>>,
    /// Keys are canonical .git dirs; values are repo_paths as the rest of
    /// the app sees them (un-canonicalized, matching discovery / cache).
    /// Notify reports events with the canonical form on Windows (`\\?\…`
    /// prefix), so we need this lookup to emit paths that match the cache
    /// and the all-mode rows.
    git_to_repo: Arc<Mutex<DirMap>>,
    debounce: DebounceMap,
    /// Serializes add/remove mutations without blocking the notify callback's
    /// map lookup. This closes the "second dependent observes a pending watch"
    /// race while preserving the no-nested-lock rule around OS watch calls.
    mutations: Mutex<()>,
}

impl RepoWatcher {
    pub fn start(app: AppHandle) -> anyhow::Result<Self> {
        let debounce: DebounceMap = Arc::new(Mutex::new(HashMap::new()));
        let debounce_for_event = debounce.clone();
        let app_for_event = app.clone();

        let git_to_repo: Arc<Mutex<DirMap>> = Arc::new(Mutex::new(HashMap::new()));
        let g2r = Arc::clone(&git_to_repo);

        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else {
                return;
            };
            // Cheap reject before any lock: only ref movement matters to a
            // commit timeline. A `git status` rewrites the index, a fetch
            // writes objects/**, gc repacks — none of those change what the
            // timeline shows, yet each used to schedule a full refresh
            // (libgit2 revwalk + two SQLite opens + a write transaction).
            // In the agent workflow this product targets — status/commit
            // loops running all day across N repos — this filter is the
            // difference between "refresh per git command" and "refresh per
            // actual ref movement".
            let relevant_paths = relevant_git_event_paths(&event.paths);
            if relevant_paths.is_empty() {
                return;
            }

            // Find the repo(s) this event belongs to. Worktrees keep their
            // gitdir at `main/.git/worktrees/<name>` and submodules at
            // `super/.git/modules/<name>`, so "first ancestor named .git"
            // matches the WRONG directory in those cases. Look up the
            // event's ancestors against the registered watched dirs and
            // take the longest-prefix match instead. A shared common dir
            // (main repo + its linked worktrees) fans out to every
            // dependent repo — a fetch in the main checkout must refresh
            // the worktree rows too.
            let mut repos = HashSet::new();
            for path in relevant_paths {
                if let Some(dependents) = snapshot_repos_for_event_path(path, &g2r) {
                    repos.extend(dependents);
                }
            }
            for repo_path in repos {
                schedule_refresh(app_for_event.clone(), repo_path, debounce_for_event.clone());
            }
        })?;

        Ok(Self {
            inner: Arc::new(Mutex::new(watcher)),
            git_to_repo,
            debounce,
            mutations: Mutex::new(()),
        })
    }

    pub(crate) fn event_epoch(&self, repo_path: &Path) -> u64 {
        self.debounce
            .lock()
            .ok()
            .and_then(|map| map.get(repo_path).map(|entry| entry.generation))
            .unwrap_or(0)
    }

    /// Pin one or more notify generations across a short cache mutation.
    /// Ref events block on this guard and then schedule a newer refresh, so an
    /// older walk can never commit after the newer event was already consumed.
    pub(crate) fn guard_event_epochs<'a>(
        &'a self,
        expected: &[(String, u64)],
    ) -> Option<RepoEventEpochGuard<'a>> {
        let guard = self.debounce.lock().ok()?;
        let unchanged = expected.iter().all(|(path, epoch)| {
            guard
                .get(Path::new(path))
                .map(|entry| entry.generation)
                .unwrap_or(0)
                == *epoch
        });
        unchanged.then_some(RepoEventEpochGuard { _guard: guard })
    }

    /// Pin the debounce map once and return the subset whose epochs still
    /// match. Fleet refill uses this instead of an all-or-nothing guard: one
    /// busy repository must not discard stable commits collected for every
    /// unrelated repository in the sweep.
    pub(crate) fn guard_matching_event_epochs<'a>(
        &'a self,
        expected: &[(String, u64)],
    ) -> Option<(RepoEventEpochGuard<'a>, HashSet<String>)> {
        let guard = self.debounce.lock().ok()?;
        let unchanged = expected
            .iter()
            .filter_map(|(path, epoch)| {
                (guard
                    .get(Path::new(path))
                    .map(|entry| entry.generation)
                    .unwrap_or(0)
                    == *epoch)
                    .then_some(path.clone())
            })
            .collect();
        Some((RepoEventEpochGuard { _guard: guard }, unchanged))
    }

    pub fn add(&self, repo_path: &Path) -> WatcherMutation {
        // Preserve the durable catalog spelling (legacy extended-UNC rows
        // included) because cache rows key by that exact string. Link helpers
        // compare normalized identity keys, so alternate spellings still
        // dedupe and detach correctly.
        let repo_path = repo_path.to_path_buf();
        let dirs = resolve_git_dirs_to_watch(&repo_path);
        if dirs.is_empty() {
            return WatcherMutation {
                failures: 1,
                ..WatcherMutation::default()
            };
        }
        // Git-directory resolution AND canonicalization can touch a network
        // filesystem. Keep both outside the process-wide mutation critical
        // section; the lock below is only the add/remove linearization point.
        let canonical_dirs = dirs
            .into_iter()
            .map(|dir| dir.canonicalize().unwrap_or(dir))
            .collect::<Vec<_>>();
        let _mutation = self
            .mutations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // Phase 1 (map lock only): record interest, collect dirs that need
        // a fresh OS watch. Phase 2 (watcher lock only): start them. The
        // two locks are NEVER held together: the notify backend can block
        // watch() on its event thread, and the event closure takes the map
        // lock — holding the map lock across watch() deadlocks the moment
        // an event fires while a repo is being added.
        let (to_watch, links_added) = {
            let Ok(mut map) = self.git_to_repo.lock() else {
                return WatcherMutation {
                    failures: 1,
                    ..WatcherMutation::default()
                };
            };
            add_repo_links(&mut map, &repo_path, canonical_dirs)
        };
        let mut outcome = WatcherMutation {
            links_added,
            ..WatcherMutation::default()
        };
        if to_watch.is_empty() {
            return outcome;
        }
        let mut failed: Vec<PathBuf> = Vec::new();
        {
            let Ok(mut w) = self.inner.lock() else {
                outcome.failures += to_watch.len();
                if let Ok(mut map) = self.git_to_repo.lock() {
                    rollback_failed_links(&mut map, &repo_path, &to_watch);
                }
                outcome.links_added = outcome.links_added.saturating_sub(to_watch.len());
                return outcome;
            };
            for canon in &to_watch {
                if w.watch(canon, RecursiveMode::Recursive).is_ok() {
                    outcome.watches_added += 1;
                } else {
                    failed.push(canon.clone());
                    outcome.failures += 1;
                }
            }
        }
        // Roll back map entries whose watch never started, so a later add
        // retries instead of assuming the dir is covered.
        if !failed.is_empty() {
            if let Ok(mut map) = self.git_to_repo.lock() {
                rollback_failed_links(&mut map, &repo_path, &failed);
            }
            outcome.links_added = outcome.links_added.saturating_sub(failed.len());
        }
        outcome
    }

    /// Detach `repo_path`'s interest in every watched dir — the watcher
    /// half of "exclude repo". Without this an excluded repo's .git stays
    /// subscribed and every new agent commit in it re-inserts rows the
    /// user asked to remove. Refcounted: a dir other repos still depend on
    /// (a common gitdir shared by a main repo and its worktrees) keeps its
    /// OS watch; only dirs left with zero dependents are unwatched.
    pub fn remove(&self, repo_path: &Path) -> WatcherMutation {
        let _mutation = self
            .mutations
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let repo_path = repo_path.to_path_buf();
        let (to_unwatch, links_removed) = {
            let Ok(mut map) = self.git_to_repo.lock() else {
                return WatcherMutation {
                    failures: 1,
                    ..WatcherMutation::default()
                };
            };
            remove_repo_links(&mut map, &repo_path)
        };
        let mut outcome = WatcherMutation {
            links_removed,
            ..WatcherMutation::default()
        };
        if to_unwatch.is_empty() {
            return outcome;
        }
        let Ok(mut w) = self.inner.lock() else {
            outcome.failures += to_unwatch.len();
            if let Ok(mut map) = self.git_to_repo.lock() {
                let restored = restore_repo_links(&mut map, &repo_path, &to_unwatch);
                outcome.links_removed = outcome.links_removed.saturating_sub(restored);
            }
            return outcome;
        };
        let mut rewatched = Vec::new();
        for dir in &to_unwatch {
            if w.unwatch(dir).is_ok() {
                outcome.watches_removed += 1;
            } else {
                // `unwatch` errors are not a proof that the OS subscription
                // disappeared. Reassert it; only restore the dependency map
                // when that re-watch succeeds. If both operations fail the
                // map must stay detached so a later add retries instead of
                // falsely assuming an OS subscription exists.
                if w.watch(dir, RecursiveMode::Recursive).is_ok() {
                    rewatched.push(dir.clone());
                }
                outcome.failures += 1;
            }
        }
        drop(w);
        if !rewatched.is_empty() {
            if let Ok(mut map) = self.git_to_repo.lock() {
                let restored = restore_repo_links(&mut map, &repo_path, &rewatched);
                outcome.links_removed = outcome.links_removed.saturating_sub(restored);
            }
        }
        outcome
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CatalogWatchTarget {
    path: String,
    lifecycle_epoch: i64,
    user_state: String,
    status: String,
    excluded_identity: bool,
}

impl CatalogWatchTarget {
    fn should_watch(&self) -> bool {
        self.user_state != "removed" && self.status == "active" && !self.excluded_identity
    }
}

fn catalog_watch_target(
    app: &AppHandle,
    raw_path: &str,
) -> anyhow::Result<Option<CatalogWatchTarget>> {
    let conn = cache::open(app)?;
    let Some(path) = discovery_orchestrator::registered_repo_identity(&conn, raw_path)? else {
        return Ok(None);
    };
    let (lifecycle_epoch, user_state, status) = conn.query_row(
        "SELECT lifecycle_epoch, user_state, status FROM repos WHERE canonical_path = ?1",
        [&path],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )?;
    let excluded_identity = discovery_orchestrator::repo_is_excluded(&conn, &path)?;
    Ok(Some(CatalogWatchTarget {
        path,
        lifecycle_epoch,
        user_state,
        status,
        excluded_identity,
    }))
}

/// Bring one normalized catalog identity's watcher subscription in line with
/// its durable lifecycle state without holding the global lifecycle mutex
/// across filesystem/notify I/O. The before/after snapshot includes the epoch,
/// so active -> excluded -> restored ABA transitions are retried against the
/// newest state. Filesystem/notify mutation is never performed while the
/// global lifecycle mutex is held.
pub(crate) fn reconcile_repo_registration(app: &AppHandle, raw_path: &str) {
    let Some(watcher) = app.try_state::<RepoWatcher>() else {
        return;
    };

    let mut churn_retries = 0usize;
    let mut detach_retries = 0usize;
    loop {
        let before = {
            let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
            match catalog_watch_target(app, raw_path) {
                Ok(target) => target,
                Err(error) => {
                    eprintln!("watcher catalog snapshot failed for {raw_path}: {error:#}");
                    return;
                }
            }
        };
        let outcome = match &before {
            Some(target) if target.should_watch() => watcher.add(Path::new(&target.path)),
            Some(target) => watcher.remove(Path::new(&target.path)),
            None => watcher.remove(Path::new(raw_path)),
        };
        let after = {
            let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
            match catalog_watch_target(app, raw_path) {
                Ok(target) => target,
                Err(error) => {
                    eprintln!("watcher catalog recheck failed for {raw_path}: {error:#}");
                    return;
                }
            }
        };
        if before == after {
            if outcome.fully_applied() {
                return;
            }
            // A failed active attach commonly means an offline network path.
            // Do not immediately repeat the same potentially long filesystem
            // probe three times; discovery or the next startup will retry.
            // Detach failures do retry because a successful compensating
            // re-watch deliberately restored the map and still needs removal.
            if before
                .as_ref()
                .is_some_and(CatalogWatchTarget::should_watch)
            {
                eprintln!("watcher attachment was partial for {raw_path}: {outcome:?}");
                return;
            }
            detach_retries += 1;
            if detach_retries == 3 {
                // The durable exclusion/missing gate still prevents cache
                // writes even if a poisoned or unavailable notify backend
                // cannot confirm physical unsubscription.
                eprintln!("watcher detachment was partial for {raw_path}: {outcome:?}");
                return;
            }
            continue;
        }
        detach_retries = 0;
        churn_retries += 1;
        if churn_retries == 8 {
            // This requires eight lifecycle changes to land during eight
            // short watcher mutations. Yield instead of falling back to slow
            // watcher I/O under the global mutex; the next pass still fences
            // against the newest epoch.
            eprintln!("watcher reconciliation is retrying lifecycle churn for {raw_path}");
            churn_retries = 0;
            std::thread::yield_now();
        }
    }
}

pub(crate) struct RepoEventEpochGuard<'a> {
    _guard: std::sync::MutexGuard<'a, HashMap<PathBuf, DebounceEntry>>,
}

fn add_repo_links(
    map: &mut DirMap,
    repo_path: &Path,
    dirs: impl IntoIterator<Item = PathBuf>,
) -> (Vec<PathBuf>, usize) {
    let mut to_watch = Vec::new();
    let mut links_added = 0;
    for dir in dirs {
        let set = map.entry(dir.clone()).or_default();
        if set.is_empty() {
            to_watch.push(dir);
        }
        let repo_key =
            discovery_orchestrator::repo_identity_match_key(&repo_path.to_string_lossy());
        let already_linked = set.iter().any(|existing| {
            discovery_orchestrator::repo_identity_match_key(&existing.to_string_lossy()) == repo_key
        });
        if !already_linked {
            set.insert(repo_path.to_path_buf());
            links_added += 1;
        }
    }
    (to_watch, links_added)
}

fn rollback_failed_links(map: &mut DirMap, repo_path: &Path, failed: &[PathBuf]) {
    for dir in failed {
        if let Some(set) = map.get_mut(dir) {
            let repo_key =
                discovery_orchestrator::repo_identity_match_key(&repo_path.to_string_lossy());
            set.retain(|existing| {
                discovery_orchestrator::repo_identity_match_key(&existing.to_string_lossy())
                    != repo_key
            });
            if set.is_empty() {
                map.remove(dir);
            }
        }
    }
}

fn restore_repo_links(map: &mut DirMap, repo_path: &Path, dirs: &[PathBuf]) -> usize {
    let repo_key = discovery_orchestrator::repo_identity_match_key(&repo_path.to_string_lossy());
    let mut restored = 0;
    for dir in dirs {
        let set = map.entry(dir.clone()).or_default();
        if !set.iter().any(|existing| {
            discovery_orchestrator::repo_identity_match_key(&existing.to_string_lossy()) == repo_key
        }) {
            set.insert(repo_path.to_path_buf());
            restored += 1;
        }
    }
    restored
}

fn remove_repo_links(map: &mut DirMap, repo_path: &Path) -> (Vec<PathBuf>, usize) {
    let mut to_unwatch = Vec::new();
    let mut links_removed = 0;
    let repo_key = discovery_orchestrator::repo_identity_match_key(&repo_path.to_string_lossy());
    map.retain(|dir, set| {
        let before = set.len();
        set.retain(|existing| {
            discovery_orchestrator::repo_identity_match_key(&existing.to_string_lossy()) != repo_key
        });
        links_removed += before - set.len();
        if set.is_empty() {
            to_unwatch.push(dir.clone());
            false
        } else {
            true
        }
    });
    (to_unwatch, links_removed)
}

/// Does this path under a watched git dir signal something the timeline
/// cares about? Ref movement shows up as `HEAD` / `packed-refs` /
/// `FETCH_HEAD` writes or anything under `refs/` or `logs/` (reflog).
/// Index churn from `git status`, object writes during fetch/gc,
/// `COMMIT_EDITMSG`, and `*.lock` staging files are all noise. A repo that
/// happens to live under a directory literally named `refs`/`logs` can
/// false-positive — that only costs a redundant refresh, never a miss.
fn is_relevant_git_event(path: &Path) -> bool {
    let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
    if name.ends_with(".lock") {
        return false;
    }
    if matches!(name, "HEAD" | "packed-refs" | "FETCH_HEAD") {
        return true;
    }
    path.components()
        .any(|c| matches!(c.as_os_str().to_str(), Some("refs") | Some("logs")))
}

fn relevant_git_event_paths(paths: &[PathBuf]) -> Vec<&Path> {
    paths
        .iter()
        .map(PathBuf::as_path)
        .filter(|path| is_relevant_git_event(path))
        .collect()
}

/// Resolve an inbound notify event without filesystem I/O. Callers may hold
/// the map lock while using this longest-ancestor lookup.
fn direct_repos_for_event_path<'m>(
    event_path: &Path,
    git_to_repo: &'m DirMap,
) -> Option<&'m HashSet<PathBuf>> {
    for ancestor in event_path.ancestors() {
        if let Some(set) = git_to_repo.get(ancestor) {
            return Some(set);
        }
    }
    None
}

/// Snapshot the dependent repos for an event. The usual exact lookup is a
/// tiny lock-only map walk. Canonicalization is a rare fallback for notify
/// backends that return a different path spelling, and can block for many
/// seconds on a disconnected UNC path, so every OS probe happens with the
/// shared registration map unlocked.
fn snapshot_repos_for_event_path(
    event_path: &Path,
    git_to_repo: &Arc<Mutex<DirMap>>,
) -> Option<Vec<PathBuf>> {
    if let Ok(map) = git_to_repo.lock() {
        if let Some(repos) = direct_repos_for_event_path(event_path, &map) {
            return Some(repos.iter().cloned().collect());
        }
    } else {
        return None;
    }

    let canonical_ancestors: Vec<PathBuf> = event_path
        .ancestors()
        .filter_map(|ancestor| ancestor.canonicalize().ok())
        .collect();
    let Ok(map) = git_to_repo.lock() else {
        return None;
    };
    // Recheck the direct spelling after the unlocked I/O window: watcher
    // registration may have changed while canonicalization was in flight.
    direct_repos_for_event_path(event_path, &map)
        .or_else(|| {
            canonical_ancestors
                .iter()
                .find_map(|ancestor| map.get(ancestor))
        })
        .map(|repos| repos.iter().cloned().collect())
}

/// Return the actual git directory for `repo_path`. For a normal clone this
/// is `<repo>/.git`. For a linked worktree, `.git` is a small text file
/// containing `gitdir: <path>` pointing at the worktree-specific git dir
/// under the main repo's `.git/worktrees/<name>/`.
fn resolve_git_dir(repo_path: &Path) -> Option<PathBuf> {
    let dotgit = repo_path.join(".git");
    if dotgit.is_dir() {
        return Some(dotgit);
    }
    if dotgit.is_file() {
        let text = std::fs::read_to_string(&dotgit).ok()?;
        let raw = text.strip_prefix("gitdir:")?.trim();
        let p = Path::new(raw);
        let git_dir = if p.is_absolute() {
            p.to_path_buf()
        } else {
            repo_path.join(p)
        };
        if git_dir.is_dir() {
            return Some(git_dir);
        }
    }
    None
}

/// Return every git dir we should subscribe to for `repo_path`. For a normal
/// clone that's just `<repo>/.git`. For a linked worktree it's the per-worktree
/// gitdir PLUS the main repo's common gitdir, because shared refs (notably
/// `refs/remotes/*`, `packed-refs`, and `FETCH_HEAD`) live in the common dir
/// rather than the per-worktree dir. Without watching both, a `git fetch` in
/// the main checkout never produces a fresh-update event for the worktree.
fn resolve_git_dirs_to_watch(repo_path: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let Some(git_dir) = resolve_git_dir(repo_path) else {
        return out;
    };
    out.push(git_dir.clone());

    if let Ok(text) = std::fs::read_to_string(git_dir.join("commondir")) {
        // Path-aware trim: only strip line terminators. `trim()` would also
        // remove leading/trailing whitespace from the path itself, which
        // (while rare) would be wrong.
        let raw = text.trim_end_matches(['\n', '\r']);
        let p = Path::new(raw);
        let common = if p.is_absolute() {
            p.to_path_buf()
        } else {
            git_dir.join(p)
        };
        if common != git_dir && common.is_dir() {
            out.push(common);
        }
    }

    out
}

/// Trailing-edge debounce: collapse a burst of events into a single refresh
/// fired DEBOUNCE_MS after the *last* event. At most one worker thread runs
/// per repo at a time — additional events during a quiet period just bump
/// the generation and the existing worker sleeps another round.
fn schedule_refresh(app: AppHandle, repo_path: PathBuf, debounce: DebounceMap) {
    let initial_gen = {
        let Ok(mut map) = debounce.lock() else {
            return;
        };
        let entry = map.entry(repo_path.clone()).or_default();
        entry.generation = entry.generation.wrapping_add(1);
        if entry.pending {
            return;
        }
        entry.pending = true;
        entry.generation
    };

    let debounce2 = debounce.clone();
    let repo_path2 = repo_path.clone();
    std::thread::spawn(move || {
        let mut tracked = initial_gen;
        loop {
            std::thread::sleep(Duration::from_millis(DEBOUNCE_MS));
            let Ok(mut map) = debounce2.lock() else {
                return;
            };
            let Some(entry) = map.get_mut(&repo_path2) else {
                return;
            };
            if entry.generation == tracked {
                entry.pending = false;
                drop(map);
                refresh_repo(&app, &repo_path2, tracked);
                return;
            }
            tracked = entry.generation;
        }
    });
}

fn refresh_repo(app: &AppHandle, repo_path: &Path, event_epoch: u64) {
    // A debounce worker can already be sleeping when Settings excludes the
    // repo. Serialize its final refresh edge with exclusion and fail closed on
    // the durable gate, otherwise that stale worker could repopulate rows
    // after exclusion removed them.
    let repo_identity = repo_path.to_string_lossy().into_owned();
    let (lifecycle_fence, history_tracked) = {
        let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
        let Ok(gate_conn) = cache::open(app) else {
            return;
        };
        if discovery_orchestrator::repo_is_excluded(&gate_conn, &repo_identity).unwrap_or(true) {
            return;
        }
        let Ok(Some(fence)) =
            discovery_orchestrator::capture_active_repo_lifecycle_fence(&gate_conn, &repo_identity)
        else {
            return;
        };
        let tracked = history_store::coverage(&gate_conn, &repo_identity)
            .ok()
            .flatten()
            .is_some_and(|coverage| coverage.published_epoch > 0);
        (fence, tracked)
    };

    let cutoff = unix_now() - REFRESH_WINDOW_DAYS * 86_400;
    let Ok(refs_before) = git::repo_refs_fingerprint(Path::new(&repo_identity), None) else {
        return;
    };
    // Err (repo unopenable — deleted, locked) bails out. Ok(empty) flows
    // through but is inert end to end: the upsert below skips an empty
    // batch, and reconcile_repo_commits refuses an empty live set — an
    // empty walk is ambiguous (skew-stuck frontier vs truly no commits)
    // and must never wipe a repo's cached window.
    let Ok(scan) =
        git::recent_commits_scan(Path::new(&repo_identity), REFRESH_MAX_PER_REPO, cutoff)
    else {
        return;
    };
    let can_reconcile = scan.can_reconcile();
    let commits = scan.rows;
    if git::repo_refs_fingerprint(Path::new(&repo_identity), None)
        .ok()
        .as_deref()
        != Some(refs_before.as_str())
    {
        // A newer debounce worker will observe the moved roots. Persisting
        // this older walk could delete its rows after its event already fired.
        return;
    }
    // Capture history roots only after the recent walk and prove the same
    // stable ref interval once more. Taking this before `refs_before` could
    // pair snapshot A with a successful recent scan of B and leave complete
    // history pinned to A forever.
    let observed_history_roots = history_tracked
        .then(|| history_git::snapshot_roots(Path::new(&repo_identity)).ok())
        .flatten();
    if git::repo_refs_fingerprint(Path::new(&repo_identity), None)
        .ok()
        .as_deref()
        != Some(refs_before.as_str())
    {
        return;
    }

    // Git walking is the expensive part and stays outside the global edge.
    // Reacquire + recheck immediately before cache mutation; exclusion either
    // wins first (this becomes a no-op) or follows us and purges our write.
    let Some(watcher_state) = app.try_state::<RepoWatcher>() else {
        return;
    };
    let expected_epoch = [(repo_identity.clone(), event_epoch)];
    let Some(_event_guard) = watcher_state.guard_event_epochs(&expected_epoch) else {
        return;
    };
    let _lifecycle = discovery_orchestrator::lock_repo_lifecycle();
    let Ok(mut conn) = cache::open(app) else {
        return;
    };
    if !discovery_orchestrator::repo_lifecycle_fence_matches(&conn, &lifecycle_fence)
        .unwrap_or(false)
    {
        return;
    }
    let _ =
        repo_exclusions::clear_repo_unavailable_observation_in(&conn, &repo_identity, unix_now());
    let upserted = if commits.is_empty() {
        None
    } else {
        cache::upsert_commits(&mut conn, &commits).ok()
    };
    // Reconcile: the walk above is the live set — cached rows in the
    // covered span that the repo no longer reaches were amended/rebased
    // away, and keeping them would show the user history that no longer
    // exists (the one lie a read-only glance tool must never tell).
    let reconciled = can_reconcile
        .then(|| cache::reconcile_repo_commits(&mut conn, &repo_identity, &commits, cutoff, false))
        .transpose()
        .ok()
        .flatten();
    let history_changed = if history_tracked {
        history_store::mark_stale(&conn, &repo_identity, observed_history_roots.as_ref())
            .unwrap_or(false)
    } else {
        false
    };
    drop(conn);

    if history_changed {
        if let Some(indexer) = app.try_state::<Arc<history_indexer::HistoryIndexer>>() {
            let _ = indexer.request(repo_identity.clone());
        }
    }

    let inserted = upserted.as_ref().map(|o| o.inserted).unwrap_or(0);
    let deleted = reconciled.as_ref().map(|o| o.deleted).unwrap_or(0);
    // The freshest generation either write produced (reconcile bumps after
    // upsert when both ran).
    let generation = reconciled
        .as_ref()
        .map(|o| o.generation)
        .filter(|g| *g > 0)
        .or(upserted.as_ref().map(|o| o.generation))
        .unwrap_or(0);

    // Emit policy: while the panel is visible, every refresh is worth a
    // re-pull (branch labels / remote badges on existing rows update too).
    // While it is hidden, only actual row changes matter — label-only
    // refreshes would just burn IPC + queries in an invisible webview.
    let panel_visible = app
        .get_webview_window("panel")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    // Ref movement is itself user-visible in single-repo mode (branch list,
    // upstream badge, selected non-origin remote), even when the bounded
    // recent-cache walk inserted/deleted no rows. Emit while visible so those
    // views re-read the refs; hidden panels refresh on their next summon.
    if generation == 0 && !panel_visible {
        return;
    }
    if !panel_visible && inserted == 0 && deleted == 0 {
        return;
    }
    let _ = app.emit(
        "timeline://invalidated",
        cache::TimelineInvalidated {
            generation,
            inserted,
            deleted,
            repo_path: repo_identity,
        },
    );
}

/// Run one lifecycle-fenced refresh without making a fleet caller wait for
/// it. Network-path orchestration uses this on its dedicated lane; watcher
/// events still enter through the debounced scheduler above.
pub(crate) fn refresh_repo_now(app: &AppHandle, repo_path: &Path) {
    let Some(watcher) = app.try_state::<RepoWatcher>() else {
        return;
    };
    let event_epoch = watcher.event_epoch(repo_path);
    refresh_repo(app, repo_path, event_epoch);
}

/// Deterministically feed a successful gitwink fetch into the same
/// trailing-edge lane as native filesystem notifications. If notify already
/// observed the ref transaction, both signals coalesce into one cache walk;
/// if it missed the transaction, this explicit edge still refreshes the
/// timeline and emits `timeline://invalidated`.
pub(crate) fn schedule_repo_refresh(app: &AppHandle, repo_path: &Path) {
    let Some(watcher) = app.try_state::<RepoWatcher>() else {
        return;
    };
    schedule_refresh(
        app.clone(),
        repo_path.to_path_buf(),
        Arc::clone(&watcher.debounce),
    );
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(
            s.replace(
                '/',
                std::path::MAIN_SEPARATOR_STR
                    .chars()
                    .next()
                    .unwrap()
                    .to_string()
                    .as_str(),
            ),
        )
    }

    fn one(repo: &Path) -> HashSet<PathBuf> {
        let mut s = HashSet::new();
        s.insert(repo.to_path_buf());
        s
    }

    #[test]
    fn event_path_under_normal_repo_gitdir_maps_to_repo() {
        let repo = p("/tmp/myrepo");
        let gitdir = p("/tmp/myrepo/.git");
        let mut map: DirMap = HashMap::new();
        map.insert(gitdir.clone(), one(&repo));

        let event = gitdir.join("HEAD");
        assert_eq!(direct_repos_for_event_path(&event, &map), Some(&one(&repo)));
    }

    #[test]
    fn event_path_under_worktree_gitdir_maps_to_worktree_repo() {
        // Worktree at /tmp/worktree, gitdir at /tmp/main/.git/worktrees/wt1.
        // The "first ancestor named .git" of an event under that path is
        // /tmp/main/.git, which is the WRONG repo. Longest-prefix lookup
        // should pick the registered worktree gitdir instead.
        let worktree_repo = p("/tmp/worktree");
        let worktree_gitdir = p("/tmp/main/.git/worktrees/wt1");
        let mut map: DirMap = HashMap::new();
        map.insert(worktree_gitdir.clone(), one(&worktree_repo));

        let event = worktree_gitdir.join("HEAD");
        assert_eq!(
            direct_repos_for_event_path(&event, &map),
            Some(&one(&worktree_repo))
        );
    }

    #[test]
    fn event_path_under_submodule_gitdir_maps_to_submodule_repo() {
        let sub_repo = p("/tmp/super/sub");
        let sub_gitdir = p("/tmp/super/.git/modules/sub");
        let mut map: DirMap = HashMap::new();
        map.insert(sub_gitdir.clone(), one(&sub_repo));

        let event = sub_gitdir.join("refs/heads/main");
        assert_eq!(
            direct_repos_for_event_path(&event, &map),
            Some(&one(&sub_repo))
        );
    }

    #[test]
    fn shared_common_dir_fans_out_to_every_dependent_repo() {
        // A main repo and its linked worktree both depend on the common
        // gitdir (refs/remotes, packed-refs, FETCH_HEAD live there): a
        // fetch in the main checkout must refresh the worktree rows too.
        let main_repo = p("/tmp/main");
        let worktree_repo = p("/tmp/worktree");
        let common = p("/tmp/main/.git");
        let mut set = HashSet::new();
        set.insert(main_repo.clone());
        set.insert(worktree_repo.clone());
        let mut map: DirMap = HashMap::new();
        map.insert(common.clone(), set.clone());

        let event = common.join("packed-refs");
        assert_eq!(direct_repos_for_event_path(&event, &map), Some(&set));
    }

    #[test]
    fn shared_dir_is_unwatched_only_after_last_repo_detaches() {
        let main_repo = p("/tmp/main");
        let worktree_repo = p("/tmp/worktree");
        let common = p("/tmp/main/.git");
        let mut map = DirMap::new();

        let (watch, added) = add_repo_links(&mut map, &main_repo, [common.clone()]);
        assert_eq!(watch, vec![common.clone()]);
        assert_eq!(added, 1);
        let (watch, added) = add_repo_links(&mut map, &worktree_repo, [common.clone()]);
        assert!(watch.is_empty());
        assert_eq!(added, 1);

        let (unwatch, removed) = remove_repo_links(&mut map, &main_repo);
        assert!(unwatch.is_empty());
        assert_eq!(removed, 1);
        assert_eq!(map.get(&common), Some(&one(&worktree_repo)));

        let (unwatch, removed) = remove_repo_links(&mut map, &worktree_repo);
        assert_eq!(unwatch, vec![common]);
        assert_eq!(removed, 1);
        assert!(map.is_empty());
    }

    #[test]
    fn duplicate_add_is_an_observable_noop() {
        let repo = p("/tmp/main");
        let gitdir = p("/tmp/main/.git");
        let mut map = DirMap::new();
        add_repo_links(&mut map, &repo, [gitdir.clone()]);

        let (watch, added) = add_repo_links(&mut map, &repo, [gitdir]);
        assert!(watch.is_empty());
        assert_eq!(added, 0);
    }

    #[test]
    fn alternate_windows_spelling_dedupes_and_detaches_same_repo() {
        let ordinary = PathBuf::from(r"C:\repos\main");
        let extended = PathBuf::from(r"\\?\C:\REPOS\main");
        let gitdir = p("/tmp/shared/.git");
        let mut map = DirMap::new();
        add_repo_links(&mut map, &ordinary, [gitdir.clone()]);

        let (watch, added) = add_repo_links(&mut map, &extended, [gitdir.clone()]);
        assert!(watch.is_empty());
        assert_eq!(added, 0);

        let (unwatch, removed) = remove_repo_links(&mut map, &extended);
        assert_eq!(unwatch, vec![gitdir]);
        assert_eq!(removed, 1);
        assert!(map.is_empty());
    }

    #[test]
    fn failed_shared_watch_rolls_back_only_the_new_dependency() {
        let main_repo = p("/tmp/main");
        let worktree_repo = p("/tmp/worktree");
        let common = p("/tmp/main/.git");
        let mut map = DirMap::new();
        map.insert(common.clone(), one(&main_repo));
        map.get_mut(&common).unwrap().insert(worktree_repo.clone());

        rollback_failed_links(&mut map, &worktree_repo, std::slice::from_ref(&common));

        assert_eq!(map.get(&common), Some(&one(&main_repo)));
    }

    #[test]
    fn failed_rewatch_keeps_map_detached_so_a_later_add_retries() {
        let repo = p("/tmp/main");
        let gitdir = p("/tmp/main/.git");
        let mut map = DirMap::new();
        add_repo_links(&mut map, &repo, [gitdir.clone()]);

        let (to_unwatch, removed) = remove_repo_links(&mut map, &repo);
        assert_eq!(to_unwatch, vec![gitdir.clone()]);
        assert_eq!(removed, 1);
        // Models unwatch() + compensating watch() both failing: remove() must
        // not put the logical dependency back into the map.
        assert!(map.is_empty());

        let (to_watch, added) = add_repo_links(&mut map, &repo, [gitdir.clone()]);
        assert_eq!(to_watch, vec![gitdir]);
        assert_eq!(added, 1);
    }

    #[test]
    fn confirmed_rewatch_restores_map_and_prevents_duplicate_watch() {
        let repo = p("/tmp/main");
        let gitdir = p("/tmp/main/.git");
        let mut map = DirMap::new();
        add_repo_links(&mut map, &repo, [gitdir.clone()]);
        remove_repo_links(&mut map, &repo);

        assert_eq!(
            restore_repo_links(&mut map, &repo, std::slice::from_ref(&gitdir)),
            1
        );
        let (to_watch, added) = add_repo_links(&mut map, &repo, [gitdir]);
        assert!(to_watch.is_empty());
        assert_eq!(added, 0);
    }

    #[test]
    fn catalog_watch_policy_is_active_and_exclusion_gated() {
        let target = |user_state: &str, status: &str, excluded_identity| CatalogWatchTarget {
            path: "/tmp/main".to_string(),
            lifecycle_epoch: 1,
            user_state: user_state.to_string(),
            status: status.to_string(),
            excluded_identity,
        };

        assert!(target("normal", "active", false).should_watch());
        assert!(!target("normal", "missing", false).should_watch());
        assert!(!target("removed", "active", false).should_watch());
        assert!(!target("normal", "active", true).should_watch());
    }

    #[test]
    fn unregistered_event_returns_none() {
        let map: DirMap = HashMap::new();
        let event = p("/tmp/random/path");
        assert_eq!(direct_repos_for_event_path(&event, &map), None);
    }

    #[test]
    fn ref_movement_events_are_relevant() {
        for path in [
            "/r/.git/HEAD",
            "/r/.git/packed-refs",
            "/r/.git/FETCH_HEAD",
            "/r/.git/refs/heads/main",
            "/r/.git/refs/remotes/origin/main",
            "/r/.git/logs/HEAD",
            "/main/.git/worktrees/wt1/HEAD",
        ] {
            assert!(is_relevant_git_event(&p(path)), "{path} should refresh");
        }
    }

    #[test]
    fn rename_event_keeps_relevant_target_when_lock_path_comes_first() {
        let paths = vec![
            p("/r/.git/refs/heads/main.lock"),
            p("/r/.git/refs/heads/main"),
        ];
        assert_eq!(
            relevant_git_event_paths(&paths),
            vec![paths[1].as_path()],
            "notify rename-both must not be discarded because its source is a lock"
        );
    }

    #[test]
    fn index_and_object_churn_is_filtered_out() {
        for path in [
            "/r/.git/index",
            "/r/.git/index.lock",
            "/r/.git/COMMIT_EDITMSG",
            "/r/.git/MERGE_MSG",
            "/r/.git/objects/ab/cdef0123456789",
            "/r/.git/objects/pack/pack-deadbeef.pack",
            "/r/.git/refs/heads/main.lock",
            "/r/.git/packed-refs.lock",
        ] {
            assert!(!is_relevant_git_event(&p(path)), "{path} should be ignored");
        }
    }
}
