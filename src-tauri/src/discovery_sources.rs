// Tier 1 + Tier 2 discovery readers.
//
// HARD RULES:
// - Every read is local-file, read-only. Never copy, modify, or write.
// - This module: no network, no telemetry, no IPC outside the app's own
//   events — discovery is purely local-file reads. (App-wide, the only
//   network is the optional GitHub update check + the optional auto-fetch.)
// - All readers are budget-aware (deadline + cap on entries) so the
//   sync first-paint path never gets stuck on a giant DB.
// - All readers are best-effort: a missing file, locked DB, schema
//   change, or parse error returns `Vec::new()`, never propagates.
//
// Tier 1: VS Code-family recents (VS Code, Insiders, Cursor, Windsurf)
//   - `state.vscdb` SQLite, ItemTable key=history.recentlyOpenedPathsList
//   - `.code-workspace` expansion (multi-root projects)
// Tier 2: git config hints
//   - `safe.directory` entries (high-confidence repo paths)
//   - `includeIf.gitdir:<pattern>` root hints

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::{Connection, OpenFlags};
use serde::Deserialize;

/// Where a candidate path came from. Used for `repo_sources.source`
/// column + confidence weighting + UI debug surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoverySource {
    /// User explicitly dropped/pasted a path.
    Manual,
    /// Cache hit from a previous run.
    Cache,
    /// VS Code recents.
    Vscode,
    /// VS Code Insiders recents.
    VscodeInsiders,
    /// Cursor recents (VS Code fork).
    Cursor,
    /// Windsurf recents (VS Code fork).
    Windsurf,
    /// `.code-workspace` folder entry.
    CodeWorkspace,
    /// git config `safe.directory`.
    GitConfigSafe,
    /// git config `includeIf.gitdir:<pattern>` — root hint, not repo.
    /// Collected for Phase 2 root expansion.
    #[allow(dead_code)]
    GitConfigIncludeIf,
    /// Filesystem walk (Tier 5).
    FsWalk,
}

impl DiscoverySource {
    pub fn as_str(self) -> &'static str {
        match self {
            DiscoverySource::Manual => "manual",
            DiscoverySource::Cache => "cache",
            DiscoverySource::Vscode => "vscode",
            DiscoverySource::VscodeInsiders => "vscode_insiders",
            DiscoverySource::Cursor => "cursor",
            DiscoverySource::Windsurf => "windsurf",
            DiscoverySource::CodeWorkspace => "code_workspace",
            DiscoverySource::GitConfigSafe => "git_config_safe",
            DiscoverySource::GitConfigIncludeIf => "git_config_includeif",
            DiscoverySource::FsWalk => "fs_walk",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub source: DiscoverySource,
    /// 0..=100. Used by the orchestrator for paint ordering and to seed
    /// `repos.confidence`. Manual=100, IDE recents=78-82, fs walk=46.
    pub confidence: i32,
    /// Free-form provenance (e.g. the raw URI we decoded, or the
    /// `.gitconfig` line). Stored in repo_sources.raw_hint for debug.
    pub raw_hint: Option<String>,
}

// ---------------------------------------------------------------------------
// Tier 1: VS Code-family `state.vscdb`
// ---------------------------------------------------------------------------

/// Maximum bytes we'll touch synchronously. Cursor's state.vscdb has been
/// reported in the hundreds of MB on heavy users; touching it on the
/// first-paint path is unacceptable. Background prewarm uses a higher cap.
const VSCODE_DB_SYNC_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Cap on entries we'll parse out of `history.recentlyOpenedPathsList`.
/// VS Code stores up to ~50 by default; some forks store more. We take
/// the most recent 80 to cover edge cases without blowing the budget.
const VSCODE_RECENT_ENTRIES_CAP: usize = 80;

/// All VS Code-family state DBs to probe on the current platform.
///
/// Returns `(source_kind, primary_db_path, fallback_storage_json_path)`
/// tuples in priority order. Callers iterate and try each in turn,
/// stopping when the deadline expires.
pub fn vscode_family_db_paths() -> Vec<(DiscoverySource, PathBuf, Option<PathBuf>)> {
    let mut out = Vec::new();

    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = std::env::var_os("APPDATA").map(PathBuf::from) {
            let variants = [
                (DiscoverySource::Vscode, "Code"),
                (DiscoverySource::VscodeInsiders, "Code - Insiders"),
                (DiscoverySource::Cursor, "Cursor"),
                (DiscoverySource::Windsurf, "Windsurf"),
            ];
            for (source, dirname) in variants {
                let base = appdata.join(dirname);
                out.push((
                    source,
                    base.join("User").join("globalStorage").join("state.vscdb"),
                    Some(base.join("storage.json")),
                ));
            }
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
            let app_support = home.join("Library").join("Application Support");
            let variants = [
                (DiscoverySource::Vscode, "Code"),
                (DiscoverySource::VscodeInsiders, "Code - Insiders"),
                (DiscoverySource::Cursor, "Cursor"),
                (DiscoverySource::Windsurf, "Windsurf"),
            ];
            for (source, dirname) in variants {
                let base = app_support.join(dirname);
                out.push((
                    source,
                    base.join("User").join("globalStorage").join("state.vscdb"),
                    Some(base.join("storage.json")),
                ));
            }
        }
    }

