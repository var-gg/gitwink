use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// Default global hotkey to summon the panel. Users can override via the
/// `panel_hotkey` field in settings.json — see DEFAULT_PANEL_HOTKEY usage
/// in lib.rs. `CmdOrCtrl` resolves to Cmd on macOS and Ctrl on Windows.
pub const DEFAULT_PANEL_HOTKEY: &str = "CmdOrCtrl+Shift+G";

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
