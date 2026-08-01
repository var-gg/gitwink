use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::child_process::{run_command, CommandError, Deadline};
use super::{FetchCapability, FetchOutcome};

const MAX_CAPTURED_STDERR: usize = 16 * 1024;
const MAX_CAPTURED_PATH: usize = 8 * 1024;
const MAX_FETCH_ATTEMPTS: u8 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchTarget {
    pub key: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchRun {
    pub outcome: FetchOutcome,
    pub attempts: u8,
}

/// Resolve the common Git directory through a killable system-Git child.
/// `git2::Repository::discover` and filesystem canonicalization can block in
/// the caller on an offline UNC path; this resolver is covered by the same
/// absolute deadline as the eventual fetch.
pub fn resolve_fetch_target(repo: &Path, deadline: Deadline) -> Result<FetchTarget, FetchOutcome> {
    let mut command = git_command(repo);
    command.args(["rev-parse", "--show-toplevel", "--git-common-dir"]);
    let output = run_command(command, deadline, MAX_CAPTURED_PATH, MAX_CAPTURED_STDERR)
        .map_err(map_command_error)?;
    if !output.status.success() {
        return Err(classify_failure(&output.stderr));
    }

    let Some(common) = output
        .stdout
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
    else {
        return Err(FetchOutcome::Unavailable);
    };
    let common = Path::new(common.trim_end_matches(['\r', '\n']));
    let joined = if common.is_absolute() {
        common.to_path_buf()
    } else {
        repo.join(common)
    };
    Ok(FetchTarget {
        key: coordinator_key(&joined),
    })
}

/// Cheap, local-only capability probe with its own short absolute deadline.
/// Stdout is discarded so a credential-bearing remote URL is never retained.
pub fn repo_fetch_capability(repo: &Path, deadline: Deadline) -> FetchCapability {
    let mut command = git_command(repo);
    command.args(["config", "--local", "--get", "remote.origin.url"]);
    match run_command(command, deadline, 0, MAX_CAPTURED_STDERR) {
        Ok(output) if output.status.success() => FetchCapability::Available,
        Ok(output) if output.stderr.trim().is_empty() => FetchCapability::NoOrigin,
        Ok(output) => match classify_failure(&output.stderr) {
            FetchOutcome::NoOrigin => FetchCapability::NoOrigin,
            _ => FetchCapability::Unavailable,
        },
        Err(_) => FetchCapability::Unavailable,
    }
}

/// Run one hardened `origin` fetch under an already-started absolute deadline.
/// Only transient ref-lock collisions are retried, and every backoff consumes
/// the same deadline budget as repository resolution and preflight.
pub fn git_fetch_one_shot(repo: &Path, deadline: Deadline) -> FetchRun {
    match origin_availability(repo, deadline) {
        Ok(()) => {}
        Err(outcome) => {
            return FetchRun {
                outcome,
                attempts: 0,
            }
        }
    }

    let no_hooks = std::env::temp_dir().join("gitwink-no-hooks");
    let _ = std::fs::create_dir_all(&no_hooks);

    run_fetch_with(repo, deadline, || {
        run_fetch_attempt(repo, &no_hooks, deadline)
    })
}

fn run_fetch_with(
    repo: &Path,
    deadline: Deadline,
    mut attempt_fetch: impl FnMut() -> FetchOutcome,
) -> FetchRun {
    for attempt in 1..=MAX_FETCH_ATTEMPTS {
        let outcome = attempt_fetch();
        if outcome != FetchOutcome::RefLock || attempt == MAX_FETCH_ATTEMPTS {
            return FetchRun {
                outcome,
                attempts: attempt,
            };
        }

        let delay = retry_delay(repo, attempt);
        let Some(remaining) = deadline.remaining() else {
            return FetchRun {
                outcome: FetchOutcome::Timeout,
                attempts: attempt,
            };
        };
        if remaining <= delay {
            std::thread::sleep(remaining);
            return FetchRun {
                outcome: FetchOutcome::Timeout,
                attempts: attempt,
            };
        }
        std::thread::sleep(delay);
    }

    unreachable!("bounded fetch attempt loop always returns")
}

fn origin_availability(repo: &Path, deadline: Deadline) -> Result<(), FetchOutcome> {
    let mut command = git_command(repo);
    command.args(["config", "--local", "--get", "remote.origin.url"]);
    match run_command(command, deadline, 0, MAX_CAPTURED_STDERR) {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) if output.stderr.trim().is_empty() => Err(FetchOutcome::NoOrigin),
        Ok(output) => Err(classify_failure(&output.stderr)),
        Err(error) => Err(map_command_error(error)),
    }
}