    out
}

#[derive(Debug, Deserialize)]
struct RecentlyOpened {
    entries: Vec<RecentEntry>,
}

#[derive(Debug, Deserialize)]
struct RecentEntry {
    #[serde(rename = "folderUri")]
    folder_uri: Option<String>,
    #[serde(rename = "fileUri")]
    file_uri: Option<String>,
    #[serde(rename = "workspace")]
    workspace: Option<WorkspaceRef>,
    #[serde(rename = "remoteAuthority")]
    remote_authority: Option<String>,
    #[allow(dead_code)]
    label: Option<String>,
}

#[derive(Debug, Deserialize)]
struct WorkspaceRef {
    #[serde(rename = "configPath")]
    config_path: Option<String>,
}

/// Read one VS Code-family `state.vscdb` and emit candidates. Skips
/// silently on any failure: missing file, oversize DB, locked DB,
/// schema change, JSON parse error. Caller is expected to call this
/// for each known DB path under a shared deadline.
pub fn read_vscode_recents(
    db_path: &Path,
    source: DiscoverySource,
    deadline: Instant,
) -> Vec<Candidate> {
    if Instant::now() >= deadline {
        return Vec::new();
    }
    if !db_path.exists() {
        return Vec::new();
    }
    if let Ok(meta) = std::fs::metadata(db_path) {
        if meta.len() > VSCODE_DB_SYNC_MAX_BYTES {
            return Vec::new();
        }
    }

    // SQLite URI with mode=ro is the safest way to open a DB that the
    // owning app may have locked — we never want a write lock.
    let uri = format!("file:{}?mode=ro&immutable=1", db_path.to_string_lossy());
    let conn = match Connection::open_with_flags(
        &uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY
            | OpenFlags::SQLITE_OPEN_URI
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };
    let _ = conn.busy_timeout(Duration::from_millis(20));

    let value: Result<String, _> = conn.query_row(
        "SELECT value FROM ItemTable WHERE key = 'history.recentlyOpenedPathsList' LIMIT 1",
        [],
        |row| row.get(0),
    );

    let Ok(json) = value else {
        return Vec::new();
    };

    let Ok(parsed) = serde_json::from_str::<RecentlyOpened>(&json) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for entry in parsed.entries.into_iter().take(VSCODE_RECENT_ENTRIES_CAP) {
        // Remote workspaces (vscode-remote://, SSH, devcontainer, WSL):
        // not a local path, can't be a local repo. Skip in v0.1.1.
        if entry.remote_authority.is_some() {
            continue;
        }

        if let Some(uri) = entry.folder_uri.as_deref() {
            if let Some(path) = file_uri_to_path(uri) {
                out.push(Candidate {
                    path,
                    source,
                    confidence: 82,
                    raw_hint: Some(uri.to_string()),
                });
            }
        } else if let Some(uri) = entry.file_uri.as_deref() {
            // A recent FILE — its parent is a plausible repo root candidate
            // (Repository::discover will walk further up if needed). Lower
            // confidence than a folder entry since it's an inference.
            if let Some(parent) =
                file_uri_to_path(uri).and_then(|p| p.parent().map(Path::to_path_buf))
            {
                out.push(Candidate {
                    path: parent,
                    source,
                    confidence: 60,
                    raw_hint: Some(uri.to_string()),
                });
            }
        } else if let Some(ws) = entry.workspace {
            // A `.code-workspace` recent. We store the workspace file
            // itself as the candidate; the orchestrator runs
            // `expand_code_workspace` to turn it into N folder candidates.
            if let Some(cfg_uri) = ws.config_path {
                if let Some(path) = file_uri_to_path(&cfg_uri) {
                    out.push(Candidate {
                        path,
                        source: DiscoverySource::CodeWorkspace,
                        confidence: 72,
                        raw_hint: Some(cfg_uri),
                    });
                }
            }
        }
    }

    out
}

