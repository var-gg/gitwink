//! Microsoft Store-native update discovery and installation.
//!
//! This module is deliberately separate from `update.rs`: the Store is an
//! authoritative package channel with its own availability and install APIs,
//! while direct/Scoop builds discover releases through Tauri's GitHub-backed
//! updater. A Store user must only see an update after StoreContext says that
//! the package is actually available to this device.

#[cfg(not(target_os = "windows"))]
use tauri::AppHandle;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StoreAvailableUpdate {
    pub version: String,
    pub mandatory: bool,
}

fn format_package_version(major: u16, minor: u16, build: u16, revision: u16) -> String {
    if revision == 0 {
        format!("{major}.{minor}.{build}")
    } else {
        format!("{major}.{minor}.{build}.{revision}")
    }
}

#[cfg(target_os = "windows")]
mod imp {
    use std::{
        sync::{Mutex, OnceLock},
        time::{Duration, Instant},
    };

    use anyhow::{anyhow, bail, Context, Result};
    use tauri::{AppHandle, Manager};
    use windows::{
        core::Interface,
        ApplicationModel::Package,
        Services::Store::{
            StoreContext, StorePackageUpdate, StorePackageUpdateResult, StorePackageUpdateState,
            StorePackageUpdateStatus,
        },
        Win32::{Foundation::HWND, UI::Shell::IInitializeWithWindow},
    };
    use windows_collections::{IIterable, IVectorView};
    use windows_future::{
        AsyncOperationCompletedHandler, AsyncOperationProgressHandler,
        AsyncOperationWithProgressCompletedHandler, AsyncStatus, IAsyncOperationWithProgress,
    };

    use super::{format_package_version, StoreAvailableUpdate};

    type Updates = Vec<StorePackageUpdate>;
    type UpdateView = IVectorView<StorePackageUpdate>;
    type UpdateOperation =
        IAsyncOperationWithProgress<StorePackageUpdateResult, StorePackageUpdateStatus>;

    /// Package identity is stronger than an install-path heuristic. A
    /// packaged desktop app cannot safely replace itself with an NSIS updater,
    /// even when a sideloaded package uses a nonstandard install path.
    pub fn has_package_identity() -> bool {
        Package::Current().is_ok()
    }

    // Microsoft documents a 30-minute minimum interval for this Store query.
    // Cache both a positive and a negative result so manual checks and the
    // subsequent install action cannot accidentally burn the device quota.
    const CHECK_CACHE_TTL: Duration = Duration::from_secs(30 * 60);

    #[derive(Clone)]
    struct CachedStoreCheck {
        checked_at: Instant,
        available: Option<StoreAvailableUpdate>,
        updates: Updates,
    }

    static CHECK_CACHE: OnceLock<Mutex<Option<CachedStoreCheck>>> = OnceLock::new();

    /// Start and complete the Store query through WinRT-owned apartment-aware
    /// callbacks. The apartment-bound IVectorView is copied inside that
    /// callback and never crosses into Tauri's Send runtime.
    async fn get_updates(app: &AppHandle) -> Result<CachedStoreCheck> {
        let (tx, mut rx) = tauri::async_runtime::channel(1);
        let main_app = app.clone();
        app.run_on_main_thread(move || {
            let result = (|| {
                let context = store_context(&main_app, false)?;
                let operation = context
                    .GetAppAndOptionalStorePackageUpdatesAsync()
                    .context("Microsoft Store rejected the update query")?;
                let completion_context = context.clone();
                let completion_tx = tx.clone();
                let handler = AsyncOperationCompletedHandler::<UpdateView>::new(
                    move |operation, status| {
                        // Keep StoreContext alive until the asynchronous query
                        // has produced and copied its collection.
                        let _context_guard = &completion_context;
                        let result = (|| {
                            let operation = operation.as_ref().ok_or_else(|| {
                                anyhow!("Microsoft Store update check lost its operation")
                            })?;
                            if status != AsyncStatus::Completed {
                                let code = operation.ErrorCode().ok();
                                bail!(
                                    "Microsoft Store update check ended in state {status:?} (code: {code:?})"
                                );
                            }
                            let view = operation.GetResults().context(
                                "Microsoft Store could not return app update results",
                            )?;
                            // Optional-package updates must neither drive the
                            // badge nor hitchhike on the gitwink install.
                            let updates = select_app_updates(copy_update_view(&view)?)?;
                            let available = inspect_updates(&updates)?;
                            Ok(CachedStoreCheck {
                                checked_at: Instant::now(),
                                available,
                                updates,
                            })
                        })();
                        let _ = completion_tx.blocking_send(result);
                        Ok(())
                    },
                );
                operation
                    .SetCompleted(&handler)
                    .context("Microsoft Store could not schedule the update result")?;
                Ok(())
            })();
            if let Err(error) = result {
                let _ = tx.blocking_send(Err(error));
            }
        })
        .context("could not schedule the Microsoft Store update check")?;

        rx.recv()
            .await
            .ok_or_else(|| anyhow!("Microsoft Store update check did not finish"))?
    }

