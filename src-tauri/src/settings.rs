use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

/// Serializes every settings round-trip (load → mutate → save) across the
/// process. Writers run on at least four independent threads — the panel
/// move-debounce thread, the diff-window persist threads, sync IPC
/// commands, and the updater snooze/skip paths — and every helper here is
/// a whole-file read-modify-write: without one lock around the full
/// round-trip, two interleaved writers silently drop each other's fields
/// (drag the panel while the Settings slider saves → ui_scale snaps back).
static SETTINGS_IO: Mutex<()> = Mutex::new(());

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
    /// "Pin" mode for the panel — when true, the panel does NOT auto-hide
    /// on blur and is not always-on-top. The taskbar entry promise was
    /// dropped during the WebView2 stability arc: runtime set_skip_taskbar
    /// toggles on Windows destabilise subsequent WebView builds, so the
    /// panel always honours tauri.conf.json's skipTaskbar=true regardless
    /// of pin state. Pinned therefore means "no blur-dismiss + normal
    /// stacking" — the user still summons via tray / hotkey rather than
    /// alt-tab. Default false (the tray-glance behaviour gitwink is
    /// designed around).
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

/// Public path resolver for the Settings window's "Open settings.json"
/// footer link (the tray used to host this entry but it was demoted to
/// keep the tray to one Settings item). Writes the current — possibly
/// default — state first so the editor never opens to a "file not
/// found" dialog on a fresh install.
pub fn ensure_path(app: &AppHandle) -> Result<PathBuf> {
    let path = settings_path(app)?;
    if !path.exists() {
        // Write whatever load() returns (default if no file) so the user
        // has something to edit rather than opening a non-existent file.
        update_with(app, |_| {})?;
    }
    Ok(path)
}

pub fn load(app: &AppHandle) -> Settings {
    let _guard = SETTINGS_IO.lock().unwrap_or_else(|p| p.into_inner());
    load_unlocked(app)
}

fn load_unlocked(app: &AppHandle) -> Settings {
    let Ok(path) = settings_path(app) else {
        return Settings::default();
    };
    let Ok(bytes) = fs::read(&path) else {
        return Settings::default();
    };
    parse_or_bak(&path, &bytes)
}

/// Parse settings bytes; on failure, preserve the original as .bak and fall
/// back to defaults. A hand-edit typo (the Settings window links straight
/// to this file) must never silently become a full reset — the next
/// auto-save (a panel drag suffices) would persist the wipe, so the user's
/// original is kept next to it.
fn parse_or_bak(path: &std::path::Path, bytes: &[u8]) -> Settings {
    match serde_json::from_slice::<Settings>(bytes) {
        Ok(s) => s,
        Err(e) => {
            let bak = path.with_extension("json.bak");
            let _ = fs::copy(path, &bak);
            eprintln!(
                "gitwink: settings.json didn't parse ({e}); using defaults — \
                 your original was kept at {}",
                bak.display()
            );
            Settings::default()
        }
    }
}

/// Load for a read-modify-write round-trip. Unlike `load_unlocked`, an
/// EXISTING file we merely failed to READ (AV lock, transient IO error) is
/// an abort, not a default: returning defaults there would let the very
/// next save overwrite settings that are still intact on disk.
fn load_for_update(app: &AppHandle) -> Result<Settings> {
    let path = settings_path(app)?;
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Settings::default());
        }
        Err(e) => {
            return Err(e).with_context(|| format!("read {}", path.display()));
        }
    };
    Ok(parse_or_bak(&path, &bytes))
}

fn save_unlocked(app: &AppHandle, settings: &Settings) -> Result<()> {
    let path = settings_path(app)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("mkdir {}", parent.display()))?;
    }
    let bytes = serde_json::to_vec_pretty(settings).context("serialize settings")?;
    // Atomic replace: write + fsync the new content beside the file, then
    // rename over it (std::fs::rename replaces on Windows too). The fsync
    // matters — without it a power loss shortly after the rename can
    // publish a tmp file whose CONTENT never hit the platter.
    let tmp = path.with_extension("json.tmp");
    {
        use std::io::Write;
        let mut f =
            fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(&bytes)
            .with_context(|| format!("write {}", tmp.display()))?;
        f.sync_all()
            .with_context(|| format!("fsync {}", tmp.display()))?;
    }
    fs::rename(&tmp, &path)
        .with_context(|| format!("rename {} over {}", tmp.display(), path.display()))?;
    Ok(())
}

/// The one mutation path: lock → load → mutate → atomic save. Every
/// save_* helper goes through here so concurrent writers serialize on the
/// whole round-trip, not just the final write.
fn update_with(app: &AppHandle, mutate: impl FnOnce(&mut Settings)) -> Result<()> {
    let _guard = SETTINGS_IO.lock().unwrap_or_else(|p| p.into_inner());
    let mut s = load_for_update(app)?;
    mutate(&mut s);
    save_unlocked(app, &s)
}

