//! Bounded local image reads. Missing content and unavailable previews are distinct.
use anyhow::Result;
use base64::{engine::general_purpose, Engine as _};
use git2::Repository;
use serde::{Deserialize, Serialize};
use std::{io::Read, path::Path};

pub const MAX_BLOB_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlobPreview {
    pub status: String,
    pub base64: Option<String>,
    pub size: Option<u64>,
}
impl BlobPreview {
    pub(crate) fn unavailable(status: &str, size: Option<u64>) -> Self {
        Self {
            status: status.into(),
            size,
            base64: None,
        }
    }
    fn ready(bytes: &[u8]) -> Self {
        Self {
            status: "ready".into(),
            size: Some(bytes.len() as u64),
            base64: Some(general_purpose::STANDARD.encode(bytes)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitFileBlobs {
    pub old: BlobPreview,
    pub new: BlobPreview,
    pub extension: String,
}

fn load_lfs(repo: &Repository, bytes: &[u8]) -> BlobPreview {
    let text = String::from_utf8_lossy(bytes);
    let oid = text
        .lines()
        .find_map(|line| line.strip_prefix("oid sha256:"))
        .filter(|oid| oid.len() == 64 && oid.bytes().all(|b| b.is_ascii_hexdigit()));
    let size = text
        .lines()
        .find_map(|line| line.strip_prefix("size "))
        .and_then(|size| size.parse::<u64>().ok());
    if size.is_some_and(|n| n > MAX_BLOB_BYTES as u64) {
        return BlobPreview::unavailable("too_large", size);
    }
    let Some(oid) = oid else {
        return BlobPreview::unavailable("unreadable", size);
    };
    let rel = Path::new("lfs/objects")
        .join(&oid[..2])
        .join(&oid[2..4])
        .join(oid);
    let mut roots = vec![repo.path().to_path_buf()];
    if let Ok(common) = std::fs::read_to_string(repo.path().join("commondir")) {
        roots.push(repo.path().join(common.trim_end_matches(['\r', '\n'])));
    }
    for root in roots {
        match std::fs::File::open(root.join(&rel)) {
            Ok(file) => {
                let actual_size = file.metadata().ok().map(|m| m.len());
                if actual_size.is_some_and(|n| n > MAX_BLOB_BYTES as u64) {
                    return BlobPreview::unavailable("too_large", actual_size);
                }
                let mut data = Vec::new();
                if file
                    .take(MAX_BLOB_BYTES as u64 + 1)
                    .read_to_end(&mut data)
                    .is_err()
                {
                    return BlobPreview::unavailable("unreadable", actual_size);
                }
                if data.len() > MAX_BLOB_BYTES {
                    return BlobPreview::unavailable("too_large", actual_size);
                }
                return BlobPreview::ready(&data);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return BlobPreview::unavailable("unreadable", size),
        }
    }
    BlobPreview::unavailable("lfs_missing", size)
}

pub(crate) fn load_tree(
    repo: &Repository,
    tree: Option<&git2::Tree<'_>>,
    path: &str,
) -> BlobPreview {
    let Some(tree) = tree else {
        return BlobPreview::unavailable("absent", None);
    };
    let entry = match tree.get_path(Path::new(path)) {
        Ok(entry) => entry,
        Err(e) => {
            return BlobPreview::unavailable(
                if e.code() == git2::ErrorCode::NotFound {
                    "absent"
                } else {
                    "unreadable"
                },
                None,
            )
        }
    };
    let size = match repo.odb().and_then(|odb| odb.read_header(entry.id())) {
        Ok((size, _)) => size,
        Err(_) => return BlobPreview::unavailable("unreadable", None),
    };
    if size > MAX_BLOB_BYTES {
        return BlobPreview::unavailable("too_large", Some(size as u64));
    }
    match repo.find_blob(entry.id()) {
        Ok(blob)
            if blob
                .content()
                .starts_with(b"version https://git-lfs.github.com/spec/v1") =>
        {
            load_lfs(repo, blob.content())
        }
        Ok(blob) => BlobPreview::ready(blob.content()),
        Err(_) => BlobPreview::unavailable("unreadable", Some(size as u64)),
    }
}

pub fn commit_file_blobs(
    repo_path: &Path,
    hash: &str,
    path: &str,
    old_path: Option<&str>,
) -> Result<CommitFileBlobs> {
    let repo = Repository::open(repo_path)?;
    let commit = repo.find_commit(git2::Oid::from_str(hash)?)?;
    let new = commit.tree()?;
    let old = if commit.parent_count() > 0 {
        Some(commit.parent(0)?.tree()?)
    } else {
        None
    };
    Ok(CommitFileBlobs {
        old: load_tree(&repo, old.as_ref(), old_path.unwrap_or(path)),
        new: load_tree(&repo, Some(&new), path),
        extension: Path::new(path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase(),
    })
}
