//! App settings and the signed-in account, as JSON in the app's config folder. The
//! token is not here; it lives in the keychain.

use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize};
use skvaller_core::DEFAULT_KEEP_READ_DAYS;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    pub account: Option<Account>,
    /// FR-5a.
    pub sync_interval_minutes: u32,
    /// FR-5c.
    pub keep_read_days: u32,
    /// Ask before marking every article in a view as read.
    pub confirm_mark_all_read: bool,
    #[serde(deserialize_with = "or_default")]
    pub ui_font: Font,
    #[serde(deserialize_with = "or_default")]
    pub article_font: Font,
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

impl Default for Config {
    fn default() -> Self {
        Self {
            account: None,
            sync_interval_minutes: 15,
            keep_read_days: DEFAULT_KEEP_READ_DAYS,
            confirm_mark_all_read: true,
            ui_font: Font::default(),
            article_font: Font::default(),
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

        assert_eq!(config.article_font, Font::Serif);
        assert_eq!(config.ui_font, Font::Inter);
        assert!(config.account.is_some());
        assert_eq!(Config::default().ui_font, Font::Inter);
    }
}
