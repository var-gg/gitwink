// Multi-tier discovery orchestrator.
//
// Runs as a background task spawned in setup(). Pulls candidates from
// every available source (cache, VS Code family, git config, filesystem
// walk, learned roots), deduplicates, validates with git2::Repository
// ::discover, and upserts to cache. Learns parent roots from validated
// repos so the next run finds siblings.
//
// Commit 3 scope: fill the cache silently. UI still sees repos through
// the existing `list_repos` / `discover_repos` paths. Commit 4 wires
// event emission and a richer event model.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use rusqlite::{params, Connection};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

use crate::cache;
use crate::discovery;
use crate::discovery_sources::{
    self, Candidate, DiscoverySource, GitConfigHint,
};

/// Payload mirroring `cache::Repo` plus the source the orchestrator
/// learned the repo from. Frontend uses `name` + `path` directly and
/// `source` only for debug tooltips. camelCase serialization matches
/// existing event payloads.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoveredRepoPayload {
    pub path: String,
    pub name: String,
    pub source: &'static str,
    pub confidence: i32,
}

/// Streamed every time the validation loop hits a milestone or finishes.
/// `state` is "scanning" while the pipeline is alive and "complete" once
/// the final scan.log line is written.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanProgressPayload {
    pub repos_found: usize,
    pub state: &'static str,
    pub errors: usize,
}

/// Repo lifecycle transition (active → missing or vice versa). Frontend
/// uses this to grey out a row that moved/disappeared on disk, or to
/// restore a row that came back.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoStatusPayload {
    pub canonical_path: String,
    pub status: &'static str,
}

/// Meta-table key that tracks whether the first-launch full scan has
/// already completed. Lives in cache.db (not settings.json) so wiping
/// the cache correctly re-triggers the full scan.
pub const META_INITIAL_SCAN_COMPLETED: &str = "initial_scan_completed";

/// Hard cap on how long the synchronous portion of VS Code recents
/// reading is allowed to take. The orchestrator runs this in a
/// spawn_blocking, but we still don't want to drag a giant Cursor DB
/// past this — the deadline propagates into each read.
const VSCODE_RECENTS_DEADLINE_MS: u64 = 2_000;

/// Max candidates we'll validate per run. Validation is git2::open per
/// candidate; an unbounded list (e.g. JetBrains user with 1000 recents)
/// could otherwise pin a CPU core.
const VALIDATION_CAP: usize = 400;

/// scan.log size cap before rotation. Append-only file in app data dir.
const SCAN_LOG_MAX_BYTES: u64 = 1 * 1024 * 1024;

/// Top N candidates whose parent dir gets learned as a high-confidence
/// scan root. Without this cap a user with 200 IDE recents in
/// different parent dirs would seed too many learned roots.
const PARENT_LEARNING_CAP: usize = 64;

/// Validated repo as the orchestrator sees it after `git2::Repository::discover`.
#[derive(Debug, Clone)]
struct ValidatedRepo {
    /// Path as the candidate source observed it (used for path_aliases).
    observed_path: String,
    /// Canonicalized workdir (primary key in the repos table).
    canonical_path: String,
    /// `.git` directory path.
    gitdir_path: String,
    /// Display name = leaf folder name of canonical_path.
    name: String,
}

/// Owned handle to the running orchestrator. Holding it lets the app
/// cancel the task on exit if needed.
pub struct OrchestratorHandle {
    cancel: CancellationToken,
}

impl OrchestratorHandle {
    #[allow(dead_code)]
    pub fn cancel(&self) {
        self.cancel.cancel();
    }
}

/// Spin up the discovery prewarm task. Fire-and-forget: the task runs
/// to completion or until cancelled, logging summary lines to scan.log.
pub fn start(app: AppHandle) -> OrchestratorHandle {
    let cancel = CancellationToken::new();
    let token_clone = cancel.clone();
    let app_clone = app.clone();

    tauri::async_runtime::spawn(async move {
        if let Err(e) = run_discovery_async(app_clone, token_clone).await {
            eprintln!("discovery orchestrator: {e:#}");
        }
    });

    OrchestratorHandle { cancel }
}

