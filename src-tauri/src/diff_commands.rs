use crate::{cache, git};
use serde::{Deserialize, Serialize};
use std::path::Path;
use tauri::{AppHandle, Manager};

/// Phase 6 detail-tier cache: an in-memory LRU of `changed_files` results,
/// so expanding (or re-expanding) a commit doesn't recompute the git diff.
/// Bounded by entry count; the file list of a very large commit is skipped
/// (rare, and recomputing it on demand is fine). Lost on restart — it is a
/// pure cache. Managed in `lib.rs` and shared by `changed_files` +
/// `changed_files_batch`.
pub struct ChangedFilesCache(std::sync::Mutex<ChangedFilesLru>);

struct ChangedFilesLru {
    /// key (`repo_path\0hash`) → (truncated file list, TOTAL file count,
    /// last-access tick)
    entries: std::collections::HashMap<String, (Vec<git::ChangedFile>, usize, u64)>,
    tick: u64,
}

/// Max cached commits before LRU eviction kicks in.
const CHANGED_FILES_CACHE_CAP: usize = 256;
/// Ship (and cache) at most this many files per commit; the TOTAL count
/// rides along so the UI can say "showing N of M". The old behaviour —
/// refusing to cache >1000-file commits at all — made every viewport slide
/// near a vendor-bump commit recompute its full tree diff forever.
const CHANGED_FILES_LIST_CAP: usize = 1_500;
/// Upper bound on commits one `changed_files_batch` call will process.
const CHANGED_FILES_PREFETCH_CAP: usize = 100;

/// `changed_files` response: the (possibly capped) list plus the true total.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangedFilesResult {
    pub files: Vec<git::ChangedFile>,
    pub total: usize,
}

impl Default for ChangedFilesCache {
    fn default() -> Self {
        Self(std::sync::Mutex::new(ChangedFilesLru {
            entries: std::collections::HashMap::new(),
            tick: 0,
        }))
    }
}

impl ChangedFilesCache {
    fn key(repo_path: &str, hash: &str) -> String {
        format!("{repo_path}\0{hash}")
    }

    fn get(&self, repo_path: &str, hash: &str) -> Option<ChangedFilesResult> {
        let mut lru = self.0.lock().ok()?;
        lru.tick += 1;
        let tick = lru.tick;
        let entry = lru.entries.get_mut(&Self::key(repo_path, hash))?;
        entry.2 = tick;
        Some(ChangedFilesResult {
            files: entry.0.clone(),
            total: entry.1,
        })
    }

    fn contains(&self, repo_path: &str, hash: &str) -> bool {
        self.0
            .lock()
            .map(|lru| lru.entries.contains_key(&Self::key(repo_path, hash)))
            .unwrap_or(false)
    }

    /// Cache a (pre-capped) list + the commit's true total file count.
    fn put(&self, repo_path: &str, hash: &str, files: &[git::ChangedFile], total: usize) {
        let Ok(mut lru) = self.0.lock() else {
            return;
        };
        lru.tick += 1;
        let tick = lru.tick;
        lru.entries
            .insert(Self::key(repo_path, hash), (files.to_vec(), total, tick));
        // Evict least-recently-used entries down to the cap.
        while lru.entries.len() > CHANGED_FILES_CACHE_CAP {
            let victim = lru
                .entries
                .iter()
                .min_by_key(|(_, (_, _, t))| *t)
                .map(|(k, _)| k.clone());
            match victim {
                Some(k) => {
                    lru.entries.remove(&k);
                }
                None => break,
            }
        }
    }
}

/// Compute a commit's changed files and cap the shipped list, keeping the
/// true total for the "showing N of M" footer.
fn changed_files_capped(repo_path: &str, hash: &str) -> Result<ChangedFilesResult, String> {
    let mut files = git::changed_files(Path::new(repo_path), hash).map_err(|e| e.to_string())?;
    let total = files.len();
    files.truncate(CHANGED_FILES_LIST_CAP);
    Ok(ChangedFilesResult { files, total })
}

#[tauri::command]
pub async fn changed_files(
    app: AppHandle,
    repo_path: String,
    hash: String,
) -> Result<ChangedFilesResult, String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<ChangedFilesResult, String> {
        if let Some(cache) = app.try_state::<ChangedFilesCache>() {
            if let Some(hit) = cache.get(&repo_path, &hash) {
                return Ok(hit);
            }
        }
        let result = changed_files_capped(&repo_path, &hash)?;
        if let Some(cache) = app.try_state::<ChangedFilesCache>() {
            cache.put(&repo_path, &hash, &result.files, result.total);
        }
        Ok(result)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// One commit reference for `changed_files_batch`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CommitRef {
    pub repo_path: String,
    pub hash: String,
}

/// Phase 6 detail-tier prefetch: warm the `changed_files` cache for a set
/// of commits — the rows in/near the timeline viewport — so expanding one
/// is instant. Already-cached commits are skipped; the batch is capped so
/// a huge request can't run unbounded git work.
#[tauri::command]
pub async fn changed_files_batch(app: AppHandle, commits: Vec<CommitRef>) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(cache) = app.try_state::<ChangedFilesCache>() else {
            return;
        };
        for c in commits.into_iter().take(CHANGED_FILES_PREFETCH_CAP) {
            if cache.contains(&c.repo_path, &c.hash) {
                continue;
            }
            if let Ok(r) = changed_files_capped(&c.repo_path, &c.hash) {
                cache.put(&c.repo_path, &c.hash, &r.files, r.total);
            }
        }
    })
    .await
    .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn commit_file_blobs(
    repo_path: String,
    hash: String,
    file_path: String,
    old_path: Option<String>,
) -> Result<git::CommitFileBlobs, String> {
    tauri::async_runtime::spawn_blocking(move || {
        git::commit_file_blobs(
            Path::new(&repo_path),
            &hash,
            &file_path,
            old_path.as_deref(),
        )
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Default unified-diff context. Only this size is cached — it's the hot
/// path. Expanded-context requests (±25 / whole-file) are on-demand and
/// rare, so they skip the cache rather than bloating its key with the
/// context size.
const DEFAULT_DIFF_CONTEXT: u32 = 3;

#[tauri::command]
pub async fn file_diff(
    app: AppHandle,
    repo_path: String,
    hash: String,
    file_path: String,
    context_lines: u32,
) -> Result<crate::git_diff::FilePatch, String> {
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<crate::git_diff::FilePatch, String> {
        let cacheable = context_lines == DEFAULT_DIFF_CONTEXT;
        // 1. Cache hit? (default context only)
        if cacheable {
            if let Ok(conn) = cache::open(&app) {
                if let Ok(Some(text)) = cache::get_diff(&conn, &repo_path, &hash, &file_path) {
                    if let Ok(patch) = serde_json::from_str::<crate::git_diff::FilePatch>(&text) {
                        return Ok(patch);
                    }
                }
            }
        }
        // 2. Compute, persist (default context only), return.
        let text = git::file_diff(Path::new(&repo_path), &hash, &file_path, context_lines)
            .map_err(|e| e.to_string())?;
        if cacheable {
            if let Ok(conn) = cache::open(&app) {
                if let Ok(json) = serde_json::to_string(&text) {
                    let _ = cache::put_diff(&conn, &repo_path, &hash, &file_path, &json);
                }
            }
        }
        Ok(text)
    })
    .await
    .map_err(|e| e.to_string())?
}
