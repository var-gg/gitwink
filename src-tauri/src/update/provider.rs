//! Provider-specific update I/O.
//!
//! The orchestrator owns scheduling, publication and UI events. This module
//! owns only the external calls required by each check/install backend.

use anyhow::{bail, Result};
use tauri::AppHandle;
use tauri_plugin_updater::UpdaterExt;

use crate::store_update;

use super::policy::{CheckBackend, InstallBackend};
use super::{emit_progress, AvailableUpdate};

pub(super) async fn check(
    app: &AppHandle,
    backend: CheckBackend,
) -> Result<Option<AvailableUpdate>> {
    match backend {
        CheckBackend::Store => Ok(store_update::check(app)
            .await?
            .map(|update| AvailableUpdate {
                version: update.version,
                notes: if update.mandatory {
                    "Microsoft Store marked this update as required. gitwink will download and install it without leaving the app."
                        .to_string()
                } else {
                    "This update is ready from Microsoft Store. gitwink can download and install it without leaving the app."
                        .to_string()
                },
                mandatory: update.mandatory,
            })),
        CheckBackend::Tauri => Ok(app.updater()?.check().await?.map(|update| AvailableUpdate {
            version: update.version,
            notes: update.body.unwrap_or_default(),
            mandatory: false,
        })),
    }
}

pub(super) async fn install(app: &AppHandle, backend: InstallBackend) -> Result<()> {
    match backend {
        InstallBackend::Store => store_update::install(app).await,
        InstallBackend::Tauri => install_tauri_update(app).await,
        InstallBackend::ExternalCommand { provider, command } => bail!(
            "EXTERNAL_UPDATE_PROVIDER:{provider:?}: run `{command}` through the owning package manager"
        ),
    }
}

async fn install_tauri_update(app: &AppHandle) -> Result<()> {
    let updater = app.updater()?;
    let pending = updater
        .check()
        .await?
        .ok_or_else(|| anyhow::anyhow!("No update available."))?;
    let progress_app = app.clone();
    let finish_app = app.clone();
    let mut downloaded = 0u64;
    pending
        .download_and_install(
            move |chunk, total| {
                downloaded = downloaded.saturating_add(chunk as u64);
                let progress = total
                    .filter(|total| *total > 0)
                    .map(|total| downloaded as f64 / total as f64)
                    .unwrap_or(0.0);
                emit_progress(&progress_app, "downloading", progress);
            },
            move || emit_progress(&finish_app, "installing", 1.0),
        )
        .await?;
    Ok(())
}