/// Manually add a repo by path (drag-drop / paste / future file picker).
/// Validates with git2 (will walk up if the user dropped a subdirectory
/// of a repo), upserts with primary_source='manual' + user_state='pinned'
/// + confidence 100 so it stays at the top of the paint order, learns
/// parent roots, and emits `timeline://repo-discovered` so the frontend
/// can append a row immediately.
///
/// Returns the discovered repo payload on success, or an error string
/// the frontend can surface inline (e.g. "Not a Git working tree").
pub fn add_repo_explicit(app: &AppHandle, raw_path: &str) -> Result<DiscoveredRepoPayload> {
    let path = PathBuf::from(raw_path.trim());
    if path.as_os_str().is_empty() {
        return Err(anyhow::anyhow!("empty path"));
    }
    let validated = validate_repo_candidate(&path)
        .ok_or_else(|| anyhow::anyhow!("Not a Git working tree"))?;

    let mut conn = cache::open(app)?;
    let candidate = Candidate {
        path: PathBuf::from(&validated.canonical_path),
        source: DiscoverySource::Manual,
        confidence: 100,
        raw_hint: Some(format!("manual:{raw_path}")),
    };
    let now = unix_now();
    {
        let tx = conn.transaction()?;
        upsert_discovered_repo(&tx, &validated, &candidate, now)?;
        // Manual add overrides any pre-existing 'normal' user_state and
        // promotes the repo to 'pinned' so it survives a tombstone-style
        // wipe and stays at the top of paint order.
        tx.execute(
            "UPDATE repos SET user_state = 'pinned' WHERE canonical_path = ?1",
            params![&validated.canonical_path],
        )?;
        record_repo_source(&tx, &validated, &candidate, now)?;
        record_path_alias(&tx, &validated, &candidate, now)?;
        for learned in learn_roots_from_repo(&validated, DiscoverySource::Manual) {
            upsert_discovery_root(&tx, &learned, &validated.canonical_path, now)?;
        }
        // Manual-added paths shouldn't stay on the tombstone list — the
        // user explicitly asked for them back.
        tx.execute(
            "DELETE FROM discovery_tombstones WHERE canonical_path = ?1",
            params![&validated.canonical_path],
        )?;
        tx.commit()?;
    }

    let payload = DiscoveredRepoPayload {
        path: validated.canonical_path.clone(),
        name: validated.name.clone(),
        source: DiscoverySource::Manual.as_str(),
        confidence: 100,
    };
    let _ = app.emit("timeline://repo-discovered", &payload);

    let _ = append_scan_log(
        app,
        &format!(
            "{} manual_add: path={} name={}",
            now, validated.canonical_path, validated.name
        ),
    );

    Ok(payload)
}

async fn run_discovery_async(app: AppHandle, cancel: CancellationToken) -> Result<()> {
    let app_for_blocking = app.clone();
    let cancel_for_blocking = cancel.clone();

    let join = tauri::async_runtime::spawn_blocking(move || -> Result<(usize, usize, bool)> {
        let mut conn = cache::open(&app_for_blocking)?;
        let is_first_run = cache::meta_get(&conn, META_INITIAL_SCAN_COMPLETED)?
            .as_deref()
            != Some("true");
        let summary = run_pipeline_sync(&app_for_blocking, &mut conn, cancel_for_blocking.clone(), is_first_run)?;

        // Only mark the first-run flag if the scan actually completed
        // (didn't get cancelled mid-flight) — otherwise the next launch
        // would skip the full sweep with an incomplete cache.
        if is_first_run && !cancel_for_blocking.is_cancelled() {
            cache::meta_set(&conn, META_INITIAL_SCAN_COMPLETED, "true")?;
        }
        Ok((summary.0, summary.1, is_first_run))
    })
    .await;

    match join {
        Ok(Ok((seen, valid, first))) => {
            let line = format!(
                "{} discovery: seen={} valid={} first_run={} cancelled={}",
                unix_now(),
                seen,
                valid,
                first,
                cancel.is_cancelled()
            );
            let _ = append_scan_log(&app, &line);
            Ok(())
        }
        Ok(Err(e)) => Err(e),
        Err(e) => Err(anyhow::anyhow!("orchestrator join error: {e}")),
    }
}