    fn copy_update_view(view: &UpdateView) -> Result<Updates> {
        let size = view.Size()?;
        let mut updates = Vec::with_capacity(size as usize);
        for index in 0..size {
            updates.push(view.GetAt(index)?);
        }
        Ok(updates)
    }

    /// StoreContext methods that display consent UI require an HWND owner in
    /// unpackaged desktop frameworks. Initialize whenever the panel exists so
    /// the same context is safe for both silent and interactive operations.
    fn store_context(app: &AppHandle, require_owner: bool) -> Result<StoreContext> {
        let context = StoreContext::GetDefault().context("Microsoft Store is unavailable")?;
        match app.get_webview_window("panel") {
            Some(panel) => {
                let raw_hwnd = panel
                    .hwnd()
                    .context("could not resolve the gitwink panel window")?
                    .0;
                let initializer: IInitializeWithWindow = context
                    .cast()
                    .context("Microsoft Store context does not support a desktop owner window")?;
                // SAFETY: `raw_hwnd` comes from the live Tauri panel and this
                // function only runs on Tauri's UI thread.
                unsafe { initializer.Initialize(HWND(raw_hwnd)) }
                    .context("could not attach Microsoft Store UI to the gitwink panel")?;
            }
            None if require_owner => bail!("The gitwink panel must be open to install an update."),
            None => {}
        }
        Ok(context)
    }

    fn select_app_updates(updates: Updates) -> Result<Updates> {
        let current_family = Package::Current()
            .context("gitwink does not have Microsoft Store package identity")?
            .Id()?
            .FamilyName()?
            .to_string();

        let mut selected = Vec::new();
        let mut first_error = None;
        for update in updates {
            let family = update
                .Package()
                .and_then(|package| package.Id())
                .and_then(|id| id.FamilyName())
                .map(|family| family.to_string());
            match family {
                Ok(family) if family == current_family => selected.push(update),
                Ok(_) => {}
                Err(error) => {
                    eprintln!("gitwink: skipped unreadable optional Store update: {error}");
                    first_error.get_or_insert(error);
                }
            }
        }

        if selected.is_empty() {
            if let Some(error) = first_error {
                return Err(error).context("Microsoft Store returned unreadable package updates");
            }
        }
        Ok(selected)
    }

    fn inspect_updates(updates: &[StorePackageUpdate]) -> Result<Option<StoreAvailableUpdate>> {
        let mut best: Option<((u16, u16, u16, u16), StoreAvailableUpdate)> = None;
        let mut mandatory = false;
        for update in updates {
            let package_id = update.Package()?.Id()?;
            let version = package_id.Version()?;
            let tuple = (
                version.Major,
                version.Minor,
                version.Build,
                version.Revision,
            );
            mandatory |= update.Mandatory()?;
            let candidate = StoreAvailableUpdate {
                version: format_package_version(
                    version.Major,
                    version.Minor,
                    version.Build,
                    version.Revision,
                ),
                mandatory,
            };
            if best.as_ref().is_none_or(|(current, _)| tuple > *current) {
                best = Some((tuple, candidate));
            }
        }

        Ok(best.map(|(_, mut update)| {
            update.mandatory = mandatory;
            update
        }))
    }

