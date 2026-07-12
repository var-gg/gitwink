//! Runtime update policy and its frontend capability contract.
//!
//! Acquisition source and update ownership are not the same thing. WinGet,
//! for example, installs the same NSIS artifact as a direct GitHub download,
//! so both intentionally use the standalone/Tauri update policy. Scoop owns
//! its install step, while an MSIX package must stay inside the Store channel.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::store_update;

const SCOOP_UPDATE_COMMAND: &str = "scoop update gitwink";

/// The installed artifact's update-ownership class.
///
/// `Standalone` deliberately includes WinGet, direct NSIS/MSI installs and
/// macOS DMGs: they are the same self-updating artifact after installation.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum InstallChannel {
    Standalone,
    Scoop,
    Store,
}

/// Stable provider identity crossing IPC. Presentation layers translate this
/// code; backend policy never sends English prose as UI data.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum UpdateProvider {
    GithubReleases,
    MicrosoftStore,
    Scoop,
    Gitwink,
}

/// Capability payload consumed by the UI. Channel names remain available for
/// diagnostics, but presentation code must branch on these capabilities.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdatePolicyView {
    pub channel: InstallChannel,
    pub check_provider: UpdateProvider,
    pub supports_background_checks: bool,
    pub install_action: UpdateInstallActionView,
}

