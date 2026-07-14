use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

#[cfg(windows)]
use std::os::windows::process::CommandExt;

use super::{FetchCapability, FetchOutcome};

const MAX_CAPTURED_STDERR: usize = 16 * 1024;
const STDERR_COLLECTION_GRACE: Duration = Duration::from_millis(200);

pub fn repo_fetch_capability(repo: &Path) -> FetchCapability {
    origin_availability(repo)
}

/// Run one hardened, non-interactive system-Git fetch. The explicit remote,
/// empty refmap, and branch-only destination ensure this can update only
/// `refs/remotes/origin/*`; it never touches local branches, tags, the index,
/// working tree, or submodules.
pub fn git_fetch_one_shot(repo: &Path, timeout: Duration) -> FetchOutcome {
    match origin_availability(repo) {
        FetchCapability::NoOrigin => return FetchOutcome::NoOrigin,
        FetchCapability::Unavailable => return FetchOutcome::Unavailable,
        FetchCapability::Available => {}
    }

    let no_hooks = std::env::temp_dir().join("gitwink-no-hooks");
    let _ = std::fs::create_dir_all(&no_hooks);

    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo)
        .arg("-c")
        .arg(format!("core.hooksPath={}", no_hooks.display()))
        // Abort a genuinely stalled HTTP(S) transfer while allowing a large,
        // steadily-progressing pack to finish within the caller's wall clock.
        .args([
            "-c",
            "http.lowSpeedLimit=1000",
            "-c",
            "http.lowSpeedTime=20",
        ])
        .args(["-c", "maintenance.auto=false", "-c", "gc.auto=0"])
        .args(["-c", "fetch.showForcedUpdates=false"])
        .args(["-c", "fetch.prune=false", "-c", "fetch.pruneTags=false"])
        .args([
            "fetch",
            "--atomic",
            "--quiet",
            "--no-tags",
            "--no-recurse-submodules",
            // Ignore any repository-configured mirror refmap. Only the
            // explicit branch-only mapping below may choose destinations.
            "--refmap=",
        ])
        .arg("origin")
        .arg("+refs/heads/*:refs/remotes/origin/*")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("SSH_ASKPASS_REQUIRE", "never")
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
        // Drain stderr to avoid pipe back-pressure, but retain only a bounded
        // prefix in memory for stable error classification.
        .stderr(Stdio::piped());

    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return FetchOutcome::Unavailable;
        }
        Err(_) => return FetchOutcome::Failed,
    };
    let stderr_capture = child
        .stderr
        .take()
        .map(|stderr| spawn_stderr_capture(stderr, MAX_CAPTURED_STDERR));

    match wait_with_timeout(child, timeout) {
        WaitOutcome::Exited(status) if status.success() => {
            // The reader has already drained concurrently. A successful fetch
            // has no error to classify, so never extend the UI wait for EOF
            // from an inherited descendant pipe.
            drop(stderr_capture);
            FetchOutcome::Ok
        }
        WaitOutcome::Exited(_) => {
            let stderr = collect_stderr(stderr_capture);
            classify_failure(&stderr)
        }
        WaitOutcome::TimedOut => {
            // A wedged descendant may still own the pipe. Detach the reader
            // instead of turning the timeout itself into another unbounded wait.
            drop(stderr_capture);
            FetchOutcome::Timeout
        }
        WaitOutcome::Errored => {
            drop(stderr_capture);
            FetchOutcome::Failed
        }
    }
}

fn origin_availability(repo: &Path) -> FetchCapability {
    let Ok(repository) = git2::Repository::discover(repo) else {
        return FetchCapability::Unavailable;
    };
    let availability = match repository.find_remote("origin") {
        Ok(remote) if remote.url().is_some_and(|url| !url.trim().is_empty()) => {
            FetchCapability::Available
        }
        _ => FetchCapability::NoOrigin,
    };
    availability
}

fn read_bounded(mut reader: impl Read, limit: usize) -> String {
    let mut captured = Vec::with_capacity(limit.min(4096));
    let mut buffer = [0_u8; 4096];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let remaining = limit.saturating_sub(captured.len());
        if remaining > 0 {
            captured.extend_from_slice(&buffer[..read.min(remaining)]);
        }
    }
    String::from_utf8_lossy(&captured).into_owned()
}

fn spawn_stderr_capture(reader: impl Read + Send + 'static, limit: usize) -> Receiver<String> {
    let (sender, receiver) = mpsc::sync_channel(1);
    std::thread::spawn(move || {
        let _ = sender.send(read_bounded(reader, limit));
    });
    receiver
}

fn collect_stderr(capture: Option<Receiver<String>>) -> String {
    capture
        .and_then(|capture| capture.recv_timeout(STDERR_COLLECTION_GRACE).ok())
        .unwrap_or_default()
}

