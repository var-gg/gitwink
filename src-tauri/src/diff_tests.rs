use crate::{git_blobs, git_diff};
use git2::{Repository, Signature};

fn commit(repo: &Repository, files: &[(&str, &[u8])]) -> String {
    let mut builder = repo.treebuilder(None).unwrap();
    for (name, bytes) in files {
        builder
            .insert(name, repo.blob(bytes).unwrap(), 0o100644)
            .unwrap();
    }
    let tree = repo.find_tree(builder.write().unwrap()).unwrap();
    let sig = Signature::now("test", "test@example.com").unwrap();
    let parent = repo.head().ok().and_then(|head| head.peel_to_commit().ok());
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
fn renamed_file_preserves_old_side_and_exact_provenance() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let before = (0..100)
        .map(|i| format!("original line {i}\n"))
        .collect::<String>();
    let base = commit(&repo, &[("before.txt", before.as_bytes())]);
    let after = before.replace("original line 50\n", "edited line 50\n");
    let head = commit(&repo, &[("after.txt", after.as_bytes())]);
    let patch = git_diff::file_diff(dir.path(), &head, "after.txt", 3).unwrap();
    assert!(patch.text.contains("-original line 50"));
    assert!(patch.text.contains("+edited line 50"));
    assert!(!patch.text.contains("new file mode"));
    assert_eq!(
        (patch.old_path.as_str(), patch.new_path.as_str()),
        ("before.txt", "after.txt")
    );
    assert_eq!((patch.old_revision, patch.new_revision), (base, head));
}

#[test]
fn oversized_line_is_explicitly_incomplete_and_not_partially_emitted() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let large = "x".repeat(git_diff::MAX_DIFF_BYTES + 100);
    let head = commit(&repo, &[("large.txt", large.as_bytes())]);
    let patch = git_diff::file_diff(dir.path(), &head, "large.txt", 3).unwrap();
    assert!(patch.truncated);
    assert_eq!(patch.reason.as_deref(), Some("patch_byte_limit"));
    assert!(patch.text.len() < 1024);
}

#[test]
fn oversized_image_is_distinct_from_absent_and_other_side_still_renders() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    commit(&repo, &[("image.png", b"small image")]);
    let large = vec![1; git_blobs::MAX_BLOB_BYTES + 1];
    let head = commit(&repo, &[("image.png", &large)]);
    let blobs = git_blobs::commit_file_blobs(dir.path(), &head, "image.png", None).unwrap();
    assert_eq!(blobs.old.status, "ready");
    assert_eq!(blobs.new.status, "too_large");
    assert_eq!(blobs.new.size, Some(large.len() as u64));
    assert!(blobs.new.base64.is_none());
    let head = commit(&repo, &[]);
    assert_eq!(
        git_blobs::commit_file_blobs(dir.path(), &head, "image.png", None)
            .unwrap()
            .new
            .status,
        "absent"
    );
}

#[test]
fn unavailable_lfs_and_oversized_local_object_are_distinct() {
    let dir = tempfile::tempdir().unwrap();
    let repo = Repository::init(dir.path()).unwrap();
    let oid = "a".repeat(64);
    let pointer =
        format!("version https://git-lfs.github.com/spec/v1\noid sha256:{oid}\nsize 10\n");
    let head = commit(&repo, &[("image.png", pointer.as_bytes())]);
    assert_eq!(
        git_blobs::commit_file_blobs(dir.path(), &head, "image.png", None)
            .unwrap()
            .new
            .status,
        "lfs_missing"
    );
    let lfs = repo.path().join("lfs/objects/aa/aa");
    std::fs::create_dir_all(&lfs).unwrap();
    let file = std::fs::File::create(lfs.join(oid)).unwrap();
    file.set_len(git_blobs::MAX_BLOB_BYTES as u64 + 1).unwrap();
    assert_eq!(
        git_blobs::commit_file_blobs(dir.path(), &head, "image.png", None)
            .unwrap()
            .new
            .status,
        "too_large"
    );
}
