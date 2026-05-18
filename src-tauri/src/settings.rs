use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Debug, Default, Serialize, Deserialize, Clone)]
pub struct Settings {
    pub panel_position: Option<PanelPosition>,
    #[serde(default)]
    pub pinned_repos: Vec<String>,
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