/// Walk active rows, check path existence, flip status accordingly.
/// Emits one `timeline://repo-status` event per row that changed state
/// so the frontend can grey out / restore individual rows without a
/// full reload. Read-only: never deletes a row even if it's been gone
/// forever — that's the user's call via hide/relink.
fn verify_existing_repos(app: &AppHandle, conn: &mut Connection) -> Result<()> {
    let mut stmt = conn.prepare(
        r#"
        SELECT canonical_path, status
        FROM repos
        WHERE user_state NOT IN ('removed')
          AND status IN ('active', 'missing')
        "#,
    )?;
    let rows: Vec<(String, String)> = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    drop(stmt);

    let now = unix_now();
    let mut transitions: Vec<(String, &'static str)> = Vec::new();
    let tx = conn.transaction()?;
    for (path, status) in rows {
        let exists = Path::new(&path).is_dir();
        let new_status: &'static str = if exists { "active" } else { "missing" };
        if status == new_status {
            continue;
        }
        if new_status == "missing" {
            tx.execute(
                "UPDATE repos SET status = 'missing', missing_since = ?1 WHERE canonical_path = ?2",
                params![now, &path],
            )?;
        } else {
            tx.execute(
                "UPDATE repos SET status = 'active', missing_since = NULL, last_verified_at = ?1 WHERE canonical_path = ?2",
                params![now, &path],
            )?;
        }
        transitions.push((path, new_status));
    }
    tx.commit()?;

    for (path, status) in transitions {
        let _ = app.emit(
            "timeline://repo-status",
            &RepoStatusPayload {
                canonical_path: path,
                status,
            },
        );
    }
    Ok(())
}

/// Mark a repo as user-hidden. Sets user_state='removed' (so UI filters
/// it out) and adds a tombstone (so future discovery passes don't auto-
/// rediscover it). Returns the canonical path that was hidden, or an
/// error if no such repo exists in cache.
pub fn hide_repo(app: &AppHandle, canonical_path: &str) -> Result<()> {
    let mut conn = cache::open(app)?;
    let now = unix_now();
    let tx = conn.transaction()?;
    let updated = tx.execute(
        "UPDATE repos SET user_state = 'removed', removed_at = ?1 WHERE canonical_path = ?2",
        params![now, canonical_path],
    )?;
    if updated == 0 {
        return Err(anyhow::anyhow!("no such repo: {canonical_path}"));
    }
    tx.execute(
        r#"
        INSERT INTO discovery_tombstones (canonical_path, removed_at, reason)
        VALUES (?1, ?2, 'user_hide')
        ON CONFLICT(canonical_path) DO UPDATE SET
            removed_at = excluded.removed_at,
            reason = excluded.reason
        "#,
        params![canonical_path, now],
    )?;
    tx.commit()?;

    let _ = app.emit(
        "timeline://repo-status",
        &RepoStatusPayload {
            canonical_path: canonical_path.to_string(),
            status: "removed",
        },
    );
    let _ = append_scan_log(app, &format!("{now} user_hide: {canonical_path}"));
    Ok(())
}

/// Update the tray tooltip to reflect the current scan state. Best-
/// effort: tray might not exist yet or the tooltip API might fail on
/// some Linux distros — log to stderr if so but don't propagate.
fn set_tray_tooltip(app: &AppHandle, text: &str) {
    if let Some(tray) = app.tray_by_id("main") {
        if let Err(e) = tray.set_tooltip(Some(text)) {
            eprintln!("tray tooltip update failed: {e}");
        }
    }
}

/// Format the tooltip exactly as the spec requires.
fn tooltip_text(repos_found: usize, state: &str, errors: usize) -> String {
    match (state, errors) {
        ("scanning", _) => format!("gitwink — scanning... {repos_found} found"),
        ("complete", 0) => format!("gitwink — {repos_found} repositories"),
        ("complete", n) => format!("gitwink — {repos_found} repositories, {n} issues"),
        _ => "gitwink".to_string(),
    }
}

/// Pull candidates from every tier, dedup, validate, upsert. Returns
/// (candidates_seen, candidates_valid).
fn run_pipeline_sync(
    app: &AppHandle,
    conn: &mut Connection,
    cancel: CancellationToken,
    is_first_run: bool,
) -> Result<(usize, usize)> {
    // Verification sweep: check active cached repos for path existence
    // BEFORE doing new discovery. Repos that disappeared on disk get
    // marked 'missing' so the UI can grey them out. Repos that came
    // back from 'missing' get restored. Path existence is cheap (no
    // git2 here) — full re-validation happens when a candidate comes
    // through the regular tier flow.
    if let Err(e) = verify_existing_repos(app, conn) {
        eprintln!("orchestrator verification sweep failed: {e:#}");
    }

    let mut candidates: Vec<Candidate> = Vec::new();

    // Tier 1: VS Code-family recents (VS Code / Insiders / Cursor / Windsurf).
    let recents_deadline = Instant::now() + Duration::from_millis(VSCODE_RECENTS_DEADLINE_MS);
    for (source, primary, _fallback) in discovery_sources::vscode_family_db_paths() {
        if cancel.is_cancelled() {
            return Ok((candidates.len(), 0));
        }
        candidates.extend(discovery_sources::read_vscode_recents(
            &primary,
            source,
            recents_deadline,
        ));
    }

    // Tier 1b: expand any .code-workspace candidates into folder entries.
    // Iterate over a snapshot — we mutate `candidates` while reading it.
    let workspaces: Vec<PathBuf> = candidates
        .iter()
        .filter(|c| c.source == DiscoverySource::CodeWorkspace)
        .map(|c| c.path.clone())
        .collect();
    for ws_path in workspaces {
        if cancel.is_cancelled() {
            return Ok((candidates.len(), 0));
        }
        candidates.extend(discovery_sources::expand_code_workspace(&ws_path));
    }

    // Tier 2: git config hints (safe.directory; includeIf root patterns
    // are noted but Tier 4 expansion of them is Phase 2).
    for hint in discovery_sources::read_git_config_hints() {
        if let GitConfigHint::RepoPath(path) = hint {
            candidates.push(Candidate {
                path,
                source: DiscoverySource::GitConfigSafe,
                confidence: 85,
                raw_hint: None,
            });
        }
    }

    // Tier 4: learned roots from previous runs. Read-only here; the
    // learning loop happens after we validate.
    let learned_roots = load_learned_roots(conn)?;
    for root in &learned_roots {
        if cancel.is_cancelled() {
            return Ok((candidates.len(), 0));
        }
        if !root.root_path.is_dir() {
            continue;
        }
        let mut acc: Vec<PathBuf> = Vec::new();
        discovery::scan_path(&root.root_path, |p| acc.push(p));
        for p in acc {
            candidates.push(Candidate {
                path: p,
                source: DiscoverySource::FsWalk,
                confidence: 55,
                raw_hint: Some(format!("learned:{}", root.root_path.display())),
            });
        }
    }

    // Tier 5: default filesystem fallback. Heavy — only on first run.
    // Subsequent runs rely on cache + IDE recents + learned roots +
    // watcher events to surface new repos.
    if is_first_run {
        for root in discovery::default_roots() {
            if cancel.is_cancelled() {
                return Ok((candidates.len(), 0));
            }
            let mut acc: Vec<PathBuf> = Vec::new();
            discovery::scan_path(&root, |p| acc.push(p));
            for p in acc {
                candidates.push(Candidate {
                    path: p,
                    source: DiscoverySource::FsWalk,
                    confidence: 46,
                    raw_hint: Some(format!("default-root:{}", root.display())),
                });
            }
        }
    }

    let seen = candidates.len();
    let dedup = dedup_candidates(candidates);

    // Tombstones: skip any path the user explicitly hid.
    let tombstoned = load_tombstones(conn)?;
    let to_validate: Vec<Candidate> = dedup
        .into_iter()
        .filter(|c| {
            let canon = canonicalize_lossy(&c.path);
            !tombstoned.contains(&canon)
        })
        .take(VALIDATION_CAP)
        .collect();

    let now = unix_now();
    let mut valid = 0usize;
    let mut parent_learn_count = 0usize;
    let mut last_tooltip_at = Instant::now() - Duration::from_secs(1);
    let mut emit_buffer: Vec<DiscoveredRepoPayload> = Vec::new();

    set_tray_tooltip(app, &tooltip_text(0, "scanning", 0));
    emit_scan_progress(app, 0, "scanning", 0);

    {
        let tx = conn.transaction()?;
        for candidate in to_validate {
            if cancel.is_cancelled() {
                break;
            }
            let Some(validated) = validate_repo_candidate(&candidate.path) else {
                continue;
            };
            upsert_discovered_repo(&tx, &validated, &candidate, now)?;
            record_repo_source(&tx, &validated, &candidate, now)?;
            record_path_alias(&tx, &validated, &candidate, now)?;

            if parent_learn_count < PARENT_LEARNING_CAP {
                for learned in learn_roots_from_repo(&validated, candidate.source) {
                    upsert_discovery_root(&tx, &learned, &validated.canonical_path, now)?;
                    parent_learn_count += 1;
                }
            }
            valid += 1;

            // Stage the payload — we emit it AFTER the transaction
            // commits so listeners never see a row that might roll back.
            emit_buffer.push(DiscoveredRepoPayload {
                path: validated.canonical_path.clone(),
                name: validated.name.clone(),
                source: candidate.source.as_str(),
                confidence: candidate.confidence,
            });

            // Tooltip + progress throttle: spec says max one update per
            // 500ms so a fast scan doesn't thrash the tray. The last
            // tick of the scan is emitted explicitly outside the loop.
            if last_tooltip_at.elapsed() >= Duration::from_millis(500) {
                set_tray_tooltip(app, &tooltip_text(valid, "scanning", 0));
                emit_scan_progress(app, valid, "scanning", 0);
                last_tooltip_at = Instant::now();
            }
        }
        tx.commit()?;
    }

    // Flush per-repo events AFTER commit. Frontend can append these to
    // its row list — the cache row they reference is now durable.
    for payload in emit_buffer {
        let _ = app.emit("timeline://repo-discovered", &payload);
    }

    // Final tick.
    set_tray_tooltip(app, &tooltip_text(valid, "complete", 0));
    emit_scan_progress(app, valid, "complete", 0);

    Ok((seen, valid))
}

fn emit_scan_progress(app: &AppHandle, repos_found: usize, state: &'static str, errors: usize) {
    let _ = app.emit(
        "scan-progress",
        &ScanProgressPayload {
            repos_found,
            state,
            errors,
        },
    );
}

// ---------------------------------------------------------------------------
// Candidate dedup
// ---------------------------------------------------------------------------

/// Best-effort canonical key for dedup BEFORE git2 validation. Uses
/// `std::fs::canonicalize` if possible, falls back to the raw path with
/// lowercased ASCII drive letter on Windows. Later, validated repos
/// dedupe by the true git workdir canonical path; this is just to keep
/// the pre-validation set small.
fn canonicalize_lossy(path: &Path) -> String {
    if let Ok(canon) = std::fs::canonicalize(path) {
        return canon.to_string_lossy().into_owned();
    }
    let s = path.to_string_lossy();
    #[cfg(target_os = "windows")]
    {
        let mut chars: Vec<char> = s.chars().collect();
        if chars.len() >= 2 && chars[1] == ':' {
            chars[0] = chars[0].to_ascii_lowercase();
        }
        return chars.into_iter().collect::<String>().replace('/', "\\");
    }
    #[cfg(not(target_os = "windows"))]
    {
        s.into_owned()
    }
}

/// Dedupe candidates by best-effort canonical key, keeping the highest-
/// confidence one and merging the source list (the higher one wins).
fn dedup_candidates(mut input: Vec<Candidate>) -> Vec<Candidate> {
    input.sort_by(|a, b| b.confidence.cmp(&a.confidence));
    let mut seen: HashMap<String, Candidate> = HashMap::new();
    for c in input {
        let key = canonicalize_lossy(&c.path);
        seen.entry(key).or_insert(c);
    }
    seen.into_values().collect()
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Validate a candidate path with git2 (read-only). Returns None for
/// non-repos, bare repos (v0.1 viewer scope), and unreadable paths.
fn validate_repo_candidate(candidate_path: &Path) -> Option<ValidatedRepo> {
    let probe = if candidate_path.is_file() {
        candidate_path.parent().unwrap_or(candidate_path)
    } else {
        candidate_path
    };

    let repo = git2::Repository::discover(probe).ok()?;
    if repo.is_bare() {
        return None;
    }
    let workdir = repo.workdir()?;
    let canonical = strip_unc_prefix(
        std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf()),
    );
    let gitdir = strip_unc_prefix(repo.path().to_path_buf());

    let name = canonical
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo")
        .to_string();

    Some(ValidatedRepo {
        observed_path: candidate_path.to_string_lossy().into_owned(),
        canonical_path: canonical.to_string_lossy().into_owned(),
        gitdir_path: gitdir.to_string_lossy().into_owned(),
        name,
    })
}

/// Strip Windows extended-length path prefix (`\\?\`) for storage. We
/// keep using it transiently for OS calls that need >MAX_PATH support,
/// but storing it as the canonical_path key creates phantom duplicates
/// against rows that were written before the orchestrator existed (or
/// by any path that didn't go through `fs::canonicalize`). True UNC
/// extended-length paths (`\\?\UNC\server\share\…`) are left alone —
/// stripping their prefix would lose the UNC discriminator.
fn strip_unc_prefix(p: PathBuf) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        let s = p.to_string_lossy();
        if let Some(rest) = s.strip_prefix(r"\\?\") {
            if !rest.starts_with("UNC\\") {
                return PathBuf::from(rest);
            }
        }
    }
    p
}

