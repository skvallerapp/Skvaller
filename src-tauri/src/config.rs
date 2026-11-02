//! App settings and the signed-in account, as JSON in the app's config folder. The
//! token is not here; it lives in the keychain.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize};
use skvaller_core::DEFAULT_KEEP_READ_DAYS;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub account: Option<Account>,
    /// Stored next to `account`, not nested, in the file.
    #[serde(flatten)]
    pub settings: Settings,
}

/// What the Settings window changes. The UI receives and sends all of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// FR-5a. 0 means manually: no sync starts on its own.
    #[serde(deserialize_with = "sync_interval_or_default")]
    pub sync_interval_minutes: u32,
    /// FR-5c.
    #[serde(deserialize_with = "keep_read_days_or_default")]
    pub keep_read_days: u32,
    /// Ask before marking every article in a view as read.
    #[serde(deserialize_with = "confirm_mark_all_read_or_default")]
    pub confirm_mark_all_read: bool,
    #[serde(deserialize_with = "or_default")]
    pub ui_font: Font,
    #[serde(deserialize_with = "or_default")]
    pub article_font: Font,
    /// Shortcut keys that differ from the defaults, by action. `None` means no key.
    /// The UI knows the actions and the defaults.
    #[serde(deserialize_with = "or_default")]
    pub shortcuts: BTreeMap<String, Option<String>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Font {
    /// Bundled with the app; other scripts fall back to the system's fonts.
    #[default]
    Inter,
    System,
    Serif,
}

/// An unknown value gives the default, instead of failing the whole file and with it
/// the saved account. The next save writes the default back.
fn or_default<'de, D: Deserializer<'de>, T: Deserialize<'de> + Default>(
    d: D,
) -> Result<T, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(T::deserialize(value).unwrap_or_default())
}

/// Like `or_default`, for a field whose type's default is not the setting's default:
/// a bad sync interval must not turn into 0, which means manually.
fn or_else<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
    fallback: impl FnOnce(Settings) -> T,
) -> Result<T, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(T::deserialize(value).unwrap_or_else(|_| fallback(Settings::default())))
}

fn sync_interval_or_default<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    or_else(d, |s| s.sync_interval_minutes)
}

fn keep_read_days_or_default<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    or_else(d, |s| s.keep_read_days)
}

fn confirm_mark_all_read_or_default<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    or_else(d, |s| s.confirm_mark_all_read)
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sync_interval_minutes: 15,
            keep_read_days: DEFAULT_KEEP_READ_DAYS,
            confirm_mark_all_read: true,
            ui_font: Font::default(),
            article_font: Font::default(),
            shortcuts: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Account {
    /// The Google Reader API URL.
    pub server: String,
    pub username: String,
}

impl Account {
    /// The keychain entry for this account's token.
    pub fn keychain_key(&self) -> String {
        format!("{}@{}", self.username, self.server)
    }
}

impl Config {
    /// A missing or unreadable file gives the defaults.
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// Writes to a temporary file first, so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_default_to_inter_and_an_unknown_one_keeps_the_rest() {
        let config: Config = serde_json::from_str(
            r#"{ "account": { "server": "https://rss.example.com", "username": "carl" },
                 "articleFont": "serif", "uiFont": "comic" }"#,
        )
        .unwrap();

        assert_eq!(config.settings.article_font, Font::Serif);
        assert_eq!(config.settings.ui_font, Font::Inter);
        assert!(config.account.is_some());
        assert_eq!(Config::default().settings.ui_font, Font::Inter);
    }

    #[test]
    fn settings_sit_next_to_the_account_in_the_file() {
        let mut config = Config::default();
        config.settings.sync_interval_minutes = 0;
        config
            .settings
            .shortcuts
            .insert("next".into(), Some("n".into()));
        config.settings.shortcuts.insert("sync".into(), None);

        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["syncIntervalMinutes"], 0);
        assert_eq!(json["shortcuts"]["next"], "n");

        let back: Config = serde_json::from_value(json).unwrap();
        assert_eq!(back.settings, config.settings);
    }

    #[test]
    fn a_bad_value_gives_that_settings_default_and_keeps_the_rest() {
        let config: Config = serde_json::from_str(
            r#"{ "account": { "server": "https://rss.example.com", "username": "carl" },
                 "syncIntervalMinutes": "15", "keepReadDays": -1, "confirmMarkAllRead": null,
                 "shortcuts": { "next": 5 }, "articleFont": "serif" }"#,
        )
        .unwrap();

        assert_eq!(config.settings.sync_interval_minutes, 15);
        assert_eq!(config.settings.keep_read_days, DEFAULT_KEEP_READ_DAYS);
        assert!(config.settings.confirm_mark_all_read);
        assert!(config.settings.shortcuts.is_empty());
        assert_eq!(config.settings.article_font, Font::Serif);
        assert!(config.account.is_some());
    }

    #[test]
    fn a_file_from_before_the_settings_window_keeps_its_values() {
        let config: Config = serde_json::from_str(
            r#"{ "syncIntervalMinutes": 30, "keepReadDays": 7, "confirmMarkAllRead": false }"#,
        )
        .unwrap();

        assert_eq!(config.settings.sync_interval_minutes, 30);
        assert_eq!(config.settings.keep_read_days, 7);
        assert!(!config.settings.confirm_mark_all_read);
        assert!(config.settings.shortcuts.is_empty());
    }
}
