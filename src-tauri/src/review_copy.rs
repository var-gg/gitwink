//! One validated read batch for clipboard context, avoiding two full worktree hashes per file.
use crate::{git_diff::FilePatch, review_git, review_types::ReviewSnapshot};
use anyhow::{bail, Result};
use std::collections::{HashMap, HashSet};

pub fn patches(
    snapshot: &ReviewSnapshot,
    file_ids: &[String],
) -> Result<HashMap<String, FilePatch>> {
    if file_ids.len() > 32 {
        bail!("Copy supports at most 32 text patches");
    }
    let repo = review_git::validate(snapshot)?;
    let mut result = HashMap::new();
    let mut seen = HashSet::new();
    let mut remaining = 40 * 1024;
    for id in file_ids {
        if !seen.insert(id) {
            continue;
        }
        let mut patch = review_git::read_patch(&repo, snapshot, id, 3)?;
        if patch.text.len() > remaining {
            patch.text.clear();
            patch.truncated = true;
            patch.reason = Some("clipboard_byte_limit".into());
        } else {
            remaining -= patch.text.len();
        }
        result.insert(id.clone(), patch);
    }
    review_git::validate(snapshot)?;
    Ok(result)
}