fn classify_failure(stderr: &str) -> FetchOutcome {
    let lower = stderr.to_ascii_lowercase();
    if contains_any(
        &lower,
        &[
            "cannot lock ref",
            "another git process",
            "unable to create",
            ".lock': file exists",
            ".lock\": file exists",
        ],
    ) {
        return FetchOutcome::RefLock;
    }
    if contains_any(
        &lower,
        &[
            "authentication failed",
            "authentication required",
            "could not read username",
            "terminal prompts disabled",
            "permission denied (publickey)",
            "repository not found",
            "access denied",
            "the requested url returned error: 401",
            "the requested url returned error: 403",
        ],
    ) {
        return FetchOutcome::Authentication;
    }
    if contains_any(
        &lower,
        &[
            "could not resolve host",
            "failed to connect",
            "connection timed out",
            "connection reset",
            "network is unreachable",
            "unable to access",
            "ssl certificate problem",
            "tls",
            "could not read from remote repository",
        ],
    ) {
        return FetchOutcome::Network;
    }
    if contains_any(
        &lower,
        &[
            "not a git repository",
            "cannot change to",
            "no such file or directory",
        ],
    ) {
        return FetchOutcome::Unavailable;
    }
    FetchOutcome::Failed
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

enum WaitOutcome {
    Exited(std::process::ExitStatus),
    TimedOut,
    Errored,
}

fn wait_with_timeout(mut child: std::process::Child, timeout: Duration) -> WaitOutcome {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return WaitOutcome::Exited(status),
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return WaitOutcome::TimedOut;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return WaitOutcome::Errored;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn origin_preflight_distinguishes_missing_and_unavailable_repositories() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            origin_availability(root.path()),
            FetchCapability::Unavailable
        );

        let repo = git2::Repository::init(root.path()).unwrap();
        assert_eq!(origin_availability(root.path()), FetchCapability::NoOrigin);
        repo.remote("origin", "https://example.invalid/repo.git")
            .unwrap();
        assert_eq!(origin_availability(root.path()), FetchCapability::Available);
    }

    #[test]
    fn one_shot_fetch_updates_only_the_origin_branch_mirror() {
        let root = tempfile::tempdir().unwrap();
        let origin_path = root.path().join("origin.git");
        let client_path = root.path().join("client");
        let origin = git2::Repository::init_bare(&origin_path).unwrap();
        let blob_id = origin.blob(b"hello\n").unwrap();
        let mut tree_builder = origin.treebuilder(None).unwrap();
        tree_builder.insert("README.md", blob_id, 0o100644).unwrap();
        let tree_id = tree_builder.write().unwrap();
        drop(tree_builder);
        let tree = origin.find_tree(tree_id).unwrap();
        let signature = git2::Signature::now("gitwink", "tests@gitwink.local").unwrap();
        let commit_id = origin
            .commit(
                Some("refs/heads/main"),
                &signature,
                &signature,
                "initial",
                &tree,
                &[],
            )
            .unwrap();
        origin
            .reference("refs/tags/v1.0.0", commit_id, true, "test tag")
            .unwrap();

        let client = git2::Repository::init(&client_path).unwrap();
        client
            .remote("origin", &origin_path.to_string_lossy())
            .unwrap();
        client
            .remote_add_fetch("origin", "+refs/heads/*:refs/heads/unsafe/*")
            .unwrap();

        assert_eq!(
            git_fetch_one_shot(&client_path, Duration::from_secs(5)),
            FetchOutcome::Ok
        );
        assert_eq!(
            client.refname_to_id("refs/remotes/origin/main").unwrap(),
            commit_id
        );
        assert!(client.find_reference("refs/heads/unsafe/main").is_err());
        assert!(client.find_reference("refs/tags/v1.0.0").is_err());
    }

    #[test]
    fn failure_classifier_prefers_ref_lock_over_generic_text() {
        assert_eq!(
            classify_failure("error: cannot lock ref 'refs/remotes/origin/main'"),
            FetchOutcome::RefLock
        );
        assert_eq!(
            classify_failure("fatal: Authentication failed for remote"),
            FetchOutcome::Authentication
        );
        assert_eq!(
            classify_failure("fatal: unable to access: Could not resolve host"),
            FetchOutcome::Network
        );
        assert_eq!(classify_failure("fatal: unknown"), FetchOutcome::Failed);
    }

    #[test]
    fn stderr_capture_is_bounded_but_fully_drains_input() {
        let input = vec![b'x'; MAX_CAPTURED_STDERR * 2];
        let captured = read_bounded(Cursor::new(input), MAX_CAPTURED_STDERR);
        assert_eq!(captured.len(), MAX_CAPTURED_STDERR);
    }
}
