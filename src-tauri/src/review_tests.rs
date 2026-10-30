use crate::{review_git, review_store, review_types::*};
use git2::{Repository, Signature};
use std::path::Path;

fn commit(repo: &Repository, name: &str, text: &str) -> String {
    std::fs::write(repo.workdir().unwrap().join(name), text).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new(name)).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = Signature::now("test", "test@example.com").unwrap();
    let parent = repo.head().ok().and_then(|r| r.peel_to_commit().ok());
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        "fixture",
        &tree,
        &parent.iter().collect::<Vec<_>>(),
    )
    .unwrap()
    .to_string()
}

#[test]
fn range_is_net_change_and_saved_baseline_uses_exact_commits() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let base = commit(&repo, "one.txt", "before\n");
    commit(&repo, "one.txt", "temporary\n");
    let head = commit(&repo, "one.txt", "after\n");
    let snapshot = review_git::snapshot(
        &ReviewRequest::Range {
            repo_path: dir.path().to_string_lossy().into(),
            base: base.clone(),
            head: "HEAD".into(),
        },
        None,
    )
    .unwrap();
    let patch = review_git::patch(&snapshot, &snapshot.files[0].id, 3).unwrap();
    assert!(patch.text.contains("-before"));
    assert!(patch.text.contains("+after"));
    assert!(!patch.text.contains("temporary"));
    assert_eq!(snapshot.head.as_deref(), Some(head.as_str()));
    assert!(snapshot.can_mark_reviewed);
    let baseline = ReviewBaseline {
        identity: snapshot.identity,
        head: base,
    };
    let since = review_git::snapshot(
        &ReviewRequest::SinceReviewed {
            repo_path: snapshot.repo_path,
        },
        Some(baseline),
    )
    .unwrap();
    assert_eq!(since.files.len(), 1);
    assert_eq!(since.baseline_status, "ready");
}

#[test]
fn current_changes_keep_three_groups_and_never_write_git_state() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    commit(&repo, "both.txt", "committed\n");
    std::fs::write(dir.path().join("both.txt"), "staged\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("both.txt")).unwrap();
    index.write().unwrap();
    std::fs::write(dir.path().join("both.txt"), "unstaged\n").unwrap();
    std::fs::write(dir.path().join("new.txt"), "untracked\n").unwrap();
    let index_before = std::fs::read(repo.path().join("index")).unwrap();
    let head_before = review_git::head(&repo);
    let snapshot = review_git::snapshot(
        &ReviewRequest::Worktree {
            repo_path: dir.path().to_string_lossy().into(),
        },
        None,
    )
    .unwrap();
    assert_eq!(snapshot.files.len(), 3);
    for (group, before, after) in [
        ("staged", "committed", "staged"),
        ("unstaged", "staged", "unstaged"),
        ("untracked", "", "untracked"),
    ] {
        let file = snapshot.files.iter().find(|f| f.group == group).unwrap();
        let patch = review_git::patch(&snapshot, &file.id, 3).unwrap();
        if !before.is_empty() {
            assert!(patch.text.contains(&format!("-{before}")));
        }
        assert!(
            patch.text.contains(&format!("+{after}")),
            "{group}: {}",
            patch.text
        );
    }
    assert!(!snapshot.can_mark_reviewed);
    assert_eq!(
        std::fs::read(repo.path().join("index")).unwrap(),
        index_before
    );
    assert_eq!(review_git::head(&repo), head_before);
    assert_eq!(
        std::fs::read_to_string(dir.path().join("both.txt")).unwrap(),
        "unstaged\n"
    );
    std::fs::write(dir.path().join("both.txt"), "concurrent edit\n").unwrap();
    assert!(review_git::patch(&snapshot, &snapshot.files[0].id, 3).is_err());
}

#[test]
fn unborn_repository_exposes_untracked_and_staged_changes() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    std::fs::write(dir.path().join("first.txt"), "first\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("first.txt")).unwrap();
    index.write().unwrap();
    let snapshot = review_git::snapshot(
        &ReviewRequest::Worktree {
            repo_path: dir.path().to_string_lossy().into(),
        },
        None,
    )
    .unwrap();
    assert!(snapshot.head.is_none());
    assert_eq!(snapshot.files[0].group, "staged");
    assert!(review_git::patch(&snapshot, &snapshot.files[0].id, 3)
        .unwrap()
        .text
        .contains("+first"));
}

#[test]
fn stale_head_and_rewritten_baseline_require_explicit_new_review() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let base = commit(&repo, "one.txt", "before\n");
    let request = ReviewRequest::SinceReviewed {
        repo_path: dir.path().to_string_lossy().into(),
    };
    let snapshot = review_git::snapshot(&request, None).unwrap();
    assert_eq!(snapshot.baseline_status, "missing");
    assert!(snapshot.files.is_empty());
    let head = commit(&repo, "one.txt", "after\n");
    assert!(review_git::validate(&snapshot).is_err());
    let baseline = ReviewBaseline {
        identity: snapshot.identity,
        head,
    };
    repo.head()
        .unwrap()
        .set_target(git2::Oid::from_str(&base).unwrap(), "fixture rewind")
        .unwrap();
    let rewound = review_git::snapshot(&request, Some(baseline)).unwrap();
    assert_eq!(rewound.baseline_status, "diverged");
    assert!(!rewound.can_mark_reviewed);
}

#[test]
fn baseline_persistence_is_scoped_and_compare_and_swap_guarded() {
    let mut conn = rusqlite::Connection::open_in_memory().unwrap();
    review_store::initialize(&conn).unwrap();
    let first = ReviewBaseline {
        identity: "physical-repo".into(),
        head: "commit-a".into(),
    };
    review_store::save(&mut conn, "repo", "main", None, &first).unwrap();
    assert_eq!(
        review_store::load(&conn, "repo", "main").unwrap(),
        Some(first.clone())
    );
    assert!(review_store::load(&conn, "repo", "other-branch")
        .unwrap()
        .is_none());
    assert!(review_store::load(&conn, "other-worktree", "main")
        .unwrap()
        .is_none());
    let next = ReviewBaseline {
        identity: first.identity.clone(),
        head: "commit-b".into(),
    };
    assert!(review_store::save(&mut conn, "repo", "main", None, &next).is_err());
    review_store::save(&mut conn, "repo", "main", Some(&first), &next).unwrap();
}

#[test]
fn clipboard_batch_is_bounded_and_rejects_files_outside_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let base = commit(&repo, "large.txt", "before\n");
    let head = commit(&repo, "large.txt", &"x".repeat(50 * 1024));
    let snapshot = review_git::snapshot(
        &ReviewRequest::Range {
            repo_path: dir.path().to_string_lossy().into(),
            base,
            head,
        },
        None,
    )
    .unwrap();
    let id = snapshot.files[0].id.clone();
    let result = crate::review_copy::patches(&snapshot, &[id.clone()]).unwrap();
    assert!(result[&id].truncated);
    assert_eq!(result[&id].reason.as_deref(), Some("clipboard_byte_limit"));
    assert!(result[&id].text.is_empty());
    assert!(crate::review_copy::patches(&snapshot, &["untracked:outside".into()]).is_err());
    assert!(crate::review_copy::patches(&snapshot, &vec![id; 33]).is_err());
}
