//! App settings and the signed-in account, as JSON in the app's config folder. The
//! token is not here; it lives in the keychain.

use std::path::Path;

use serde::{Deserialize, Serialize};
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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            account: None,
            sync_interval_minutes: 15,
            keep_read_days: DEFAULT_KEEP_READ_DAYS,
            confirm_mark_all_read: true,
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