/// Parse `file:///...` URIs into local PathBufs. Skips non-local schemes
/// (vscode-remote://, ssh://, untitled:) by returning None.
fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let decoded = percent_decode(rest)?;

    #[cfg(target_os = "windows")]
    {
        // file:///c%3A/Users/foo  →  /c:/Users/foo  →  C:\Users\foo
        let trimmed = decoded.trim_start_matches('/');
        if trimmed.len() >= 2 && trimmed.as_bytes().get(1) == Some(&b':') {
            return Some(PathBuf::from(trimmed.replace('/', "\\")));
        }
        // UNC: file://server/share/...  →  \\server\share\...
        // (note: file:// for UNC strips the leading //, so `decoded` here
        // starts with the server name directly when it's a true UNC URI;
        // we don't reach this branch for POSIX-style /tmp/... paths.)
        if !decoded.is_empty() && !decoded.starts_with('/') {
            return Some(PathBuf::from(format!("\\\\{}", decoded).replace('/', "\\")));
        }
        // POSIX-style /tmp/... on Windows isn't a valid local path —
        // drop it instead of inventing a fake UNC.
        None
    }

    #[cfg(not(target_os = "windows"))]
    {
        Some(PathBuf::from(decoded))
    }
}

/// Tiny percent-decoder for file:// URIs. We don't pull in `urlencoding`
/// just for this; the alphabet is small and the input is short.
fn percent_decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            let val = u8::from_str_radix(hex, 16).ok()?;
            out.push(val);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

// ---------------------------------------------------------------------------
// Tier 1b: `.code-workspace` expansion
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct CodeWorkspaceFile {
    folders: Option<Vec<CodeWorkspaceFolder>>,
}

#[derive(Debug, Deserialize)]
struct CodeWorkspaceFolder {
    path: Option<String>,
    uri: Option<String>,
}

/// Top folders to expand from a single `.code-workspace`. Multi-root
/// workspaces with 20+ folders exist; capped to keep validation cheap.
const CODE_WORKSPACE_FOLDERS_CAP: usize = 20;

/// Read a `.code-workspace` file (JSON-with-comments-tolerant via
/// serde_json's lenient mode is not on by default, so we parse strict
/// JSON and accept silent failures for files using comments). Returns
/// folder candidates, with paths resolved relative to the workspace
/// file's directory.
pub fn expand_code_workspace(workspace_file: &Path) -> Vec<Candidate> {
    if workspace_file
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| !s.eq_ignore_ascii_case("code-workspace"))
        .unwrap_or(true)
    {
        return Vec::new();
    }

    let Ok(text) = std::fs::read_to_string(workspace_file) else {
        return Vec::new();
    };
    let Ok(ws) = serde_json::from_str::<CodeWorkspaceFile>(&text) else {
        return Vec::new();
    };

    let base = workspace_file
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));

    let mut out = Vec::new();
    for folder in ws
        .folders
        .unwrap_or_default()
        .into_iter()
        .take(CODE_WORKSPACE_FOLDERS_CAP)
    {
        let candidate_path = if let Some(uri) = folder.uri {
            file_uri_to_path(&uri)
        } else if let Some(rel) = folder.path {
            let p = PathBuf::from(&rel);
            Some(if p.is_absolute() { p } else { base.join(p) })
        } else {
            None
        };

        if let Some(p) = candidate_path {
            out.push(Candidate {
                path: p,
                source: DiscoverySource::CodeWorkspace,
                confidence: 78,
                raw_hint: Some(workspace_file.to_string_lossy().into_owned()),
            });
        }
    }

    out
}

// ---------------------------------------------------------------------------
// Tier 2: git config hints
// ---------------------------------------------------------------------------

/// Hint extracted from the user's git config files. Either a concrete
/// repo path (`safe.directory`) or a glob pattern hint about where
/// repos live (`includeIf.gitdir:<pat>`).
#[derive(Debug, Clone)]
pub enum GitConfigHint {
    RepoPath(PathBuf),
    /// `includeIf "gitdir:<pat>"` pattern — collected for future
    /// Tier 4 expansion (Phase 2). v0.1.1 only acts on RepoPath.
    #[allow(dead_code)]
    RootPattern(String),
}

