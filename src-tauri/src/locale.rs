use serde::{Deserialize, Serialize};

/// User preference stored in settings.json. `System` follows the OS on each
/// launch; explicit choices remain stable if the OS language later changes.
#[derive(Debug, Default, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LocalePreference {
    #[default]
    System,
    En,
    Ko,
    Ja,
}

/// Locale after resolving `System`. Kept deliberately small and serializable:
/// the frontend uses this exact value to select one lazy catalog chunk.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AppLocale {
    En,
    Ko,
    Ja,
}

pub fn resolve_system(preference: LocalePreference) -> AppLocale {
    let system_locale = sys_locale::get_locale();
    resolve(preference, system_locale.as_deref())
}

pub fn resolve(preference: LocalePreference, system_locale: Option<&str>) -> AppLocale {
    match preference {
        LocalePreference::En => AppLocale::En,
        LocalePreference::Ko => AppLocale::Ko,
        LocalePreference::Ja => AppLocale::Ja,
        LocalePreference::System => match system_locale
            .unwrap_or_default()
            .split(['-', '_'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "ko" => AppLocale::Ko,
            "ja" => AppLocale::Ja,
            _ => AppLocale::En,
        },
    }
}

/// Native strings that must exist before any webview JavaScript executes.
/// Keep this catalog tiny: rich UI prose lives in Lingui PO catalogs, while
/// the tray remains a dependency-free Rust surface.
pub struct NativeCatalog {
    pub reset_panel_position: &'static str,
    pub settings: &'static str,
    pub quit: &'static str,
    pub check_for_updates: &'static str,
    pub update_available_prefix: &'static str,
    pub update_available_tooltip: &'static str,
}

pub fn native_catalog(locale: AppLocale) -> NativeCatalog {
    match locale {
        AppLocale::En => NativeCatalog {
            reset_panel_position: "Reset panel position",
            settings: "Settings…",
            quit: "Quit gitwink",
            check_for_updates: "Check for updates",
            update_available_prefix: "Update available",
            update_available_tooltip: "update available",
        },
        AppLocale::Ko => NativeCatalog {
            reset_panel_position: "패널 위치 초기화",
            settings: "설정…",
            quit: "gitwink 종료",
            check_for_updates: "업데이트 확인",
            update_available_prefix: "업데이트 가능",
            update_available_tooltip: "업데이트 가능",
        },
        AppLocale::Ja => NativeCatalog {
            reset_panel_position: "パネル位置をリセット",
            settings: "設定…",
            quit: "gitwink を終了",
            check_for_updates: "アップデートを確認",
            update_available_prefix: "アップデートがあります",
            update_available_tooltip: "アップデートがあります",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn system_locale_accepts_bcp47_and_posix_separators() {
        assert_eq!(
            resolve(LocalePreference::System, Some("ko-KR")),
            AppLocale::Ko
        );
        assert_eq!(
            resolve(LocalePreference::System, Some("ja_JP")),
            AppLocale::Ja
        );
        assert_eq!(
            resolve(LocalePreference::System, Some("en-US")),
            AppLocale::En
        );
    }

    #[test]
    fn unsupported_and_missing_system_locales_fall_back_to_english() {
        assert_eq!(
            resolve(LocalePreference::System, Some("fr-FR")),
            AppLocale::En
        );
        assert_eq!(resolve(LocalePreference::System, None), AppLocale::En);
    }

    #[test]
    fn explicit_preference_ignores_the_system_locale() {
        assert_eq!(resolve(LocalePreference::Ko, Some("ja-JP")), AppLocale::Ko);
        assert_eq!(resolve(LocalePreference::Ja, Some("ko-KR")), AppLocale::Ja);
        assert_eq!(resolve(LocalePreference::En, Some("ko-KR")), AppLocale::En);
    }
}