// ---------------------------------------------------------------------------
// Cache upsert
// ---------------------------------------------------------------------------

fn upsert_discovered_repo(
    tx: &rusqlite::Transaction,
    repo: &ValidatedRepo,
    candidate: &Candidate,
    now: i64,
) -> Result<()> {
    // First, try inserting fresh. If a row with the same canonical_path
    // already exists, the conflict path updates last_seen / last_verified
    // and bumps confidence ONLY if the new source is more confident.
    tx.execute(
        r#"
        INSERT INTO repos (
            path, name, discovered_at, last_seen_at,
            canonical_path, gitdir_path,
            status, user_state, primary_source, confidence,
            first_seen_at, last_verified_at, repo_kind
        )
        VALUES (?1, ?2, ?3, ?3, ?4, ?5, 'active', 'normal', ?6, ?7, ?3, ?3, 'workdir')
        ON CONFLICT(canonical_path) DO UPDATE SET
            path = excluded.path,
            name = excluded.name,
            gitdir_path = excluded.gitdir_path,
            status = 'active',
            missing_since = NULL,
            last_seen_at = excluded.last_seen_at,
            last_verified_at = excluded.last_verified_at,
            primary_source = CASE
                WHEN excluded.confidence > repos.confidence THEN excluded.primary_source
                ELSE repos.primary_source
            END,
            confidence = MAX(repos.confidence, excluded.confidence)
        "#,
        params![
            &repo.canonical_path,
            &repo.name,
            now,
            &repo.canonical_path,
            &repo.gitdir_path,
            candidate.source.as_str(),
            candidate.confidence as i64,
        ],
    )?;
    Ok(())
}

