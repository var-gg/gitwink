// Watch each discovered repo's .git directory. Any change (a `git commit`,
// `git checkout`, branch update, fetch, etc.) bubbles up to a debounced
// refresh that re-reads that repo's recent commits and emits a
// `timeline://repo-fill` event the frontend already knows how to merge.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::{cache, git};

/// How recent a commit has to be to enter the streamed payload. Matches the
/// panel's default time window so the merge in the frontend is meaningful.
const REFRESH_WINDOW_DAYS: i64 = 7;
const REFRESH_MAX_PER_REPO: usize = 10;
const DEBOUNCE_MS: u64 = 500;

#[derive(Serialize, Clone)]
struct RepoFillPayload {
    commits: Vec<git::CommitSummary>,
    fresh: bool,
}

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

pub struct RepoWatcher {
    inner: Arc<Mutex<RecommendedWatcher>>,
    /// canonical .git dir → repo_path as the rest of the app sees it
    /// (un-canonicalized, matches what discovery / cache use). Notify
    /// reports events with the canonical form on Windows (`\\?\…` prefix),
    /// so we need this lookup to emit a path that matches the cache and
    /// the all-mode rows.
    git_to_repo: Arc<Mutex<HashMap<PathBuf, PathBuf>>>,
}

impl RepoWatcher {
    pub fn start(app: AppHandle) -> anyhow::Result<Self> {
        let debounce: DebounceMap = Arc::new(Mutex::new(HashMap::new()));
        let app_for_event = app.clone();

        let git_to_repo: Arc<Mutex<HashMap<PathBuf, PathBuf>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let g2r = Arc::clone(&git_to_repo);

        let watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            let Ok(event) = res else {
                return;
            };
            let Some(path) = event.paths.first() else {
                return;
            };

            // Find the repo this event belongs to. Worktrees keep their
            // gitdir at `main/.git/worktrees/<name>` and submodules at
            // `super/.git/modules/<name>`, so "first ancestor named .git"
            // matches the WRONG directory in those cases. Look up the
            // event's ancestors against the registered watched dirs and
            // take the longest-prefix match instead.
            let Ok(map) = g2r.lock() else {
                return;
            };
            let Some(repo_path) = repo_for_event_path(path, &map) else {
                return;
            };
            drop(map);

            schedule_refresh(app_for_event.clone(), repo_path, debounce.clone());
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
        let Ok(mut map) = self.git_to_repo.lock() else {
            return;
        };
        let Ok(mut w) = self.inner.lock() else {
            return;
        };
        for dir in dirs {
            let canon = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            if map.contains_key(&canon) {
                continue;
            }
            if w.watch(&canon, RecursiveMode::Recursive).is_ok() {
                map.insert(canon, repo_path.to_path_buf());
            }
        }
    }
}

/// Resolve an inbound notify event back to one of our registered repos by
/// finding the longest-ancestor that's a key in `git_to_repo`. Falls back
/// to per-ancestor canonicalize for non-canonical input.
fn repo_for_event_path(
    event_path: &Path,
    git_to_repo: &HashMap<PathBuf, PathBuf>,
) -> Option<PathBuf> {
    for ancestor in event_path.ancestors() {
        if let Some(repo) = git_to_repo.get(ancestor) {
            return Some(repo.clone());
        }
        if let Ok(canon) = ancestor.canonicalize() {
            if let Some(repo) = git_to_repo.get(&canon) {
                return Some(repo.clone());
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
    let Ok(commits) = git::recent_commits(repo_path, REFRESH_MAX_PER_REPO, cutoff) else {
        return;
    };
    if commits.is_empty() {
        return;
    }

    if let Ok(mut conn) = cache::open(app) {
        let _ = cache::upsert_commits(&mut conn, &commits);
    }

    let _ = app.emit(
        "timeline://repo-fill",
        RepoFillPayload { commits, fresh: true },
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

    #[test]
    fn event_path_under_normal_repo_gitdir_maps_to_repo() {
        let repo = p("/tmp/myrepo");
        let gitdir = p("/tmp/myrepo/.git");
        let mut map = HashMap::new();
        map.insert(gitdir.clone(), repo.clone());

        let event = gitdir.join("HEAD");
        assert_eq!(repo_for_event_path(&event, &map), Some(repo));
    }

    #[test]
    fn event_path_under_worktree_gitdir_maps_to_worktree_repo() {
        // Worktree at /tmp/worktree, gitdir at /tmp/main/.git/worktrees/wt1.
        // The "first ancestor named .git" of an event under that path is
        // /tmp/main/.git, which is the WRONG repo. Longest-prefix lookup
        // should pick the registered worktree gitdir instead.
        let worktree_repo = p("/tmp/worktree");
        let worktree_gitdir = p("/tmp/main/.git/worktrees/wt1");
        let mut map = HashMap::new();
        map.insert(worktree_gitdir.clone(), worktree_repo.clone());

        let event = worktree_gitdir.join("HEAD");
        assert_eq!(repo_for_event_path(&event, &map), Some(worktree_repo));
    }

    #[test]
    fn event_path_under_submodule_gitdir_maps_to_submodule_repo() {
        let sub_repo = p("/tmp/super/sub");
        let sub_gitdir = p("/tmp/super/.git/modules/sub");
        let mut map = HashMap::new();
        map.insert(sub_gitdir.clone(), sub_repo.clone());

        let event = sub_gitdir.join("refs/heads/main");
        assert_eq!(repo_for_event_path(&event, &map), Some(sub_repo));
    }

    #[test]
    fn unregistered_event_returns_none() {
        let map: HashMap<PathBuf, PathBuf> = HashMap::new();
        let event = p("/tmp/random/path");
        assert_eq!(repo_for_event_path(&event, &map), None);
    }
}