    fn cached_check() -> Option<CachedStoreCheck> {
        let cache = CHECK_CACHE.get_or_init(|| Mutex::new(None));
        let guard = cache.lock().unwrap_or_else(|error| error.into_inner());
        guard
            .as_ref()
            .filter(|entry| entry.checked_at.elapsed() < CHECK_CACHE_TTL)
            .cloned()
    }

    fn save_check(entry: &CachedStoreCheck) {
        let cache = CHECK_CACHE.get_or_init(|| Mutex::new(None));
        *cache.lock().unwrap_or_else(|error| error.into_inner()) = Some(entry.clone());
    }

    async fn query_app_updates(app: &AppHandle) -> Result<CachedStoreCheck> {
        let entry = get_updates(app).await?;
        save_check(&entry);
        Ok(entry)
    }

    pub async fn check(app: &AppHandle) -> Result<Option<StoreAvailableUpdate>> {
        if let Some(cached) = cached_check() {
            return Ok(cached.available);
        }
        let queried = query_app_updates(app).await?;
        Ok(queried.available)
    }

    pub async fn install(app: &AppHandle) -> Result<()> {
        let checked = match cached_check() {
            Some(cached) => cached,
            None => query_app_updates(app).await?,
        };
        if checked.available.is_none() {
            bail!("Microsoft Store reports that gitwink is already up to date.");
        }
        let updates = checked.updates;

        crate::update::emit_progress(app, "preparing", 0.0);

        if can_install_silently(app).await? {
            let result = run_install_operation(app, updates, InstallRequest::Silent).await?;
            // Once the Store accepted a silent operation, a terminal policy
            // failure (battery, Wi-Fi, cancellation, etc.) is authoritative.
            // Opening a second consent flow would only repeat the failure.
            return require_completed(result);
        }

        crate::update::emit_progress(app, "waiting", 0.0);
        let result = run_install_operation(app, updates, InstallRequest::Interactive).await?;
        require_completed(result)
    }

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum InstallRequest {
        Silent,
        Interactive,
    }