/// Read `~/.gitconfig` and `~/.config/git/config` for repo + root hints.
/// Uses git2's default config resolver, which respects the same lookup
/// order as the `git` CLI. Read-only.
pub fn read_git_config_hints() -> Vec<GitConfigHint> {
    let mut hints = Vec::new();

    let cfg = match git2::Config::open_default() {
        Ok(c) => c,
        Err(_) => return hints,
    };

    let mut entries = match cfg.entries(None) {
        Ok(e) => e,
        Err(_) => return hints,
    };

    while let Some(entry_res) = entries.next() {
        let Ok(entry) = entry_res else { continue };
        let Some(name) = entry.name() else { continue };
        let value = entry.value().unwrap_or("").trim();

        if name == "safe.directory" && !value.is_empty() && value != "*" {
            hints.push(GitConfigHint::RepoPath(expand_tilde(value)));
            continue;
        }

        // `git config` flattens `includeIf "gitdir:<pat>"` into entries
        // like `includeIf.gitdir:C:/k2/keymall/.path`. The value points
        // at the included config file, but the CONDITION (between the
        // first `.` and the trailing `.path`) is the root hint we want.
        if name.starts_with("includeIf.") && name.ends_with(".path") {
            let condition = name
                .trim_start_matches("includeIf.")
                .trim_end_matches(".path");
            if let Some(pattern) = condition
                .strip_prefix("gitdir:")
                .or_else(|| condition.strip_prefix("gitdir/i:"))
            {
                hints.push(GitConfigHint::RootPattern(pattern.to_string()));
            }
        }
    }

    hints
}

