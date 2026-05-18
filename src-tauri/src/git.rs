// Read-only git access via `git2`.
//
// HARD RULE: every call here must be read-only. No commits, fetches, pushes,
// merges, or any operation that mutates a repo's state on disk.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use git2::{Oid, Repository, Sort};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitSummary {
    pub repo_path: String,
    pub repo_name: String,
    pub hash: String,
    pub short_hash: String,
    pub summary: String,
    pub author: String,
    pub email: String,
    /// Unix seconds.
    pub timestamp: i64,
    /// Branch-name hint for commits NOT reachable from the currently checked-out
    /// branch. None means "this commit is on the branch you're already on" (or
    /// HEAD is detached, in which case we suppress all labels to avoid noise).
    pub branch_label: Option<String>,
    /// True if this commit has more than one parent (merge commit).
    pub is_merge: bool,
    /// True if any tag points at this commit.
    pub is_tagged: bool,
}

/// Walk HEAD and return up to `max_count` recent commits whose author time is
/// at or after `since_unix_seconds`. Empty / detached repos return an empty
/// vector rather than erroring.
pub fn recent_commits(
    repo_path: &Path,
    max_count: usize,
    since_unix_seconds: i64,
) -> Result<Vec<CommitSummary>> {
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;

    let repo_path_str = repo_path.to_string_lossy().into_owned();
    let repo_name = repo_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();

    // Identify what HEAD is on right now so we can suppress branch labels for
    // the user's current branch (the spec rule: "체크아웃된 브랜치는 생략").
    let detached = repo.head_detached().unwrap_or(true);
    let head_branch_name: Option<String> = if detached {
        None
    } else {
        repo.head()
            .ok()
            .and_then(|h| h.shorthand().map(|s| s.to_string()))
    };

    // Set of OIDs reachable from HEAD — these get no label.
    let head_reachable: HashSet<Oid> = if head_branch_name.is_some() {
        match repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .and_then(|c| {
                let mut rw = repo.revwalk().ok()?;
                rw.push(c.id()).ok()?;
                Some(rw.flatten().collect::<HashSet<_>>())
            }) {
            Some(set) => set,
            None => HashSet::new(),
        }
    } else {
        HashSet::new()
    };

    // Collect oids that any tag points to (peel annotated tags through to the
    // referenced commit). Used to set is_tagged on the result rows.
    let tagged_oids: HashSet<Oid> = {
        let mut set = HashSet::new();
        if let Ok(refs) = repo.references_glob("refs/tags/*") {
            for r in refs.flatten() {
                if let Ok(obj) = r.peel(git2::ObjectType::Commit) {
                    set.insert(obj.id());
                }
            }
        }
        set
    };

    // For each NON-current branch, build oid -> branch_name (first encountered
    // wins). Skips commits already on HEAD.
    let mut commit_to_branch: HashMap<Oid, String> = HashMap::new();
    if let Ok(refs) = repo.references_glob("refs/heads/*") {
        for r in refs.flatten() {
            let Some(name) = r.shorthand().map(|s| s.to_string()) else {
                continue;
            };
            if Some(&name) == head_branch_name.as_ref() {
                continue;
            }
            let Some(tip) = r.target() else { continue };
            let mut rw = match repo.revwalk() {
                Ok(rw) => rw,
                Err(_) => continue,
            };
            if rw.push(tip).is_err() {
                continue;
            }
            for oid_res in rw {
                let Ok(oid) = oid_res else { continue };
                if head_reachable.contains(&oid) {
                    continue;
                }
                commit_to_branch.entry(oid).or_insert_with(|| name.clone());
            }
        }
    }

    let mut revwalk = repo.revwalk().context("init revwalk")?;

    // Walk every local ref head, not just HEAD. The revwalk dedupes commits
    // that appear on multiple branches, so an agent that committed to a
    // feature branch (or to a branch the user isn't currently on) still
    // shows up in the timeline.
    let mut pushed = 0usize;
    if let Ok(refs) = repo.references_glob("refs/heads/*") {
        for r in refs.flatten() {
            if let Some(oid) = r.target() {
                if revwalk.push(oid).is_ok() {
                    pushed += 1;
                }
            }
        }
    }
    // Fall back to HEAD if for some reason no heads were enumerable
    // (newly-init'd repo, detached HEAD, etc.).
    if pushed == 0 {
        let head = match repo.head() {
            Ok(h) => h,
            Err(_) => return Ok(Vec::new()),
        };
        let head_commit = match head.peel_to_commit() {
            Ok(c) => c,
            Err(_) => return Ok(Vec::new()),
        };
        revwalk.push(head_commit.id()).context("push HEAD")?;
    }

    revwalk
        .set_sorting(Sort::TIME)
        .context("set revwalk sort")?;

    let mut out: Vec<CommitSummary> = Vec::with_capacity(max_count);
    for oid in revwalk {
        if out.len() >= max_count {
            break;
        }
        let oid = match oid {
            Ok(o) => o,
            Err(_) => continue,
        };
        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let ts = commit.time().seconds();
        if ts < since_unix_seconds {
            // TIME sort is descending, so older commits won't reappear later.
            break;
        }
        let hash = oid.to_string();
        let short_hash: String = hash.chars().take(7).collect();
        let summary = commit.summary().unwrap_or("").to_string();
        let author = commit.author();
        let branch_label = if head_branch_name.is_none() || head_reachable.contains(&oid) {
            None
        } else {
            commit_to_branch.get(&oid).cloned()
        };
        out.push(CommitSummary {
            repo_path: repo_path_str.clone(),
            repo_name: repo_name.clone(),
            hash,
            short_hash,
            summary,
            author: author.name().unwrap_or("").to_string(),
            email: author.email().unwrap_or("").to_string(),
            timestamp: ts,
            branch_label,
            is_merge: commit.parent_count() > 1,
            is_tagged: tagged_oids.contains(&oid),
        });
    }

    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Signature, Time};
    use tempfile::TempDir;

    fn empty_tree(repo: &Repository) -> git2::Tree<'_> {
        let mut index = repo.index().unwrap();
        let tree_id = index.write_tree().unwrap();
        repo.find_tree(tree_id).unwrap()
    }

    fn commit_with_time(
        repo: &Repository,
        msg: &str,
        ts: i64,
        parents: &[&git2::Commit<'_>],
    ) -> git2::Oid {
        let sig = Signature::new("t", "t@e", &Time::new(ts, 0)).unwrap();
        let tree = empty_tree(repo);
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, parents)
            .unwrap()
    }

    #[test]
    fn empty_repo_returns_empty() {
        let dir = TempDir::new().unwrap();
        let _ = Repository::init(dir.path()).unwrap();
        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        assert!(commits.is_empty());
    }

    #[test]
    fn returns_commits_newest_first() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let c1 = commit_with_time(&repo, "first", 1_000, &[]);
        let c1c = repo.find_commit(c1).unwrap();
        let c2 = commit_with_time(&repo, "second", 2_000, &[&c1c]);
        let c2c = repo.find_commit(c2).unwrap();
        let _ = commit_with_time(&repo, "third", 3_000, &[&c2c]);

        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        assert_eq!(
            commits.iter().map(|c| c.summary.as_str()).collect::<Vec<_>>(),
            vec!["third", "second", "first"]
        );
        assert_eq!(commits[0].timestamp, 3_000);
    }

    #[test]
    fn caps_max_count() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let mut parent_oid = commit_with_time(&repo, "c0", 1_000, &[]);
        for i in 1..5 {
            let parent = repo.find_commit(parent_oid).unwrap();
            parent_oid = commit_with_time(&repo, &format!("c{i}"), 1_000 + i, &[&parent]);
        }

        let commits = recent_commits(dir.path(), 3, 0).unwrap();
        assert_eq!(commits.len(), 3);
    }

    #[test]
    fn cutoff_excludes_old_commits() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let old = commit_with_time(&repo, "old", 100, &[]);
        let oldc = repo.find_commit(old).unwrap();
        let _new = commit_with_time(&repo, "new", 2_000, &[&oldc]);

        let commits = recent_commits(dir.path(), 10, 1_000).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].summary, "new");
    }

    #[test]
    fn current_branch_commits_have_no_label() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let _ = commit_with_time(&repo, "on current", 1_000, &[]);
        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].branch_label, None);
    }

    #[test]
    fn other_branch_commits_get_label() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let main = commit_with_time(&repo, "shared", 1_000, &[]);
        let mainc = repo.find_commit(main).unwrap();

        repo.branch("dev", &mainc, false).unwrap();
        let sig = Signature::new("t", "t@e", &Time::new(2_000, 0)).unwrap();
        let tree = empty_tree(&repo);
        repo.commit(
            Some("refs/heads/dev"),
            &sig,
            &sig,
            "dev-only",
            &tree,
            &[&mainc],
        )
        .unwrap();

        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        let dev_commit = commits.iter().find(|c| c.summary == "dev-only").unwrap();
        let shared_commit = commits.iter().find(|c| c.summary == "shared").unwrap();
        assert_eq!(dev_commit.branch_label.as_deref(), Some("dev"));
        assert_eq!(shared_commit.branch_label, None);
    }

    #[test]
    fn detached_head_suppresses_all_labels() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let c1 = commit_with_time(&repo, "first", 1_000, &[]);
        let c1c = repo.find_commit(c1).unwrap();
        repo.branch("dev", &c1c, false).unwrap();
        let sig = Signature::new("t", "t@e", &Time::new(2_000, 0)).unwrap();
        let tree = empty_tree(&repo);
        repo.commit(
            Some("refs/heads/dev"),
            &sig,
            &sig,
            "dev-only",
            &tree,
            &[&c1c],
        )
        .unwrap();

        // Detach HEAD onto the first commit.
        repo.set_head_detached(c1).unwrap();

        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        assert!(
            commits.iter().all(|c| c.branch_label.is_none()),
            "detached HEAD must suppress all labels: {commits:?}"
        );
    }

    #[test]
    fn walks_all_local_branches() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        // Commit on the default branch (becomes "refs/heads/master" or
        // "refs/heads/main" depending on git config).
        let main1 = commit_with_time(&repo, "main-only", 1_000, &[]);
        let main1c = repo.find_commit(main1).unwrap();

        // Branch "dev" off main and put a unique commit on it.
        repo.branch("dev", &main1c, false).unwrap();
        let sig = Signature::new("t", "t@e", &Time::new(2_000, 0)).unwrap();
        let tree = empty_tree(&repo);
        repo.commit(
            Some("refs/heads/dev"),
            &sig,
            &sig,
            "dev-only",
            &tree,
            &[&main1c],
        )
        .unwrap();

        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        let summaries: Vec<&str> = commits.iter().map(|c| c.summary.as_str()).collect();
        assert!(summaries.contains(&"main-only"), "missing main-only");
        assert!(summaries.contains(&"dev-only"), "missing dev-only");
    }

    #[test]
    fn dedupes_commits_visible_from_multiple_branches() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();

        // Single commit reachable from main and dev.
        let shared = commit_with_time(&repo, "shared", 1_000, &[]);
        let sharedc = repo.find_commit(shared).unwrap();
        repo.branch("dev", &sharedc, false).unwrap();

        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        assert_eq!(commits.len(), 1, "shared commit should appear once");
    }

    #[test]
    fn populates_short_hash_and_repo_name() {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        let _ = commit_with_time(&repo, "only", 1_000, &[]);

        let commits = recent_commits(dir.path(), 10, 0).unwrap();
        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].short_hash.len(), 7);
        assert!(commits[0].hash.starts_with(&commits[0].short_hash));
        assert_eq!(
            commits[0].repo_name,
            dir.path().file_name().unwrap().to_string_lossy()
        );
    }
}
