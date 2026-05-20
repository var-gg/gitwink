use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// Default global hotkey to summon the panel. Users can override via the
/// `panel_hotkey` field in settings.json — see DEFAULT_PANEL_HOTKEY usage
/// in lib.rs. `CmdOrCtrl` resolves to Cmd on macOS and Ctrl on Windows.
pub const DEFAULT_PANEL_HOTKEY: &str = "CmdOrCtrl+Shift+G";

/// How gitwink checks for its own updates. Serialized lowercase in
/// settings.json (`"enabled"` / `"manual"` / `"disabled"`).
#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UpdateCheckMode {
    /// Auto-check on startup + every 24h, with the tray dot + menu item.
    #[default]
    Enabled,
    /// No auto-check; only the tray "Check for updates" item works.
    Manual,
    /// Updater fully off — no checks, no tray affordances.
    Disabled,
}

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Settings {
    pub panel_position: Option<PanelPosition>,
    #[serde(default)]
    pub pinned_repos: Vec<String>,
    #[serde(default)]
    pub diff_window: Option<DiffWindowState>,
    /// Global hotkey spec (Tauri shortcut syntax, e.g. `"CmdOrCtrl+Shift+G"`,
    /// `"Alt+Space"`, `"Ctrl+Alt+Backquote"`). None or invalid value falls
    /// back to DEFAULT_PANEL_HOTKEY. Takes effect on next app start.
    #[serde(default)]
    pub panel_hotkey: Option<String>,
    /// Per-repo branch selection. Key = repo canonical path, value =
    /// the selected branch refNames. Restored when the user re-enters
    /// single-repo mode for that repo so the BranchChip filter survives
    /// across sessions. An absent entry means "all branches" — the
    /// first-entry default.
    #[serde(default)]
    pub branch_selections: HashMap<String, Vec<String>>,
    /// Update-checker behaviour — see `UpdateCheckMode`.
    #[serde(default)]
    pub update_check: UpdateCheckMode,
    /// Version the user chose to "Skip" in the update modal. Suppresses
    /// the update indicator for exactly this version; a newer release
    /// re-surfaces it (different version string).
    #[serde(default)]
    pub update_skipped_version: Option<String>,
    /// Unix timestamp until which the update indicator stays hidden after
    /// the user picked "Later". Absent or in the past = show normally.
    #[serde(default)]
    pub update_snooze_until: Option<i64>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct DiffWindowState {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
    #[serde(default)]
    pub maximized: bool,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy)]
pub struct PanelPosition {
    pub x: i32,
    pub y: i32,
}

fn settings_path(app: &AppHandle) -> Result<PathBuf> {
    let dir = app
        .path()
        .app_config_dir()
        .context("resolving app config dir")?;
    Ok(dir.join("settings.json"))
}

/// Public path resolver so the tray menu can offer "Open settings file..."
/// — making sure the file exists first by writing the current (possibly
/// default) state if it's missing.
pub fn ensure_path(app: &AppHandle) -> Result<PathBuf> {
    let path = settings_path(app)?;
    if !path.exists() {
        // Write whatever load() returns (default if no file) so the user
        // has something to edit rather than opening a non-existent file.
        let s = load(app);
        save(app, &s)?;
    }
    Ok(path)
}

pub fn load(app: &AppHandle) -> Settings {
    let Ok(path) = settings_path(app) else {
        return Settings::default();
    };
    let Ok(bytes) = fs::read(&path) else {
        return Settings::default();
    };
    serde_json::from_slice::<Settings>(&bytes).unwrap_or_default()
}

pub fn save(app: &AppHandle, settings: &Settings) -> Result<()> {
    let path = settings_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(settings).context("serialize settings")?;
    fs::write(&path, bytes).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

pub fn save_panel_position(app: &AppHandle, x: i32, y: i32) {
    let mut s = load(app);
    s.panel_position = Some(PanelPosition { x, y });
    if let Err(e) = save(app, &s) {
        eprintln!("settings: failed to persist panel position: {e:#}");
    }
}

pub fn clear_panel_position(app: &AppHandle) {
    let mut s = load(app);
    s.panel_position = None;
    if let Err(e) = save(app, &s) {
        eprintln!("settings: failed to clear panel position: {e:#}");
    }
}

pub fn save_pinned_repos(app: &AppHandle, pinned: Vec<String>) {
    let mut s = load(app);
    s.pinned_repos = pinned;
    if let Err(e) = save(app, &s) {
        eprintln!("settings: failed to persist pinned_repos: {e:#}");
    }
}

/// Persist the branch selection for one repo. An empty list removes the
/// entry entirely — absence means "all branches", so a selection of
/// "all" and a never-visited repo collapse to the same default.
pub fn save_branch_selection(app: &AppHandle, repo_path: &str, selection: Vec<String>) {
    let mut s = load(app);
    if selection.is_empty() {
        s.branch_selections.remove(repo_path);
    } else {
        s.branch_selections
            .insert(repo_path.to_string(), selection);
    }
    if let Err(e) = save(app, &s) {
        eprintln!("settings: failed to persist branch_selections: {e:#}");
    }
}

pub fn save_diff_window(app: &AppHandle, state: DiffWindowState) {
    let mut s = load(app);
    s.diff_window = Some(state);
    if let Err(e) = save(app, &s) {
        eprintln!("settings: failed to persist diff_window: {e:#}");
    }
}

pub fn save_replace(app: &AppHandle, settings: &Settings) -> Result<()> {
    save(app, settings)
}

/// Persist the version the user chose to skip (or `None` to clear it).
pub fn save_update_skipped_version(app: &AppHandle, version: Option<String>) {
    let mut s = load(app);
    s.update_skipped_version = version;
    if let Err(e) = save(app, &s) {
        eprintln!("settings: failed to persist update_skipped_version: {e:#}");
    }
}

/// Persist the "Later" snooze deadline (or `None` to clear it).
pub fn save_update_snooze_until(app: &AppHandle, until: Option<i64>) {
    let mut s = load(app);
    s.update_snooze_until = until;
    if let Err(e) = save(app, &s) {
        eprintln!("settings: failed to persist update_snooze_until: {e:#}");
    }
}