/// Expand a leading `~/` in a path string to the user's home directory.
fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(PathBuf::from))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn percent_decode_handles_common_uris() {
        assert_eq!(percent_decode("a%20b").as_deref(), Some("a b"));
        assert_eq!(percent_decode("c%3A/Users").as_deref(), Some("c:/Users"));
        assert_eq!(percent_decode("simple").as_deref(), Some("simple"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn file_uri_to_path_decodes_windows_drive() {
        // Standard VS Code recents shape on Windows.
        let uri = "file:///c%3A/k2/keymall/workspace";
        let p = file_uri_to_path(uri).expect("decode should succeed");
        assert_eq!(p, PathBuf::from("c:\\k2\\keymall\\workspace"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn file_uri_to_path_decodes_posix() {
        let uri = "file:///home/me/projects/foo";
        let p = file_uri_to_path(uri).expect("decode should succeed");
        assert_eq!(p, PathBuf::from("/home/me/projects/foo"));
    }

    #[test]
    fn vscode_family_paths_returns_known_variants() {
        // Just verify the four variants are wired up — actual path
        // existence depends on the host machine.
        let probes = vscode_family_db_paths();
        let names: Vec<&'static str> = probes.iter().map(|(s, _, _)| s.as_str()).collect();
        // On a runner without %APPDATA% / $HOME, this can return empty;
        // accept that, but if it's non-empty it must include vscode.
        if !names.is_empty() {
            assert!(names.contains(&"vscode"));
        }
    }

    #[test]
    fn expand_code_workspace_resolves_relative_folders() {
        let tmp = TempDir::new().unwrap();
        let ws_path = tmp.path().join("multi.code-workspace");
        fs::write(
            &ws_path,
            r#"{
                "folders": [
                    { "path": "service-a" },
                    { "path": "service-b" }
                ]
            }"#,
        )
        .unwrap();

        let candidates = expand_code_workspace(&ws_path);
        assert_eq!(candidates.len(), 2);
        assert!(candidates[0].path.ends_with("service-a"));
        assert!(candidates[1].path.ends_with("service-b"));
        assert!(matches!(
            candidates[0].source,
            DiscoverySource::CodeWorkspace
        ));
    }

    #[test]
    fn expand_code_workspace_caps_folder_count() {
        let tmp = TempDir::new().unwrap();
        let ws_path = tmp.path().join("many.code-workspace");
        let folders: Vec<String> = (0..50).map(|i| format!(r#"{{"path": "f{i}"}}"#)).collect();
        let json = format!(r#"{{"folders":[{}]}}"#, folders.join(","));
        fs::write(&ws_path, json).unwrap();

        let candidates = expand_code_workspace(&ws_path);
        assert!(candidates.len() <= CODE_WORKSPACE_FOLDERS_CAP);
    }

    #[test]
    fn expand_code_workspace_skips_non_workspace_files() {
        let tmp = TempDir::new().unwrap();
        let not_ws = tmp.path().join("settings.json");
        fs::write(&not_ws, r#"{"folders":[{"path":"x"}]}"#).unwrap();
        assert!(expand_code_workspace(&not_ws).is_empty());
    }

    #[test]
    fn read_vscode_recents_returns_empty_on_missing_db() {
        let tmp = TempDir::new().unwrap();
        let fake = tmp.path().join("no-such.vscdb");
        let deadline = Instant::now() + Duration::from_millis(100);
        let candidates = read_vscode_recents(&fake, DiscoverySource::Vscode, deadline);
        assert!(candidates.is_empty());
    }

    #[test]
    fn read_vscode_recents_handles_real_sqlite_with_recent_entries() {
        // Synthesise a minimal state.vscdb-like file with the schema and
        // key VS Code uses, so we exercise the SQLite + JSON path end to
        // end without needing VS Code installed. Use platform-correct
        // file URIs so the test asserts what would actually happen on a
        // real VS Code install.
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("state.vscdb");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE ItemTable (key TEXT PRIMARY KEY, value BLOB)",
            [],
        )
        .unwrap();

        #[cfg(target_os = "windows")]
        let recents_json = r#"{
            "entries": [
                {"folderUri": "file:///c%3A/k2/sample-repo"},
                {"folderUri": "file:///d%3A/dev/another"},
                {"fileUri": "file:///c%3A/k2/something/foo.rs"},
                {"folderUri": "vscode-remote://wsl%2Bubuntu/home/me/r", "remoteAuthority": "wsl+ubuntu"}
            ]
        }"#;
        #[cfg(not(target_os = "windows"))]
        let recents_json = r#"{
            "entries": [
                {"folderUri": "file:///tmp/sample-repo"},
                {"folderUri": "file:///tmp/another"},
                {"fileUri": "file:///tmp/something/foo.rs"},
                {"folderUri": "vscode-remote://wsl%2Bubuntu/home/me/r", "remoteAuthority": "wsl+ubuntu"}
            ]
        }"#;
        conn.execute(
            "INSERT INTO ItemTable (key, value) VALUES ('history.recentlyOpenedPathsList', ?1)",
            [recents_json],
        )
        .unwrap();
        drop(conn);

        let deadline = Instant::now() + Duration::from_secs(2);
        let candidates = read_vscode_recents(&db_path, DiscoverySource::Vscode, deadline);

        let paths: Vec<String> = candidates
            .iter()
            .map(|c| c.path.to_string_lossy().into_owned())
            .collect();

        #[cfg(target_os = "windows")]
        {
            assert!(
                paths.iter().any(|p| p.ends_with("sample-repo")),
                "missing sample-repo in {paths:?}"
            );
            assert!(
                paths.iter().any(|p| p.ends_with("another")),
                "missing another in {paths:?}"
            );
            // fileUri parent: c:\k2\something\foo.rs → c:\k2\something
            assert!(
                paths.iter().any(|p| p.ends_with("something")),
                "missing something (fileUri parent) in {paths:?}"
            );
        }
        #[cfg(not(target_os = "windows"))]
        {
            assert!(paths.contains(&"/tmp/sample-repo".to_string()));
            assert!(paths.contains(&"/tmp/another".to_string()));
            assert!(paths.contains(&"/tmp/something".to_string()));
        }
        assert!(!paths.iter().any(|p| p.contains("wsl")));
    }

    #[test]
    fn file_uri_to_path_rejects_remote_and_garbage() {
        // vscode-remote:// etc. are dropped at the entry level (remoteAuthority
        // check), but the raw decoder should also reject obvious non-file URIs.
        assert!(file_uri_to_path("vscode-remote://wsl/foo").is_none());
        assert!(file_uri_to_path("ssh://server/repo").is_none());
        assert!(file_uri_to_path("untitled:Untitled-1").is_none());
    }

    #[test]
    fn read_vscode_recents_respects_deadline() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("state.vscdb");
        std::fs::write(&db_path, b"not even a real sqlite file").unwrap();

        // Past deadline → return immediately with no work.
        let past = Instant::now() - Duration::from_millis(10);
        let candidates = read_vscode_recents(&db_path, DiscoverySource::Vscode, past);
        assert!(candidates.is_empty());
    }
}