pub fn save_panel_position(app: &AppHandle, x: i32, y: i32) {
    if let Err(e) = update_with(app, |s| {
        s.panel_position = Some(PanelPosition { x, y });
    }) {
        eprintln!("settings: failed to persist panel position: {e:#}");
    }
}

pub fn clear_panel_position(app: &AppHandle) {
    if let Err(e) = update_with(app, |s| s.panel_position = None) {
        eprintln!("settings: failed to clear panel position: {e:#}");
    }
}

pub fn save_pinned_repos(app: &AppHandle, pinned: Vec<String>) {
    if let Err(e) = update_with(app, move |s| s.pinned_repos = pinned) {
        eprintln!("settings: failed to persist pinned_repos: {e:#}");
    }
}

/// Persist the branch selection for one repo. An empty list removes the
/// entry entirely — absence means "all branches", so a selection of
/// "all" and a never-visited repo collapse to the same default.
pub fn save_branch_selection(app: &AppHandle, repo_path: &str, selection: Vec<String>) {
    if let Err(e) = update_with(app, |s| {
        if selection.is_empty() {
            s.branch_selections.remove(repo_path);
        } else {
            s.branch_selections
                .insert(repo_path.to_string(), selection);
        }
    }) {
        eprintln!("settings: failed to persist branch_selections: {e:#}");
    }
}

pub fn save_diff_window(app: &AppHandle, state: DiffWindowState) {
    if let Err(e) = update_with(app, |s| s.diff_window = Some(state)) {
        eprintln!("settings: failed to persist diff_window: {e:#}");
    }
}

/// Flip only the diff window's `maximized` flag, preserving the last
/// windowed geometry. Used while the window IS maximized — the OS-reported
/// outer rect is the maximized one, useless for restoring a windowed size.
pub fn save_diff_window_maximized(app: &AppHandle) {
    if let Err(e) = update_with(app, |s| {
        let prev = s.diff_window.unwrap_or(DiffWindowState {
            x: 200,
            y: 100,
            w: 1100,
            h: 750,
            maximized: false,
        });
        s.diff_window = Some(DiffWindowState {
            maximized: true,
            ..prev
        });
    }) {
        eprintln!("settings: failed to persist diff_window maximized: {e:#}");
    }
}

/// Persist the version the user chose to skip (or `None` to clear it).
pub fn save_update_skipped_version(app: &AppHandle, version: Option<String>) {
    if let Err(e) = update_with(app, |s| s.update_skipped_version = version) {
        eprintln!("settings: failed to persist update_skipped_version: {e:#}");
    }
}

/// Persist the "Later" snooze deadline (or `None` to clear it).
pub fn save_update_snooze_until(app: &AppHandle, until: Option<i64>) {
    if let Err(e) = update_with(app, |s| s.update_snooze_until = until) {
        eprintln!("settings: failed to persist update_snooze_until: {e:#}");
    }
}

/// Persist the UI scale multiplier (or `None` to reset to default).
pub fn save_ui_scale(app: &AppHandle, scale: Option<f32>) {
    if let Err(e) = update_with(app, |s| s.ui_scale = scale) {
        eprintln!("settings: failed to persist ui_scale: {e:#}");
    }
}

/// Persist the diff font family (or `None` for the built-in stack).
pub fn save_diff_font_family(app: &AppHandle, family: Option<String>) {
    if let Err(e) = update_with(app, |s| s.diff_font_family = family) {
        eprintln!("settings: failed to persist diff_font_family: {e:#}");
    }
}

/// Persist the panel hotkey spec. Registration is the caller's job —
/// see `set_panel_hotkey` in commands.rs.
pub fn save_panel_hotkey(app: &AppHandle, spec: Option<String>) {
    if let Err(e) = update_with(app, |s| s.panel_hotkey = spec) {
        eprintln!("settings: failed to persist panel_hotkey: {e:#}");
    }
}

/// Persist the panel pin mode. Returns the disk write Result so the
/// caller can refuse to mutate runtime state on a persistence failure —
/// otherwise the UI would say "pinned" until restart, then revert.
/// Runtime state flips (atomic + always_on_top) are the caller's job;
/// see `set_panel_pinned` in commands.rs.
pub fn save_panel_pinned(app: &AppHandle, pinned: bool) -> Result<()> {
    update_with(app, |s| s.panel_pinned = Some(pinned))
}

/// Persist the self-update mode (`Enabled` / `Manual` / `Disabled`).
/// Refreshing the tray indicator + menu is the caller's job — see
/// `set_update_check` in commands.rs.
pub fn save_update_check_mode(app: &AppHandle, mode: UpdateCheckMode) {
    if let Err(e) = update_with(app, |s| s.update_check = mode) {
        eprintln!("settings: failed to persist update_check: {e:#}");
    }
}
