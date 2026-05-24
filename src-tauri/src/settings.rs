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
    /// back to DEFAULT_PANEL_HOTKEY. Re-registered live by `set_panel_hotkey`.
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
    /// UI scale multiplier (1.0 = default) — scales the diff + timeline
    /// font size and row height together. `None` is treated as 1.0.
    #[serde(default)]
    pub ui_scale: Option<f32>,
    /// Diff/code-view font family. `None` falls back to the built-in
    /// monospace stack; any installed font name is accepted.
    #[serde(default)]
    pub diff_font_family: Option<String>,
    /// "Pin" mode for the panel — when true, the panel does not auto-hide
    /// on blur, shows in the taskbar, and is not always-on-top. Default
    /// false (the tray-glance behaviour gitwink is designed around).
    #[serde(default)]
    pub panel_pinned: Option<bool>,
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

/// Persist the UI scale multiplier (or `None` to reset to default).
pub fn save_ui_scale(app: &AppHandle, scale: Option<f32>) {
    let mut s = load(app);
    s.ui_scale = scale;
    if let Err(e) = save(app, &s) {
        eprintln!("settings: failed to persist ui_scale: {e:#}");
    }
}

/// Persist the diff font family (or `None` for the built-in stack).
pub fn save_diff_font_family(app: &AppHandle, family: Option<String>) {
    let mut s = load(app);
    s.diff_font_family = family;
    if let Err(e) = save(app, &s) {
        eprintln!("settings: failed to persist diff_font_family: {e:#}");
    }
}

/// Persist the panel hotkey spec. Registration is the caller's job —
/// see `set_panel_hotkey` in commands.rs.
pub fn save_panel_hotkey(app: &AppHandle, spec: Option<String>) {
    let mut s = load(app);
    s.panel_hotkey = spec;
    if let Err(e) = save(app, &s) {
        eprintln!("settings: failed to persist panel_hotkey: {e:#}");
    }
}

/// Persist whether the panel is in pinned mode (no auto-hide on blur,
/// shown in taskbar, not always-on-top). Applying the window flags is
/// the caller's job — see `set_panel_pinned` in commands.rs.
pub fn save_panel_pinned(app: &AppHandle, pinned: bool) {
    let mut s = load(app);
    s.panel_pinned = Some(pinned);
    if let Err(e) = save(app, &s) {
        eprintln!("settings: failed to persist panel_pinned: {e:#}");
    }
}

/// Persist the self-update mode (`Enabled` / `Manual` / `Disabled`).
/// Refreshing the tray indicator + menu is the caller's job — see
/// `set_update_check` in commands.rs.
pub fn save_update_check_mode(app: &AppHandle, mode: UpdateCheckMode) {
    let mut s = load(app);
    s.update_check = mode;
    if let Err(e) = save(app, &s) {
        eprintln!("settings: failed to persist update_check: {e:#}");
    }
}
