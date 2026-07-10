//! Read-only Git traversal for the complete-history index.
//!
//! A scan captures every local head, remote-tracking ref, tag and detached
//! HEAD, then streams metadata in bounded batches. It never mutates a repo
//! and never treats a partial/erroring walk as complete coverage.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;

use anyhow::{Context, Result};
use git2::{Oid, Repository, Sort};
use tokio_util::sync::CancellationToken;

use crate::git::CommitSummary;

pub(crate) const DEFAULT_HISTORY_BATCH_SIZE: usize = 256;
pub(crate) const MAX_HISTORY_BATCH_SIZE: usize = 256;
const MAX_MESSAGE_BYTES: usize = 8 * 1024;
const MAX_SUMMARY_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HistoryRootKind {
    Local,
    Remote,
    Tag,
    DetachedHead,
}

impl HistoryRootKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::Tag => "tag",
            Self::DetachedHead => "detached_head",
        }
    }

    #[allow(dead_code)] // coverage/readback integration hook
    pub(crate) fn from_str(value: &str) -> Option<Self> {
        match value {
            "local" => Some(Self::Local),
            "remote" => Some(Self::Remote),
            "tag" => Some(Self::Tag),
            "detached_head" => Some(Self::DetachedHead),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryRoot {
    pub(crate) ref_name: String,
    pub(crate) oid: String,
    pub(crate) kind: HistoryRootKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RootSnapshot {
    pub(crate) roots: Vec<HistoryRoot>,
    pub(crate) fingerprint: String,
}

impl RootSnapshot {
    pub(crate) fn from_roots(mut roots: Vec<HistoryRoot>) -> Self {
        roots.sort_by(|left, right| left.ref_name.cmp(&right.ref_name));
        let fingerprint = stable_fingerprint(&roots);
        Self { roots, fingerprint }
    }
}

/// True only when the new root set can add reachability but cannot remove it.
/// Ref deletion, rename, rewind or force-push conservatively selects a full
/// mark/sweep scan.
pub(crate) fn can_extend_incrementally(
    repo_path: &Path,
    published: &RootSnapshot,
    current: &RootSnapshot,
) -> Result<bool> {
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;
    let current_by_name: HashMap<&str, &HistoryRoot> = current
        .roots
        .iter()
        .map(|root| (root.ref_name.as_str(), root))
        .collect();
    let mut ancestry: HashMap<(Oid, Oid), bool> = HashMap::new();
    for old in &published.roots {
        let Some(new) = current_by_name.get(old.ref_name.as_str()) else {
            return Ok(false);
        };
        if old.kind != new.kind {
            return Ok(false);
        }
        let old_oid = Oid::from_str(&old.oid)
            .with_context(|| format!("parse published root oid {}", old.oid))?;
        let new_oid = Oid::from_str(&new.oid)
            .with_context(|| format!("parse current root oid {}", new.oid))?;
        if old_oid == new_oid {
            continue;
        }
        let is_descendant = match ancestry.get(&(new_oid, old_oid)) {
            Some(result) => *result,
            None => {
                let result = repo.graph_descendant_of(new_oid, old_oid).unwrap_or(false);
                ancestry.insert((new_oid, old_oid), result);
                result
            }
        };
        if !is_descendant {
            return Ok(false);
        }
    }
    Ok(true)
}

#[derive(Debug, Clone)]
pub(crate) struct HistoryCommit {
    pub(crate) summary: CommitSummary,
    pub(crate) message_truncated: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct WalkStats {
    pub(crate) commits: usize,
    pub(crate) batches: usize,
    pub(crate) max_batch_len: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WalkCompletion {
    Exhausted(WalkStats),
    Cancelled(WalkStats),
}

fn stable_fingerprint(roots: &[HistoryRoot]) -> String {
    // FNV-1a is only a compact diagnostic/change key. Safe finalization also
    // compares the complete sorted root vectors, so a hash collision cannot
    // publish or delete history incorrectly.
    let mut hash = 0xcbf29ce484222325_u64;
    for root in roots {
        for bytes in [
            root.kind.as_str().as_bytes(),
            root.ref_name.as_bytes(),
            root.oid.as_bytes(),
        ] {
            for byte in bytes {
                hash ^= u64::from(*byte);
                hash = hash.wrapping_mul(0x100000001b3);
            }
            hash ^= 0xff;
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    format!("fnv1a64:{hash:016x}")
}

fn collect_refs(
    repo: &Repository,
    glob: &str,
    kind: HistoryRootKind,
    roots: &mut BTreeMap<String, HistoryRoot>,
) -> Result<()> {
    let refs = repo
        .references_glob(glob)
        .with_context(|| format!("enumerate {glob}"))?;
    for reference in refs {
        let reference = reference.with_context(|| format!("read reference matching {glob}"))?;
        let Some(name) = reference.name() else {
            continue;
        };
        // Peel tags (including annotated tags) and symbolic refs to the
        // commit they actually make reachable.
        let Ok(commit) = reference.peel_to_commit() else {
            continue;
        };
        roots.insert(
            name.to_string(),
            HistoryRoot {
                ref_name: name.to_string(),
                oid: commit.id().to_string(),
                kind,
            },
        );
    }
    Ok(())
}

/// Capture the exact reachability roots used by a full scan.
pub(crate) fn snapshot_roots(repo_path: &Path) -> Result<RootSnapshot> {
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;
    let mut roots = BTreeMap::new();
    collect_refs(&repo, "refs/heads/*", HistoryRootKind::Local, &mut roots)?;
    collect_refs(&repo, "refs/remotes/*", HistoryRootKind::Remote, &mut roots)?;
    collect_refs(&repo, "refs/tags/*", HistoryRootKind::Tag, &mut roots)?;

    // A symbolic HEAD is already covered by refs/heads/*. A detached HEAD is
    // an independent reachability root and must survive branchless agent work.
    if repo.head_detached().unwrap_or(false) {
        if let Ok(head) = repo.head() {
            if let Ok(commit) = head.peel_to_commit() {
                roots.insert(
                    "HEAD(detached)".to_string(),
                    HistoryRoot {
                        ref_name: "HEAD(detached)".to_string(),
                        oid: commit.id().to_string(),
                        kind: HistoryRootKind::DetachedHead,
                    },
                );
            }
        }
    }

    Ok(RootSnapshot::from_roots(roots.into_values().collect()))
}

fn bounded_text(text: &str, max_bytes: usize) -> (String, bool) {
    if text.len() <= max_bytes {
        return (text.to_string(), false);
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_string(), true)
}

/// Stream every commit reachable from `snapshot` in bounded owned batches.
///
/// `Exhausted` is returned only after the revwalk itself reaches EOF without
/// cancellation or an object/read error. That distinction is what allows the
/// store to make COMPLETE a strong statement rather than a best effort.
pub(crate) fn walk_history<F>(
    repo_path: &Path,
    snapshot: &RootSnapshot,
    batch_size: usize,
    cancellation: &CancellationToken,
    consume: F,
) -> Result<WalkCompletion>
where
    F: FnMut(&[HistoryCommit]) -> Result<()>,
{
    walk_history_impl(repo_path, snapshot, &[], batch_size, cancellation, consume)
}

/// Walk only reachability added since `published`. Callers must first prove
/// `can_extend_incrementally`; hidden published roots are what makes the work
/// proportional to the new commits on ordinary fast-forwards.
pub(crate) fn walk_history_delta<F>(
    repo_path: &Path,
    snapshot: &RootSnapshot,
    published: &RootSnapshot,
    batch_size: usize,
    cancellation: &CancellationToken,
    consume: F,
) -> Result<WalkCompletion>
where
    F: FnMut(&[HistoryCommit]) -> Result<()>,
{
    let hidden: Vec<Oid> = published
        .roots
        .iter()
        .map(|root| {
            Oid::from_str(&root.oid)
                .with_context(|| format!("parse published root oid {}", root.oid))
        })
        .collect::<Result<Vec<_>>>()?;
    walk_history_impl(
        repo_path,
        snapshot,
        &hidden,
        batch_size,
        cancellation,
        consume,
    )
}

fn walk_history_impl<F>(
    repo_path: &Path,
    snapshot: &RootSnapshot,
    hidden: &[Oid],
    batch_size: usize,
    cancellation: &CancellationToken,
    mut consume: F,
) -> Result<WalkCompletion>
where
    F: FnMut(&[HistoryCommit]) -> Result<()>,
{
    let repo = Repository::open(repo_path)
        .with_context(|| format!("open repo {}", repo_path.display()))?;
    if cancellation.is_cancelled() {
        return Ok(WalkCompletion::Cancelled(WalkStats::default()));
    }
    let repo_path_string = repo_path.to_string_lossy().into_owned();
    let repo_name = repo_path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let keep = batch_size.clamp(1, MAX_HISTORY_BATCH_SIZE);

    let mut tagged = HashSet::new();
    let mut remote_tips: HashMap<Oid, Vec<String>> = HashMap::new();
    for root in &snapshot.roots {
        let oid =
            Oid::from_str(&root.oid).with_context(|| format!("parse root oid {}", root.oid))?;
        match root.kind {
            HistoryRootKind::Tag => {
                tagged.insert(oid);
            }
            HistoryRootKind::Remote => {
                remote_tips.entry(oid).or_default().push(
                    root.ref_name
                        .trim_start_matches("refs/remotes/")
                        .to_string(),
                );
            }
            _ => {}
        }
    }
    for labels in remote_tips.values_mut() {
        labels.sort();
        labels.dedup();
    }

    let mut revwalk = repo.revwalk().context("init history revwalk")?;
    // set_sorting resets roots, so it must precede every push.
    revwalk
        .set_sorting(Sort::TIME)
        .context("sort history revwalk")?;
    let mut pushed = 0usize;
    for root in &snapshot.roots {
        let oid =
            Oid::from_str(&root.oid).with_context(|| format!("parse root oid {}", root.oid))?;
        revwalk
            .push(oid)
            .with_context(|| format!("push history root {}", root.ref_name))?;
        pushed += 1;
    }
    for oid in hidden {
        revwalk
            .hide(*oid)
            .with_context(|| format!("hide published history root {oid}"))?;
    }

    let mut stats = WalkStats::default();
    if pushed == 0 {
        return if cancellation.is_cancelled() {
            Ok(WalkCompletion::Cancelled(stats))
        } else {
            Ok(WalkCompletion::Exhausted(stats))
        };
    }

    let mut batch = Vec::with_capacity(keep);
    for oid in revwalk {
        if cancellation.is_cancelled() {
            return Ok(WalkCompletion::Cancelled(stats));
        }
        let oid = oid.context("walk complete history")?;
        let commit = repo
            .find_commit(oid)
            .with_context(|| format!("load history commit {oid}"))?;
        let hash = oid.to_string();
        let author = commit.author();
        let parents = commit
            .parent_ids()
            .map(|parent| parent.to_string())
            .collect();
        let message_lossy = String::from_utf8_lossy(commit.message_bytes());
        let (message, message_truncated) = bounded_text(&message_lossy, MAX_MESSAGE_BYTES);
        let summary_lossy = commit
            .summary_bytes()
            .map(String::from_utf8_lossy)
            .unwrap_or_default();
        let (summary, summary_truncated) = bounded_text(&summary_lossy, MAX_SUMMARY_BYTES);
        let remote_labels = remote_tips.get(&oid).cloned().unwrap_or_default();
        let remote_tip_label = remote_labels.first().cloned();
        let remote_tip_extra_count = remote_labels.len().saturating_sub(1);

        batch.push(HistoryCommit {
            summary: CommitSummary {
                repo_path: repo_path_string.clone(),
                repo_name: repo_name.clone(),
                short_hash: hash.chars().take(7).collect(),
                hash,
                summary,
                author: author.name().unwrap_or("").to_string(),
                email: author.email().unwrap_or("").to_string(),
                timestamp: commit.time().seconds(),
                // Branch membership is a separate, on-demand concern. A
                // complete repo-wide index must not pay O(branches x history).
                branch_label: None,
                is_merge: commit.parent_count() > 1,
                is_tagged: tagged.contains(&oid),
                parents,
                message,
                remote_tip_label,
                remote_tip_extra_count,
            },
            message_truncated: message_truncated || summary_truncated,
        });

        if batch.len() == keep {
            consume(&batch)?;
            stats.commits += batch.len();
            stats.batches += 1;
            stats.max_batch_len = stats.max_batch_len.max(batch.len());
            batch.clear();
            if cancellation.is_cancelled() {
                return Ok(WalkCompletion::Cancelled(stats));
            }
        }
    }

    if !batch.is_empty() {
        consume(&batch)?;
        stats.commits += batch.len();
        stats.batches += 1;
        stats.max_batch_len = stats.max_batch_len.max(batch.len());
    }
    if cancellation.is_cancelled() {
        Ok(WalkCompletion::Cancelled(stats))
    } else {
        Ok(WalkCompletion::Exhausted(stats))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Signature, Time};
    use tempfile::TempDir;

    fn repo_with_commits(count: usize) -> (TempDir, Repository) {
        let dir = TempDir::new().unwrap();
        let repo = Repository::init(dir.path()).unwrap();
        repo.set_head("refs/heads/main").unwrap();
        let tree_oid = {
            let mut index = repo.index().unwrap();
            index.write_tree().unwrap()
        };
        let mut parent_oid = None;
        for i in 0..count {
            let tree = repo.find_tree(tree_oid).unwrap();
            let signature = Signature::new(
                "QA Author",
                "qa@example.com",
                &Time::new(1_700_000_000 + i as i64, 0),
            )
            .unwrap();
            let oid = match parent_oid {
                Some(parent_oid) => {
                    let parent = repo.find_commit(parent_oid).unwrap();
                    repo.commit(
                        Some("HEAD"),
                        &signature,
                        &signature,
                        &format!("commit {i}"),
                        &tree,
                        &[&parent],
                    )
                    .unwrap()
                }
                None => repo
                    .commit(
                        Some("HEAD"),
                        &signature,
                        &signature,
                        &format!("commit {i}"),
                        &tree,
                        &[],
                    )
                    .unwrap(),
            };
            parent_oid = Some(oid);
        }
        (dir, repo)
    }

    fn append_commit(repo: &Repository, message: &str, timestamp: i64) -> Oid {
        let tree_oid = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        let signature =
            Signature::new("QA Author", "qa@example.com", &Time::new(timestamp, 0)).unwrap();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            message,
            &tree,
            &[&parent],
        )
        .unwrap()
    }

    #[test]
    fn walk_streams_bounded_batches_and_exhausts() {
        let (dir, _repo) = repo_with_commits(23);
        let roots = snapshot_roots(dir.path()).unwrap();
        let cancellation = CancellationToken::new();
        let mut batch_lengths = Vec::new();
        let mut hashes = HashSet::new();
        let completion = walk_history(dir.path(), &roots, 5, &cancellation, |batch| {
            batch_lengths.push(batch.len());
            hashes.extend(batch.iter().map(|commit| commit.summary.hash.clone()));
            Ok(())
        })
        .unwrap();

        assert_eq!(batch_lengths, vec![5, 5, 5, 5, 3]);
        assert_eq!(hashes.len(), 23);
        assert_eq!(
            completion,
            WalkCompletion::Exhausted(WalkStats {
                commits: 23,
                batches: 5,
                max_batch_len: 5,
            })
        );
    }

    #[test]
    fn cancellation_never_reports_exhausted() {
        let (dir, _repo) = repo_with_commits(12);
        let roots = snapshot_roots(dir.path()).unwrap();
        let cancellation = CancellationToken::new();
        let cancel_from_callback = cancellation.clone();
        let completion = walk_history(dir.path(), &roots, 4, &cancellation, move |_| {
            cancel_from_callback.cancel();
            Ok(())
        })
        .unwrap();
        assert_eq!(
            completion,
            WalkCompletion::Cancelled(WalkStats {
                commits: 4,
                batches: 1,
                max_batch_len: 4,
            })
        );
    }

    #[test]
    fn snapshot_includes_remote_tags_and_detached_head() {
        let (dir, repo) = repo_with_commits(2);
        let head = repo.head().unwrap().target().unwrap();
        repo.reference("refs/remotes/origin/main", head, true, "test")
            .unwrap();
        let target = repo.find_object(head, None).unwrap();
        repo.tag_lightweight("v1", &target, false).unwrap();
        repo.set_head_detached(head).unwrap();

        let snapshot = snapshot_roots(dir.path()).unwrap();
        assert!(snapshot.roots.iter().any(|root| {
            root.kind == HistoryRootKind::Local && root.ref_name == "refs/heads/main"
        }));
        assert!(snapshot.roots.iter().any(|root| {
            root.kind == HistoryRootKind::Remote && root.ref_name == "refs/remotes/origin/main"
        }));
        assert!(snapshot
            .roots
            .iter()
            .any(|root| { root.kind == HistoryRootKind::Tag && root.ref_name == "refs/tags/v1" }));
        assert!(snapshot
            .roots
            .iter()
            .any(|root| root.kind == HistoryRootKind::DetachedHead));
    }

    #[test]
    fn fast_forward_delta_walk_visits_only_added_commits() {
        let (dir, repo) = repo_with_commits(10);
        let published = snapshot_roots(dir.path()).unwrap();
        append_commit(&repo, "delta one", 1_700_000_100);
        append_commit(&repo, "delta two", 1_700_000_101);
        let current = snapshot_roots(dir.path()).unwrap();
        assert!(can_extend_incrementally(dir.path(), &published, &current).unwrap());

        let mut hashes = Vec::new();
        let completion = walk_history_delta(
            dir.path(),
            &current,
            &published,
            16,
            &CancellationToken::new(),
            |batch| {
                hashes.extend(batch.iter().map(|commit| commit.summary.hash.clone()));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(hashes.len(), 2);
        assert_eq!(
            completion,
            WalkCompletion::Exhausted(WalkStats {
                commits: 2,
                batches: 1,
                max_batch_len: 2,
            })
        );
    }

    #[test]
    fn ref_rewind_requires_full_scan() {
        let (dir, repo) = repo_with_commits(4);
        let published = snapshot_roots(dir.path()).unwrap();
        let mut walk = repo.revwalk().unwrap();
        walk.push_head().unwrap();
        let commits: Vec<Oid> = walk.map(Result::unwrap).collect();
        let root = *commits.last().unwrap();
        repo.reference("refs/heads/main", root, true, "test rewind")
            .unwrap();
        let current = snapshot_roots(dir.path()).unwrap();
        assert!(!can_extend_incrementally(dir.path(), &published, &current).unwrap());
    }
}