fn run_fetch_attempt(repo: &Path, no_hooks: &Path, deadline: Deadline) -> FetchOutcome {
    let mut command = git_command(repo);
    command
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
            "--no-write-fetch-head",
            "--no-tags",
            "--no-recurse-submodules",
            // Ignore any repository-configured mirror refmap. Only the
            // explicit branch-only mapping below may choose destinations.
            "--refmap=",
        ])
        .arg("origin")
        .arg("+refs/heads/*:refs/remotes/origin/*");

    match run_command(command, deadline, 0, MAX_CAPTURED_STDERR) {
        Ok(output) if output.status.success() => FetchOutcome::Ok,
        Ok(output) => classify_failure(&output.stderr),
        Err(error) => map_command_error(error),
    }
}

fn git_command(repo: &Path) -> Command {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env("SSH_ASKPASS_REQUIRE", "never")
        // Git's human-readable errors are an implementation detail used only
        // for bounded classification. Pin them to a stable locale.
        .env("LC_ALL", "C")
        .env("LANG", "C")
        .env("LANGUAGE", "C")
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
        .env_remove("GIT_HTTP_LOW_SPEED_TIME");
    command
}

fn retry_delay(repo: &Path, failed_attempt: u8) -> Duration {
    let (minimum, spread) = match failed_attempt {
        1 => (100_u64, 151_u64), // 100..=250ms
        _ => (250_u64, 351_u64), // 250..=600ms
    };
    let mut hasher = DefaultHasher::new();
    repo.hash(&mut hasher);
    failed_attempt.hash(&mut hasher);
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.subsec_nanos())
        .unwrap_or(0)
        .hash(&mut hasher);
    Duration::from_millis(minimum + hasher.finish() % spread)
}

fn map_command_error(error: CommandError) -> FetchOutcome {
    match error {
        CommandError::NotFound => FetchOutcome::Unavailable,
        CommandError::TimedOut => FetchOutcome::Timeout,
        CommandError::Failed => FetchOutcome::Failed,
    }
}

