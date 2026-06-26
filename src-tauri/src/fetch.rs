//! One-shot `git fetch` on panel summon (opt-out — on by default).
//!
//! gitwink's libgit2 is built without a network transport, so fetching is
//! impossible through it — and `git.rs` is contractually read-only. This
//! module instead shells out to the SYSTEM `git` binary for a single,
//! non-interactive `git fetch` of the currently-viewed repo when the tray
//! panel is summoned, so a teammate's just-pushed commit surfaces. The
//! existing file-watcher turns the resulting `refs/remotes/origin/*` +
//! `FETCH_HEAD` update into a timeline refresh, so all this module does is
//! *trigger* the fetch.
//!
//! Safety: the fetch is pinned to an EXPLICIT remote (`origin`) and an
//! EXPLICIT branch-only refspec (`+refs/heads/*:refs/remotes/origin/*`), so it
//! can only write the remote-tracking mirror — never your local branches,
//! tags, working tree, or history, regardless of the repo's configured
//! refspecs / `fetch.all` / mirror setup. Repo-owned hooks are disabled for
//! the run (`core.hooksPath` → empty dir) and submodule recursion is off, so
//! the fetch contacts only `origin` and runs no in-repo hook code. This is the
//! real "can't damage your work" guarantee. (A repo whose primary remote isn't
//! named `origin` simply isn't auto-fetched — the timeline roots on
//! `refs/remotes/origin/*`, so there'd be nothing to surface anyway.)
//!
//! Guarantees: async/non-blocking (runs on a blocking thread), best-effort
//! non-interactive (no credential prompt under the common helpers — silent
//! no-op on auth failure / no git / no `origin` / no network), per-repo
//! cooldown, global concurrency cap, single-repo mode only.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

/// Skip a repo's fetch if we fetched it within this window.
pub const FETCH_COOLDOWN: Duration = Duration::from_secs(180);

/// Cap on concurrently-running fetches across all repos, so rapid repo
/// switching can't spawn a pile of blocking `git` processes at once.
const MAX_INFLIGHT: u32 = 2;

/// Last-resort backstop on a single fetch — NOT the normal bound. git's own
/// http.lowSpeedLimit/lowSpeedTime (set below) aborts a genuinely stalled
/// HTTP(S) transfer in ~20s, so this only catches a non-HTTP hang (e.g. a
/// wedged ssh). Kept generous so a slow-but-progressing LARGE fetch of a
/// long-stale repo runs to completion instead of being killed mid-transfer.
const FETCH_TIMEOUT: Duration = Duration::from_secs(150);

/// Per-repo last-fetch timestamps + a global in-flight counter, so repeated
/// summons don't spam a remote and a burst of repo switches can't fan out.
/// Managed Tauri state. Keys are canonicalized so symlinks / `..` / Windows
/// case-aliases / linked worktrees can't slip past the cooldown.
pub struct FetchCooldown {
    last: Mutex<HashMap<PathBuf, Instant>>,
    inflight: Arc<AtomicU32>,
}

impl Default for FetchCooldown {
    fn default() -> Self {
        Self {
            last: Mutex::new(HashMap::new()),
            inflight: Arc::new(AtomicU32::new(0)),
        }
    }
}

/// Held for the lifetime of one in-flight fetch; decrements the global
/// counter on drop (i.e. when `git_fetch_one_shot` returns). Move it into the
/// `spawn_blocking` closure so the slot is released no matter how it exits.
pub struct FetchGuard(Arc<AtomicU32>);

impl Drop for FetchGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

impl FetchCooldown {
    /// Atomically check-and-record: returns `Some(guard)` (and stamps `now` +
    /// reserves a concurrency slot) if the repo is eligible to fetch, `None`
    /// if it was fetched within `interval` OR the global cap is full. Claimed
    /// BEFORE spawning so rapid re-summons during an in-flight fetch are
    /// suppressed. A cap-rejected repo is NOT stamped, so it retries next
    /// summon. Fail-closed on a poisoned lock (skip the fetch).
    pub fn try_claim(&self, repo: &Path, interval: Duration) -> Option<FetchGuard> {
        let key = std::fs::canonicalize(repo).unwrap_or_else(|_| repo.to_path_buf());
        let Ok(mut map) = self.last.lock() else {
            return None;
        };
        let now = Instant::now();
        if let Some(last) = map.get(&key) {
            if now.duration_since(*last) < interval {
                return None;
            }
        }
        if self.inflight.load(Ordering::SeqCst) >= MAX_INFLIGHT {
            return None;
        }
        map.insert(key, now);
        self.inflight.fetch_add(1, Ordering::SeqCst);
        Some(FetchGuard(Arc::clone(&self.inflight)))
    }
}