/// The final install action is a tagged union so impossible combinations such
/// as an external action without a command cannot cross the IPC boundary.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind")]
pub enum UpdateInstallActionView {
    #[serde(rename = "inApp")]
    InApp { provider: UpdateProvider },
    #[serde(rename = "externalCommand")]
    ExternalCommand {
        provider: UpdateProvider,
        command: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CheckBackend {
    Tauri,
    Store,
}

impl CheckBackend {
    fn provider(self) -> UpdateProvider {
        match self {
            Self::Tauri => UpdateProvider::GithubReleases,
            Self::Store => UpdateProvider::MicrosoftStore,
        }
    }

    fn supports_background_checks(self) -> bool {
        match self {
            Self::Tauri | Self::Store => true,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum InstallBackend {
    Tauri,
    Store,
    ExternalCommand {
        provider: UpdateProvider,
        command: &'static str,
    },
}

impl InstallBackend {
    fn view(self) -> UpdateInstallActionView {
        match self {
            Self::Tauri => UpdateInstallActionView::InApp {
                provider: UpdateProvider::Gitwink,
            },
            Self::Store => UpdateInstallActionView::InApp {
                provider: UpdateProvider::MicrosoftStore,
            },
            Self::ExternalCommand { provider, command } => {
                UpdateInstallActionView::ExternalCommand {
                    provider,
                    command: command.to_string(),
                }
            }
        }
    }
}

/// Internal, exhaustive routing policy. Adding a channel requires defining
/// both backends and its public capabilities in this one table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct UpdatePolicy {
    channel: InstallChannel,
    check_backend: CheckBackend,
    install_backend: InstallBackend,
}

impl UpdatePolicy {
    pub(super) fn detect() -> Self {
        let executable = std::env::current_exe().ok();
        let scoop_roots = [
            std::env::var_os("SCOOP").map(std::path::PathBuf::from),
            std::env::var_os("SCOOP_GLOBAL").map(std::path::PathBuf::from),
        ];
        policy_for_environment(
            store_update::has_package_identity(),
            executable.as_deref(),
            scoop_roots.iter().flatten().map(|path| path.as_path()),
        )
    }

    pub(super) fn check_backend(self) -> CheckBackend {
        self.check_backend
    }

    pub(super) fn install_backend(self) -> InstallBackend {
        self.install_backend
    }

    pub(super) fn view(self) -> UpdatePolicyView {
        UpdatePolicyView {
            channel: self.channel,
            check_provider: self.check_backend.provider(),
            supports_background_checks: self.check_backend.supports_background_checks(),
            install_action: self.install_backend.view(),
        }
    }

    fn for_channel(channel: InstallChannel) -> Self {
        match channel {
            InstallChannel::Standalone => Self {
                channel,
                check_backend: CheckBackend::Tauri,
                install_backend: InstallBackend::Tauri,
            },
            InstallChannel::Scoop => Self {
                channel,
                check_backend: CheckBackend::Tauri,
                install_backend: InstallBackend::ExternalCommand {
                    provider: UpdateProvider::Scoop,
                    command: SCOOP_UPDATE_COMMAND,
                },
            },
            InstallChannel::Store => Self {
                channel,
                check_backend: CheckBackend::Store,
                install_backend: InstallBackend::Store,
            },
        }
    }
}

fn policy_for_environment<'a>(
    has_package_identity: bool,
    executable: Option<&Path>,
    scoop_roots: impl IntoIterator<Item = &'a Path>,
) -> UpdatePolicy {
    let executable = executable.map(|path| normalize_path(&path.to_string_lossy()));
    let in_windows_apps = executable
        .as_deref()
        .is_some_and(|path| path.contains("\\windowsapps\\"));
    if has_package_identity || in_windows_apps {
        return UpdatePolicy::for_channel(InstallChannel::Store);
    }

    let scoop_roots = scoop_roots
        .into_iter()
        .map(|root| normalize_path(&root.to_string_lossy()))
        .collect::<Vec<_>>();
    let in_scoop = executable
        .as_deref()
        .is_some_and(|path| is_scoop_path(path, &scoop_roots));
    if in_scoop {
        UpdatePolicy::for_channel(InstallChannel::Scoop)
    } else {
        UpdatePolicy::for_channel(InstallChannel::Standalone)
    }
}

fn normalize_path(path: &str) -> String {
    path.to_lowercase().replace('/', "\\")
}

fn is_scoop_path(executable: &str, roots: &[String]) -> bool {
    if executable.contains("\\scoop\\apps\\") {
        return true;
    }
    roots.iter().any(|root| {
        let prefix = format!("{}\\apps\\", root.trim_end_matches('\\'));
        executable.starts_with(&prefix)
    })
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use super::{
        policy_for_environment, CheckBackend, InstallBackend, InstallChannel,
        UpdateInstallActionView, UpdateProvider,
    };

    fn policy(path: &str) -> super::UpdatePolicy {
        policy_for_environment(false, Some(Path::new(path)), std::iter::empty())
    }

    #[test]
    fn store_identity_wins_over_install_path() {
        let policy = policy_for_environment(
            true,
            Some(Path::new(r"C:\portable\gitwink.exe")),
            std::iter::empty(),
        );
        assert_eq!(policy.view().channel, InstallChannel::Store);
        assert_eq!(policy.check_backend(), CheckBackend::Store);
        assert_eq!(policy.install_backend(), InstallBackend::Store);
    }

    #[test]
    fn channel_matrix_routes_every_supported_distribution_contract() {
        let cases = [
            (
                r"C:\Program Files\WindowsApps\var-gg.gitwink_0.11.0.0_x64\gitwink.exe",
                InstallChannel::Store,
                CheckBackend::Store,
                InstallBackend::Store,
            ),
            (
                r"C:\Users\me\SCOOP\apps\gitwink\current\gitwink.exe",
                InstallChannel::Scoop,
                CheckBackend::Tauri,
                InstallBackend::ExternalCommand {
                    provider: UpdateProvider::Scoop,
                    command: "scoop update gitwink",
                },
            ),
            // WinGet installs this same NSIS artifact; runtime ownership is
            // intentionally standalone rather than an unprovable Winget guess.
            (
                r"C:\Program Files\gitwink\gitwink.exe",
                InstallChannel::Standalone,
                CheckBackend::Tauri,
                InstallBackend::Tauri,
            ),
            (
                "/Applications/gitwink.app/Contents/MacOS/gitwink",
                InstallChannel::Standalone,
                CheckBackend::Tauri,
                InstallBackend::Tauri,
            ),
        ];

        for (path, channel, check, install) in cases {
            let policy = policy(path);
            assert_eq!(policy.view().channel, channel, "{path}");
            assert_eq!(policy.check_backend(), check, "{path}");
            assert_eq!(policy.install_backend(), install, "{path}");
        }
    }

    #[test]
    fn custom_scoop_root_is_detected_without_requiring_a_scoop_folder_name() {
        let policy = policy_for_environment(
            false,
            Some(Path::new(
                r"D:\PackageRoot\apps\gitwink\current\gitwink.exe",
            )),
            [Path::new(r"D:\PackageRoot")],
        );
        assert_eq!(policy.view().channel, InstallChannel::Scoop);
    }

    #[test]
    fn capability_payload_is_tagged_and_self_describing() {
        let scoop = policy(r"C:\scoop\apps\gitwink\current\gitwink.exe").view();
        assert!(scoop.supports_background_checks);
        assert!(matches!(
            scoop.install_action,
            UpdateInstallActionView::ExternalCommand { .. }
        ));
        assert_eq!(
            serde_json::to_value(scoop).unwrap(),
            json!({
                "channel": "scoop",
                "checkProvider": "githubReleases",
                "supportsBackgroundChecks": true,
                "installAction": {
                    "kind": "externalCommand",
                    "provider": "scoop",
                    "command": "scoop update gitwink"
                }
            })
        );
    }
}
