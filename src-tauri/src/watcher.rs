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

            // Walk up to the .git directory we're watching, then look it up
            // in the map to get the repo_path *in the form the rest of the
            // app uses* (cache rows, discovery output, panel state).
            let Some(git_dir) = path.ancestors().find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n == ".git")
                    .unwrap_or(false)
            }) else {
                return;
            };
            let Ok(map) = g2r.lock() else {
                return;
            };
            let Some(repo_path) = map.get(git_dir).cloned() else {
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
        let Some(git_dir) = resolve_git_dir(repo_path) else {
            return;
        };
        let canon = git_dir.canonicalize().unwrap_or_else(|_| git_dir.clone());
        let Ok(mut map) = self.git_to_repo.lock() else {
            return;
        };
        if map.contains_key(&canon) {
            return;
        }
        let Ok(mut w) = self.inner.lock() else {
            return;
        };
        if w.watch(&canon, RecursiveMode::Recursive).is_ok() {
            map.insert(canon, repo_path.to_path_buf());
        }
    }
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