fn classify_failure(stderr: &str) -> FetchOutcome {
    let lower = stderr.to_ascii_lowercase();
    if contains_any(
        &lower,
        &[
            "cannot lock ref",
            "another git process",
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
            "no such remote 'origin'",
            "no such remote \"origin\"",
            "remote origin not found",
        ],
    ) {
        return FetchOutcome::NoOrigin;
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

fn coordinator_key(path: &Path) -> PathBuf {
    // Git has just resolved and accessed this common directory under the
    // caller's deadline. Collapse filesystem aliases before using it as the
    // process-wide flight key: macOS can expose the same temp/worktree path as
    // both `/var/...` and `/private/var/...`. If the path races away, retain
    // the existing lexical fallback rather than making refresh unavailable.
    let physical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut normalized = PathBuf::new();
    for component in physical.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                let _ = normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    #[cfg(windows)]
    {
        PathBuf::from(
            normalized
                .to_string_lossy()
                .replace('/', "\\")
                .to_lowercase(),
        )
    }
    #[cfg(not(windows))]
    {
        normalized
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_preflight_distinguishes_missing_and_unavailable_repositories() {
        let root = tempfile::tempdir().unwrap();
        assert_eq!(
            repo_fetch_capability(root.path(), Deadline::after(Duration::from_secs(2))),
            FetchCapability::Unavailable
        );

        let repo = git2::Repository::init(root.path()).unwrap();
        assert_eq!(
            repo_fetch_capability(root.path(), Deadline::after(Duration::from_secs(2))),
            FetchCapability::NoOrigin
        );
        repo.remote("origin", "https://example.invalid/repo.git")
            .unwrap();
        assert_eq!(
            repo_fetch_capability(root.path(), Deadline::after(Duration::from_secs(2))),
            FetchCapability::Available
        );
    }

    #[test]
    fn one_shot_fetch_updates_only_the_origin_branch_mirror() {
        let fixture = FetchFixture::new();

        let run = git_fetch_one_shot(
            &fixture.client_path,
            Deadline::after(Duration::from_secs(5)),
        );
        assert_eq!(run.outcome, FetchOutcome::Ok);
        assert_eq!(run.attempts, 1);
        assert_eq!(
            fixture
                .client
                .refname_to_id("refs/remotes/origin/main")
                .unwrap(),
            fixture.commit_id
        );
        assert!(fixture
            .client
            .find_reference("refs/heads/unsafe/main")
            .is_err());
        assert!(fixture.client.find_reference("refs/tags/v1.0.0").is_err());
        assert!(!fixture.client.path().join("FETCH_HEAD").exists());
        assert_eq!(
            fixture.client.head().unwrap().target().unwrap(),
            fixture.local_commit_id
        );
        assert_eq!(
            fixture
                .client
                .refname_to_id("refs/tags/local-only")
                .unwrap(),
            fixture.local_commit_id
        );
        assert_eq!(
            std::fs::read(fixture.client_path.join("local.txt")).unwrap(),
            b"local worktree\n"
        );
        assert!(!fixture.client_path.join("README.md").exists());
    }

    #[test]
    fn transient_external_ref_lock_is_retried_successfully() {
        let fixture = FetchFixture::new();
        let lock_path = fixture.client.path().join("refs/remotes/origin/main.lock");
        std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        std::fs::write(&lock_path, b"held by external Git").unwrap();
        let deadline = Deadline::after(Duration::from_secs(5));
        origin_availability(&fixture.client_path, deadline).unwrap();
        let no_hooks = std::env::temp_dir().join("gitwink-no-hooks");
        let mut calls = 0;
        let run = run_fetch_with(&fixture.client_path, deadline, || {
            calls += 1;
            let outcome = run_fetch_attempt(&fixture.client_path, &no_hooks, deadline);
            if calls == 1 {
                assert_eq!(outcome, FetchOutcome::RefLock);
                std::fs::remove_file(&lock_path).unwrap();
            }
            outcome
        });
        assert_eq!(run.outcome, FetchOutcome::Ok);
        assert_eq!(run.attempts, 2);
    }

    #[test]
    fn persistent_external_ref_lock_stops_at_retry_budget() {
        let fixture = FetchFixture::new();
        let lock_path = fixture.client.path().join("refs/remotes/origin/main.lock");
        std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        std::fs::write(&lock_path, b"held by external Git").unwrap();

        let run = git_fetch_one_shot(
            &fixture.client_path,
            Deadline::after(Duration::from_secs(5)),
        );
        assert_eq!(run.outcome, FetchOutcome::RefLock);
        assert_eq!(run.attempts, MAX_FETCH_ATTEMPTS);
    }

    #[test]
    fn absolute_deadline_includes_discovery_preflight_and_retry_backoff() {
        let fixture = FetchFixture::new();
        let lock_path = fixture.client.path().join("refs/remotes/origin/main.lock");
        std::fs::create_dir_all(lock_path.parent().unwrap()).unwrap();
        std::fs::write(&lock_path, b"held by external Git").unwrap();

        let started = std::time::Instant::now();
        let deadline = Deadline::after(Duration::from_secs(1));
        resolve_fetch_target(&fixture.client_path, deadline).unwrap();
        if let Some(remaining) = deadline.remaining() {
            std::thread::sleep(remaining.saturating_sub(Duration::from_millis(300)));
        }
        let run = git_fetch_one_shot(&fixture.client_path, deadline);
        assert_eq!(run.outcome, FetchOutcome::Timeout);
        assert!(
            started.elapsed() < Duration::from_millis(1_500),
            "deadline overran by more than process cleanup grace"
        );
    }

    #[test]
    fn linked_worktrees_resolve_to_the_same_common_directory_key() {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        let linked = root.path().join("linked");
        let repository = repository_with_initial_commit(&main);
        repository.worktree("linked", &linked, None).unwrap();

        let main_target =
            resolve_fetch_target(&main, Deadline::after(Duration::from_secs(2))).unwrap();
        let linked_target =
            resolve_fetch_target(&linked, Deadline::after(Duration::from_secs(2))).unwrap();
        assert_eq!(main_target.key, linked_target.key);
    }

    #[cfg(unix)]
    #[test]
    fn coordinator_key_collapses_filesystem_aliases() {
        use std::os::unix::fs::symlink;

        let root = tempfile::tempdir().unwrap();
        let physical = root.path().join("physical");
        let alias = root.path().join("alias");
        std::fs::create_dir(&physical).unwrap();
        symlink(&physical, &alias).unwrap();

        assert_eq!(coordinator_key(&physical), coordinator_key(&alias));
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

    struct FetchFixture {
        _root: tempfile::TempDir,
        client_path: PathBuf,
        client: git2::Repository,
        commit_id: git2::Oid,
        local_commit_id: git2::Oid,
    }

    impl FetchFixture {
        fn new() -> Self {
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
            std::fs::write(client_path.join("local.txt"), b"local worktree\n").unwrap();
            let mut index = client.index().unwrap();
            index.add_path(Path::new("local.txt")).unwrap();
            index.write().unwrap();
            let local_tree_id = index.write_tree().unwrap();
            drop(index);
            let local_tree = client.find_tree(local_tree_id).unwrap();
            let local_commit_id = client
                .commit(
                    Some("HEAD"),
                    &signature,
                    &signature,
                    "local commit",
                    &local_tree,
                    &[],
                )
                .unwrap();
            drop(local_tree);
            client
                .reference(
                    "refs/tags/local-only",
                    local_commit_id,
                    true,
                    "local test tag",
                )
                .unwrap();
            client
                .remote("origin", &origin_path.to_string_lossy())
                .unwrap();
            client
                .remote_add_fetch("origin", "+refs/heads/*:refs/heads/unsafe/*")
                .unwrap();
            Self {
                _root: root,
                client_path,
                client,
                commit_id,
                local_commit_id,
            }
        }
    }

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
}