fn record_repo_source(
    tx: &rusqlite::Transaction,
    repo: &ValidatedRepo,
    candidate: &Candidate,
    now: i64,
) -> Result<()> {
    tx.execute(
        r#"
        INSERT INTO repo_sources (
            repo_canonical_path, source, source_path, raw_hint,
            confidence, first_seen_at, last_seen_at, last_success_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?6)
        ON CONFLICT(repo_canonical_path, source) DO UPDATE SET
            source_path = excluded.source_path,
            raw_hint = excluded.raw_hint,
            confidence = MAX(repo_sources.confidence, excluded.confidence),
            last_seen_at = excluded.last_seen_at,
            last_success_at = excluded.last_success_at
        "#,
        params![
            &repo.canonical_path,
            candidate.source.as_str(),
            &repo.observed_path,
            &candidate.raw_hint,
            candidate.confidence as i64,
            now,
        ],
    )?;
    Ok(())
}

fn record_path_alias(
    tx: &rusqlite::Transaction,
    repo: &ValidatedRepo,
    candidate: &Candidate,
    now: i64,
) -> Result<()> {
    if repo.observed_path == repo.canonical_path {
        return Ok(());
    }
    tx.execute(
        r#"
        INSERT INTO path_aliases (
            observed_path, canonical_path, source, first_seen_at, last_seen_at
        )
        VALUES (?1, ?2, ?3, ?4, ?4)
        ON CONFLICT(observed_path, source) DO UPDATE SET
            canonical_path = excluded.canonical_path,
            last_seen_at = excluded.last_seen_at
        "#,
        params![
            &repo.observed_path,
            &repo.canonical_path,
            candidate.source.as_str(),
            now,
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Learned roots
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct LearnedRoot {
    root_path: PathBuf,
    #[allow(dead_code)]
    score: f64,
    max_depth: usize,
    entry_budget: usize,
}

#[derive(Debug, Clone, Copy)]
enum LearnedKind {
    ManualParent,
    IdeParent,
    LearnedSibling,
}

impl LearnedKind {
    fn as_str(self) -> &'static str {
        match self {
            LearnedKind::ManualParent => "manual_parent",
            LearnedKind::IdeParent => "ide_parent",
            LearnedKind::LearnedSibling => "learned_sibling",
        }
    }
}

#[derive(Debug, Clone)]
struct DiscoveryRootRow {
    kind: LearnedKind,
    root_path: PathBuf,
    score: f64,
    max_depth: usize,
    entry_budget: usize,
}

/// Walk up from a validated repo and propose parent / grandparent dirs
/// as learned scan roots. Refuses to learn drive roots or home dirs.
fn learn_roots_from_repo(repo: &ValidatedRepo, source: DiscoverySource) -> Vec<DiscoveryRootRow> {
    let mut out = Vec::new();
    let repo_path = Path::new(&repo.canonical_path);

    let Some(parent1) = repo_path.parent() else {
        return out;
    };
    let manual = matches!(source, DiscoverySource::Manual);

    if is_safe_learning_root(parent1) {
        out.push(DiscoveryRootRow {
            kind: if manual {
                LearnedKind::ManualParent
            } else {
                LearnedKind::IdeParent
            },
            root_path: parent1.to_path_buf(),
            score: if manual { 0.98 } else { 0.90 },
            max_depth: 3,
            entry_budget: 2_500,
        });
    }

    if let Some(parent2) = parent1.parent() {
        if is_safe_learning_root(parent2) && !is_drive_or_home_root(parent2) {
            out.push(DiscoveryRootRow {
                kind: LearnedKind::LearnedSibling,
                root_path: parent2.to_path_buf(),
                score: if manual { 0.72 } else { 0.60 },
                max_depth: 4,
                entry_budget: 5_000,
            });
        }
    }

    out
}

fn is_safe_learning_root(path: &Path) -> bool {
    if is_drive_or_home_root(path) {
        return false;
    }
    let s = path.to_string_lossy().to_ascii_lowercase();
    let banned = [
        "\\windows",
        "\\program files",
        "\\program files (x86)",
        "\\appdata",
        "/library",
        "/system",
        "/applications",
        "node_modules",
        "/target",
        "\\target",
    ];
    !banned.iter().any(|b| s.contains(b))
}

fn is_drive_or_home_root(path: &Path) -> bool {
    if path.parent().is_none() {
        return true;
    }
    if let Some(home) = home_dir() {
        if path == home {
            return true;
        }
    }
    false
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

fn upsert_discovery_root(
    tx: &rusqlite::Transaction,
    row: &DiscoveryRootRow,
    created_from: &str,
    now: i64,
) -> Result<()> {
    tx.execute(
        r#"
        INSERT INTO discovery_roots (
            root_path, root_kind, created_from_repo,
            score, max_depth, entry_budget,
            enabled, created_at, updated_at
        )
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?7)
        ON CONFLICT(root_path) DO UPDATE SET
            root_kind = excluded.root_kind,
            score = MAX(discovery_roots.score, excluded.score),
            max_depth = MAX(discovery_roots.max_depth, excluded.max_depth),
            entry_budget = MAX(discovery_roots.entry_budget, excluded.entry_budget),
            updated_at = excluded.updated_at
        "#,
        params![
            row.root_path.to_string_lossy(),
            row.kind.as_str(),
            created_from,
            row.score,
            row.max_depth as i64,
            row.entry_budget as i64,
            now,
        ],
    )?;
    Ok(())
}

fn load_learned_roots(conn: &Connection) -> Result<Vec<LearnedRoot>> {
    let now = unix_now();
    let mut stmt = conn.prepare(
        r#"
        SELECT root_path, score, max_depth, entry_budget
        FROM discovery_roots
        WHERE enabled = 1
          AND (cooldown_until IS NULL OR cooldown_until <= ?1)
        ORDER BY score DESC, last_scan_at ASC NULLS FIRST
        LIMIT 32
        "#,
    )?;
    let rows = stmt
        .query_map(params![now], |row| {
            let path: String = row.get(0)?;
            Ok(LearnedRoot {
                root_path: PathBuf::from(path),
                score: row.get(1)?,
                max_depth: row.get::<_, i64>(2)? as usize,
                entry_budget: row.get::<_, i64>(3)? as usize,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn load_tombstones(conn: &Connection) -> Result<std::collections::HashSet<String>> {
    let mut stmt = conn.prepare("SELECT canonical_path FROM discovery_tombstones")?;
    let rows = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows.into_iter().collect())
}

// ---------------------------------------------------------------------------
// scan.log
// ---------------------------------------------------------------------------

/// Append a single line to <app_data>/scan.log, rotating to scan.log.old
/// once the file crosses SCAN_LOG_MAX_BYTES. Errors swallowed — logging
/// is observability, not correctness.
pub fn append_scan_log(app: &AppHandle, line: &str) -> Result<()> {
    use std::io::Write;
    let dir = app.path().app_config_dir()?;
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("scan.log");

    if let Ok(meta) = std::fs::metadata(&path) {
        if meta.len() >= SCAN_LOG_MAX_BYTES {
            let old = dir.join("scan.log.old");
            let _ = std::fs::remove_file(&old);
            let _ = std::fs::rename(&path, &old);
        }
    }

    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)?;
    writeln!(f, "{line}")?;
    Ok(())
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dedup_keeps_highest_confidence_per_path() {
        let p = PathBuf::from("/tmp/test-repo");
        let candidates = vec![
            Candidate {
                path: p.clone(),
                source: DiscoverySource::FsWalk,
                confidence: 46,
                raw_hint: None,
            },
            Candidate {
                path: p.clone(),
                source: DiscoverySource::Vscode,
                confidence: 82,
                raw_hint: None,
            },
            Candidate {
                path: p.clone(),
                source: DiscoverySource::Manual,
                confidence: 100,
                raw_hint: None,
            },
        ];
        let out = dedup_candidates(candidates);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].confidence, 100);
        assert!(matches!(out[0].source, DiscoverySource::Manual));
    }

    #[test]
    fn safe_learning_root_rejects_system_dirs() {
        // These would expand fs walk into noise/garbage
        assert!(!is_safe_learning_root(Path::new(
            "C:\\Windows\\System32"
        )));
        assert!(!is_safe_learning_root(Path::new("C:\\Program Files\\foo")));
        assert!(!is_safe_learning_root(Path::new(
            "/Users/me/Library/Caches"
        )));
        // Legitimate dev paths pass through
        assert!(is_safe_learning_root(Path::new("C:\\k2\\keymall")));
        assert!(is_safe_learning_root(Path::new("/Users/me/code")));
    }

    #[test]
    fn learn_roots_proposes_parent_and_grandparent() {
        let repo = ValidatedRepo {
            observed_path: "C:\\k2\\keymall\\workspace".into(),
            canonical_path: "C:\\k2\\keymall\\workspace".into(),
            gitdir_path: "C:\\k2\\keymall\\workspace\\.git".into(),
            name: "workspace".into(),
        };
        let roots = learn_roots_from_repo(&repo, DiscoverySource::Manual);
        // Expect parent (C:\k2\keymall) + grandparent (C:\k2)
        let paths: Vec<String> = roots
            .iter()
            .map(|r| r.root_path.to_string_lossy().into_owned())
            .collect();
        assert!(paths.iter().any(|p| p.ends_with("keymall")));
        assert!(paths.iter().any(|p| p.ends_with("k2")));
        // Manual-source should mark parent as ManualParent
        assert!(roots
            .iter()
            .any(|r| matches!(r.kind, LearnedKind::ManualParent)));
    }

    #[test]
    fn learn_roots_does_not_climb_into_drive_root() {
        // A repo directly under C:\foo: parent C:\foo is fine, but
        // grandparent C:\ must NOT be learned.
        let repo = ValidatedRepo {
            observed_path: "C:\\foo".into(),
            canonical_path: "C:\\foo".into(),
            gitdir_path: "C:\\foo\\.git".into(),
            name: "foo".into(),
        };
        let roots = learn_roots_from_repo(&repo, DiscoverySource::Vscode);
        for r in &roots {
            let s = r.root_path.to_string_lossy();
            assert_ne!(s.as_ref(), "C:\\");
            assert_ne!(s.as_ref(), "C:");
        }
    }
}