/// Fire a single non-interactive `git fetch origin` against `repo`, scoped to
/// the remote-tracking mirror only. Blocks the calling thread — run it under
/// `spawn_blocking`. Every failure mode (no git on PATH, no `origin`, auth
/// required, network down, repo gone) is swallowed silently; this feature
/// must never surface an error or a prompt.
pub fn git_fetch_one_shot(repo: &Path) {
    // An empty, app-owned hooks dir disables any repo-owned hook (notably the
    // `reference-transaction` hook a ref update would otherwise run) for this
    // invocation only. A missing dir already means "no hooks", but create it
    // so git never warns; a creation failure is harmless (still no hooks).
    let no_hooks = std::env::temp_dir().join("gitwink-no-hooks");
    let _ = std::fs::create_dir_all(&no_hooks);

    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo)
        // Disable repo-owned hooks for this run (see above).
        .arg("-c")
        .arg(format!("core.hooksPath={}", no_hooks.display()))
        // Abort only a STALLED transfer (HTTP/S): < 1 KB/s sustained for 20s.
        // This lets a slow-but-progressing large fetch of a long-stale repo
        // finish, instead of a fixed wall-clock kill cutting it off mid-pack.
        // SSH remotes ignore these and lean on the FETCH_TIMEOUT backstop.
        .args(["-c", "http.lowSpeedLimit=1000", "-c", "http.lowSpeedTime=20"])
        // Don't let a fetch kick off background repo maintenance / repacking —
        // we promise not to churn the user's repo, and it'd hurt the "instant"
        // feel. Belt-and-suspenders against either knob.
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        // Pure perf: skip forced-update graph work we don't display, and never
        // delete refs (no prune) — a fetch should only ever ADD to the mirror.
        .args(["-c", "fetch.showForcedUpdates=false"])
        .args(["-c", "fetch.prune=false", "-c", "fetch.pruneTags=false"])
        .args([
            "fetch",
            // All-or-nothing ref update (git >= 2.31).
            "--atomic",
            "--quiet",
            // No tag refspec is given below, and --no-tags also disables tag
            // auto-following, so no tags are written.
            "--no-tags",
            // One "repo fetch" must not fan out to submodule remotes.
            "--no-recurse-submodules",
            // CRITICAL: discard the repo's configured `remote.origin.fetch`
            // refmap. Without this, git still applies a mirror refspec like
            // `+refs/*:refs/*` as the refmap even though we pass our own
            // refspec below — which would map fetched heads onto local
            // refs/heads/* (clobbering your branches, or erroring on the
            // checked-out one). `--refmap=` makes ONLY the explicit refspec
            // below decide where refs land. (git >= 2.6.)
            "--refmap=",
        ])
        // EXPLICIT remote + EXPLICIT branch-only refspec. Combined with the
        // empty --refmap above, this is the safety pin: regardless of the
        // repo's configured refspecs / fetch.all / mirror setup, we can only
        // ever write refs/remotes/origin/*. If `origin` doesn't exist git
        // fails fast and we no-op (swallowed).
        .arg("origin")
        .arg("+refs/heads/*:refs/remotes/origin/*")
        // Non-interactive: never pop a credential prompt. GIT_TERMINAL_PROMPT
        // disables git's own TTY prompt; GCM's GUI is suppressed; ssh's askpass
        // GUI is forbidden (a key passphrase fails fast instead of popping a
        // dialog). We do NOT override the ssh binary itself, so custom
        // ssh/plink setups keep working; nulled stdio + the timeout bound the
        // rest. (Arbitrary user credential helpers can still surface UI — hence
        // "best-effort", not "never", in the docs.)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("SSH_ASKPASS_REQUIRE", "never")
        // Strip env that could redirect WHICH repo git acts on, inject config,
        // pop a GUI askpass, or override the low-speed knobs above. We do NOT
        // env_clear() — that would break PATH, ssh-agent, the credential
        // manager, and proxy / enterprise setups.
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_NAMESPACE")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_HTTP_LOW_SPEED_LIMIT")
        .env_remove("GIT_HTTP_LOW_SPEED_TIME")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // CREATE_NO_WINDOW — no console flash when spawning git on Windows.
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000);

    if let Ok(child) = cmd.spawn() {
        if wait_with_timeout(child, FETCH_TIMEOUT) {
            // Hit the backstop — a wedged/extremely-slow transfer the
            // low-speed abort didn't catch (e.g. ssh). Logged, not surfaced:
            // the fetch retries after the cooldown. With --atomic the ref
            // update is all-or-nothing, so the mirror isn't left half-updated;
            // some objects may have downloaded into the object store (inert).
            eprintln!(
                "gitwink: auto-fetch hit the {}s backstop for {} — transfer killed; the ref update did not complete this round",
                FETCH_TIMEOUT.as_secs(),
                repo.display()
            );
        }
    }
}

/// Poll-wait for the child, killing it past `timeout`. `std::process` has no
/// native timeout; a short poll loop avoids pulling in an async-process dep.
/// Returns true iff the child was killed for exceeding `timeout`.
///
/// Note: `kill()` reaps only the immediate `git` process. Descendants (ssh, a
/// credential helper, pack-processing) normally die when git's pipes close; a
/// wedged grandchild could outlive us. Full process-tree teardown (a Windows
/// Job Object) is deliberately deferred — the backstop rarely fires.
fn wait_with_timeout(mut child: std::process::Child, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return false,
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return true;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            // Don't drop a possibly-live child: dropping `Child` neither kills
            // nor reaps. Kill + reap, then report "not a timeout".
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}