    impl InstallRequest {
        fn label(self) -> &'static str {
            match self {
                Self::Silent => "silent",
                Self::Interactive => "interactive",
            }
        }
    }

    async fn can_install_silently(app: &AppHandle) -> Result<bool> {
        let (tx, mut rx) = tauri::async_runtime::channel(1);
        let main_app = app.clone();
        app.run_on_main_thread(move || {
            let result = store_context(&main_app, false).map(|context| {
                context
                    .CanSilentlyDownloadStorePackageUpdates()
                    .unwrap_or(false)
            });
            let _ = tx.blocking_send(result);
        })
        .context("could not schedule Microsoft Store capability detection")?;

        rx.recv()
            .await
            .ok_or_else(|| anyhow!("Microsoft Store capability detection did not finish"))?
    }

    fn update_iterable(updates: Updates) -> IIterable<StorePackageUpdate> {
        // WinRT class references are nullable at the ABI boundary, hence the
        // collection crate models each value as Option<T>.
        IIterable::<StorePackageUpdate>::from(updates.into_iter().map(Some).collect::<Vec<_>>())
    }

    async fn run_install_operation(
        app: &AppHandle,
        updates: Updates,
        request: InstallRequest,
    ) -> Result<StorePackageUpdateState> {
        let (tx, mut rx) = tauri::async_runtime::channel(1);
        let main_app = app.clone();
        app.run_on_main_thread(move || {
            let result = (|| {
                let context = store_context(&main_app, request == InstallRequest::Interactive)?;
                let iterable = update_iterable(updates);
                let operation = match request {
                    InstallRequest::Silent => context
                        .TrySilentDownloadAndInstallStorePackageUpdatesAsync(&iterable)
                        .context("Microsoft Store could not start the silent update")?,
                    InstallRequest::Interactive => context
                        .RequestDownloadAndInstallStorePackageUpdatesAsync(&iterable)
                        .context("Microsoft Store could not open the update consent prompt")?,
                };
                attach_progress(&main_app, &operation)?;

                let completion_context = context.clone();
                let completion_tx = tx.clone();
                let label = request.label();
                let handler = AsyncOperationWithProgressCompletedHandler::<
                    StorePackageUpdateResult,
                    StorePackageUpdateStatus,
                >::new(move |operation, status| {
                    // Keep StoreContext alive until Windows has produced the
                    // terminal result for this package operation.
                    let _context_guard = &completion_context;
                    let result = (|| {
                        let operation = operation.as_ref().ok_or_else(|| {
                            anyhow!("Microsoft Store {label} update lost its operation")
                        })?;
                        if status == AsyncStatus::Canceled {
                            bail!("Microsoft Store update was canceled.");
                        }
                        if status != AsyncStatus::Completed {
                            let code = operation.ErrorCode().ok();
                            bail!(
                                "Microsoft Store {label} update ended in state {status:?} (code: {code:?})"
                            );
                        }
                        operation
                            .GetResults()
                            .with_context(|| {
                                format!("Microsoft Store could not finish the {label} update")
                            })?
                            .OverallState()
                            .context("Microsoft Store returned an unreadable update state")
                    })();
                    let _ = completion_tx.blocking_send(result);
                    Ok(())
                });
                operation
                    .SetCompleted(&handler)
                    .context("Microsoft Store could not schedule update completion")?;
                Ok(())
            })();
            if let Err(error) = result {
                let _ = tx.blocking_send(Err(error));
            }
        })
        .context("could not schedule the Microsoft Store update")?;

        rx.recv()
            .await
            .ok_or_else(|| anyhow!("Microsoft Store update did not finish"))?
    }

    fn attach_progress(app: &AppHandle, operation: &UpdateOperation) -> Result<()> {
        let progress_app = app.clone();
        let handler = AsyncOperationProgressHandler::<
            StorePackageUpdateResult,
            StorePackageUpdateStatus,
        >::new(move |_, status| {
            let Some(status) = status.as_ref() else {
                return Ok(());
            };
            let phase = if status.PackageUpdateState == StorePackageUpdateState::Deploying {
                "installing"
            } else {
                "downloading"
            };
            crate::update::emit_progress(&progress_app, phase, status.TotalDownloadProgress);
            Ok(())
        });
        operation
            .SetProgress(&handler)
            .context("could not observe Microsoft Store update progress")?;
        Ok(())
    }

    fn require_completed(state: StorePackageUpdateState) -> Result<()> {
        match state {
            StorePackageUpdateState::Completed => Ok(()),
            StorePackageUpdateState::Canceled => bail!("Microsoft Store update was canceled."),
            StorePackageUpdateState::ErrorLowBattery => {
                bail!("Connect this PC to power, then try the update again.")
            }
            StorePackageUpdateState::ErrorWiFiRequired => {
                bail!("Connect to Wi-Fi, then try the update again.")
            }
            StorePackageUpdateState::ErrorWiFiRecommended => {
                bail!("Microsoft Store recommends Wi-Fi for this update.")
            }
            state => bail!("Microsoft Store update did not finish (state: {state:?})."),
        }
    }
}

#[cfg(target_os = "windows")]
pub use imp::{check, has_package_identity, install};

#[cfg(not(target_os = "windows"))]
pub fn has_package_identity() -> bool {
    false
}

#[cfg(not(target_os = "windows"))]
pub async fn check(_app: &AppHandle) -> anyhow::Result<Option<StoreAvailableUpdate>> {
    Ok(None)
}

#[cfg(not(target_os = "windows"))]
pub async fn install(_app: &AppHandle) -> anyhow::Result<()> {
    anyhow::bail!("Microsoft Store updates are only available on Windows.")
}

#[cfg(test)]
mod tests {
    use super::format_package_version;

    #[test]
    fn store_version_hides_only_the_zero_msix_revision() {
        assert_eq!(format_package_version(0, 11, 0, 0), "0.11.0");
        assert_eq!(format_package_version(1, 2, 3, 4), "1.2.3.4");
    }
}
