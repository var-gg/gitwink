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

use crate::{cache, git};

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

pub struct RepoWatcher {
    inner: Arc<Mutex<RecommendedWatcher>>,
    /// Keys are canonical .git dirs; values are repo_paths as the rest of
    /// the app sees them (un-canonicalized, matching discovery / cache).
    /// Notify reports events with the canonical form on Windows (`\\?\…`
    /// prefix), so we need this lookup to emit paths that match the cache
    /// and the all-mode rows.
    git_to_repo: Arc<Mutex<DirMap>>,
}

impl RepoWatcher {
    pub fn start(app: AppHandle) -> anyhow::Result<Self> {
        let debounce: DebounceMap = Arc::new(Mutex::new(HashMap::new()));
        let app_for_event = app.clone();

        let git_to_repo: Arc<Mutex<DirMap>> = Arc::new(Mutex::new(HashMap::new()));
        let g2r = Arc::clone(&git_to_repo);

        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else {
                return;
            };
            let Some(path) = event.paths.first() else {
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
            if !is_relevant_git_event(path) {
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
            let repos: Vec<PathBuf> = {
                let Ok(map) = g2r.lock() else {
                    return;
                };
                match repos_for_event_path(path, &map) {
                    Some(set) => set.iter().cloned().collect(),
                    None => return,
                }
            };
            for repo_path in repos {
                schedule_refresh(app_for_event.clone(), repo_path, debounce.clone());
            }
        })?;

        Ok(Self {
            inner: Arc::new(Mutex::new(watcher)),
            git_to_repo,
        })
    }

    pub fn add(&self, repo_path: &Path) {
        let dirs = resolve_git_dirs_to_watch(repo_path);
        if dirs.is_empty() {
            return;
        }
        // Phase 1 (map lock only): record interest, collect dirs that need
        // a fresh OS watch. Phase 2 (watcher lock only): start them. The
        // two locks are NEVER held together: the notify backend can block
        // watch() on its event thread, and the event closure takes the map
        // lock — holding the map lock across watch() deadlocks the moment
        // an event fires while a repo is being added.
        let mut to_watch: Vec<PathBuf> = Vec::new();
        {
            let Ok(mut map) = self.git_to_repo.lock() else {
                return;
            };
            for dir in dirs {
                let canon = dir.canonicalize().unwrap_or_else(|_| dir.clone());
                let set = map.entry(canon.clone()).or_default();
                if set.is_empty() {
                    to_watch.push(canon);
                }
                set.insert(repo_path.to_path_buf());
            }
        }
        if to_watch.is_empty() {
            return;
        }
        let mut failed: Vec<PathBuf> = Vec::new();
        {
            let Ok(mut w) = self.inner.lock() else {
                return;
            };
            for canon in &to_watch {
                if w.watch(canon, RecursiveMode::Recursive).is_err() {
                    failed.push(canon.clone());
                }
            }
        }
        // Roll back map entries whose watch never started, so a later add
        // retries instead of assuming the dir is covered.
        if !failed.is_empty() {
            if let Ok(mut map) = self.git_to_repo.lock() {
                for canon in failed {
                    if let Some(set) = map.get_mut(&canon) {
                        set.remove(repo_path);
                        if set.is_empty() {
                            map.remove(&canon);
                        }
                    }
                }
            }
        }
    }

    /// Detach `repo_path`'s interest in every watched dir — the watcher
    /// half of "hide repo". Without this a hidden repo's .git stays
    /// subscribed and every new agent commit in it re-inserts rows the
    /// user asked to remove. Refcounted: a dir other repos still depend on
    /// (a common gitdir shared by a main repo and its worktrees) keeps its
    /// OS watch; only dirs left with zero dependents are unwatched.
    pub fn remove(&self, repo_path: &Path) {
        let mut to_unwatch: Vec<PathBuf> = Vec::new();
        {
            let Ok(mut map) = self.git_to_repo.lock() else {
                return;
            };
            map.retain(|dir, set| {
                set.remove(repo_path);
                if set.is_empty() {
                    to_unwatch.push(dir.clone());
                    false
                } else {
                    true
                }
            });
        }
        if to_unwatch.is_empty() {
            return;
        }
        let Ok(mut w) = self.inner.lock() else {
            return;
        };
        for dir in to_unwatch {
            let _ = w.unwatch(&dir);
        }
    }
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

/// Resolve an inbound notify event back to the registered repos that
/// depend on it, by finding the longest-ancestor that's a key in
/// `git_to_repo`. Falls back to per-ancestor canonicalize for
/// non-canonical input.
fn repos_for_event_path<'m>(
    event_path: &Path,
    git_to_repo: &'m DirMap,
) -> Option<&'m HashSet<PathBuf>> {
    for ancestor in event_path.ancestors() {
        if let Some(set) = git_to_repo.get(ancestor) {
            return Some(set);
        }
        if let Ok(canon) = ancestor.canonicalize() {
            if let Some(set) = git_to_repo.get(&canon) {
                return Some(set);
            }
        }
    }
    None
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
                refresh_repo(&app, &repo_path2);
                return;
            }
            tracked = entry.generation;
        }
    });
}

fn refresh_repo(app: &AppHandle, repo_path: &Path) {
    let cutoff = unix_now() - REFRESH_WINDOW_DAYS * 86_400;
    // Err (repo unopenable — deleted, locked) bails out. Ok(empty) flows
    // through but is inert end to end: the upsert below skips an empty
    // batch, and reconcile_repo_commits refuses an empty live set — an
    // empty walk is ambiguous (skew-stuck frontier vs truly no commits)
    // and must never wipe a repo's cached window.
    let Ok(commits) = git::recent_commits(repo_path, REFRESH_MAX_PER_REPO, cutoff) else {
        return;
    };

    let Ok(mut conn) = cache::open(app) else {
        return;
    };
    let upserted = if commits.is_empty() {
        None
    } else {
        cache::upsert_commits(&mut conn, &commits).ok()
    };
    // Reconcile: the walk above is the live set — cached rows in the
    // covered span that the repo no longer reaches were amended/rebased
    // away, and keeping them would show the user history that no longer
    // exists (the one lie a read-only glance tool must never tell).
    let walk_capped = commits.len() >= REFRESH_MAX_PER_REPO;
    let reconciled =
        cache::reconcile_repo_commits(&mut conn, &repo_path.to_string_lossy(), &commits, cutoff, walk_capped)
            .ok();
    drop(conn);

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
    if generation == 0 {
        return;
    }
    let panel_visible = app
        .get_webview_window("panel")
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    if !panel_visible && inserted == 0 && deleted == 0 {
        return;
    }
    let _ = app.emit(
        "timeline://invalidated",
        cache::TimelineInvalidated {
            generation,
            inserted,
            deleted,
            repo_path: repo_path.to_string_lossy().into_owned(),
        },
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
        PathBuf::from(s.replace('/', std::path::MAIN_SEPARATOR_STR.chars().next().unwrap().to_string().as_str()))
    }

    fn one(repo: &PathBuf) -> HashSet<PathBuf> {
        let mut s = HashSet::new();
        s.insert(repo.clone());
        s
    }

    #[test]
    fn event_path_under_normal_repo_gitdir_maps_to_repo() {
        let repo = p("/tmp/myrepo");
        let gitdir = p("/tmp/myrepo/.git");
        let mut map: DirMap = HashMap::new();
        map.insert(gitdir.clone(), one(&repo));

        let event = gitdir.join("HEAD");
        assert_eq!(repos_for_event_path(&event, &map), Some(&one(&repo)));
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
            repos_for_event_path(&event, &map),
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
        assert_eq!(repos_for_event_path(&event, &map), Some(&one(&sub_repo)));
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
        assert_eq!(repos_for_event_path(&event, &map), Some(&set));
    }

    #[test]
    fn unregistered_event_returns_none() {
        let map: DirMap = HashMap::new();
        let event = p("/tmp/random/path");
        assert_eq!(repos_for_event_path(&event, &map), None);
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
