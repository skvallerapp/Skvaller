//! App settings and the accounts, as JSON in the app's config folder. Tokens are not
//! here; they live in the keychain.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Deserializer, Serialize};
use skvaller_core::DEFAULT_KEEP_READ_DAYS;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    #[serde(deserialize_with = "accounts_or_default")]
    pub accounts: Vec<Account>,
    /// The ID of the account the main window shows.
    pub active: Option<String>,
    /// The number in the next account's ID. Only grows, so an ID, and with it a cache
    /// file, is never reused.
    pub next_id: u64,
    /// Removed accounts whose cache files could not be deleted yet, such as on Windows
    /// while still open. The next start deletes them.
    pub removed: Vec<String>,
    /// Stored next to `accounts`, not nested, in the file.
    #[serde(flatten)]
    pub settings: Settings,
}

/// What the Settings window changes for every account. The UI receives and sends all of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    /// Ask before marking every article in a view as read.
    #[serde(deserialize_with = "confirm_mark_all_read_or_default")]
    pub confirm_mark_all_read: bool,
    #[serde(deserialize_with = "or_default")]
    pub theme: Theme,
    /// Off stops the UI's animations. The system's reduced motion stops them as well.
    #[serde(deserialize_with = "animations_or_default")]
    pub animations: bool,
    #[serde(deserialize_with = "or_default")]
    pub ui_font: Font,
    /// The article's headline, with its date and source.
    #[serde(deserialize_with = "or_default")]
    pub headline_font: Font,
    /// The article's body text.
    #[serde(deserialize_with = "article_font_or_default")]
    pub article_font: Font,
    /// Shortcut keys that differ from the defaults, by action. `None` means no key.
    /// The UI knows the actions and the defaults.
    #[serde(deserialize_with = "or_default")]
    pub shortcuts: BTreeMap<String, Option<String>>,
    #[serde(deserialize_with = "or_default")]
    pub sort_subscriptions: SortSubscriptions,
    #[serde(deserialize_with = "or_default")]
    pub mark_read_on: MarkReadOn,
    /// Mark articles read as they scroll out of the top of the list.
    #[serde(deserialize_with = "or_default")]
    pub mark_read_on_scroll: bool,
    /// The unread count of every account on the app icon, where the system shows one.
    #[serde(deserialize_with = "unread_badge_or_default")]
    pub unread_badge: bool,
}

/// How the sidebar orders folders and the feeds in no folder.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SortSubscriptions {
    /// Folders, then the feeds in no folder, each A–Z.
    #[default]
    FoldersFirst,
    /// Folders and the feeds in no folder in one A–Z list.
    Alphabetical,
}

/// When opening an article marks it read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MarkReadOn {
    #[default]
    Open,
    /// Once it has been open a moment, so stepping past it doesn't.
    Delay,
    /// Never on its own.
    Manual,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Theme {
    /// Light or dark, as the system is.
    #[default]
    System,
    Light,
    Dark,
    /// Warm brown, on a dark base.
    Espresso,
}

impl Theme {
    /// The theme for the windows' own parts, such as the title bar. `None` follows
    /// the system.
    pub fn native(self) -> Option<tauri::Theme> {
        match self {
            Theme::System => None,
            Theme::Light => Some(tauri::Theme::Light),
            Theme::Dark | Theme::Espresso => Some(tauri::Theme::Dark),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Font {
    /// Bundled with the app, like Geist, Source Serif and Literata; other scripts fall
    /// back to the system's fonts.
    #[default]
    Inter,
    Geist,
    System,
    SourceSerif,
    Literata,
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

fn article_font_or_default<'de, D: Deserializer<'de>>(d: D) -> Result<Font, D::Error> {
    or_else(d, |s| s.article_font)
}

fn animations_or_default<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    or_else(d, |s| s.animations)
}

fn confirm_mark_all_read_or_default<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    or_else(d, |s| s.confirm_mark_all_read)
}

fn unread_badge_or_default<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    or_else(d, |s| s.unread_badge)
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            confirm_mark_all_read: true,
            theme: Theme::default(),
            animations: true,
            ui_font: Font::default(),
            headline_font: Font::default(),
            article_font: Font::Literata,
            shortcuts: BTreeMap::new(),
            sort_subscriptions: SortSubscriptions::default(),
            mark_read_on: MarkReadOn::default(),
            mark_read_on_scroll: false,
            unread_badge: true,
        }
    }
}

/// An account, of either kind, with its own sync settings.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    /// Names the account's cache file, `accounts/<id>.db`. Never reused while the
    /// account exists.
    pub id: String,
    /// The name the UI shows. The user can change it.
    pub description: String,
    #[serde(flatten)]
    pub kind: AccountKind,
    /// FR-5a.
    #[serde(default, deserialize_with = "or_default")]
    pub sync: SyncSchedule,
    /// FR-5c.
    #[serde(
        default = "default_keep_read_days",
        deserialize_with = "keep_read_days_or_default"
    )]
    pub keep_read_days: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum AccountKind {
    /// A sync service, through the Google Reader API.
    Greader {
        /// The Google Reader API URL.
        server: String,
        username: String,
    },
    /// Feeds fetched on this device. At most one.
    Local,
}

impl AccountKind {
    /// The keychain entry for the account's token. A local account has none.
    pub fn keychain_key(&self) -> Option<String> {
        match self {
            AccountKind::Greader { server, username } => Some(format!("{username}@{server}")),
            AccountKind::Local => None,
        }
    }
}

/// When an account syncs on its own. In the file and the UI: `"manual"`, `"onStart"`,
/// or a number of minutes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncSchedule {
    /// Only when the user asks.
    Manual,
    /// Once when the app starts, then only when the user asks.
    OnStart,
    /// On start, then every this many minutes.
    Every(u32),
}

impl Default for SyncSchedule {
    fn default() -> Self {
        SyncSchedule::Every(15)
    }
}

impl Serialize for SyncSchedule {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            SyncSchedule::Manual => s.serialize_str("manual"),
            SyncSchedule::OnStart => s.serialize_str("onStart"),
            SyncSchedule::Every(minutes) => s.serialize_u32(*minutes),
        }
    }
}

impl<'de> Deserialize<'de> for SyncSchedule {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(d)? {
            serde_json::Value::String(s) if s == "manual" => Ok(SyncSchedule::Manual),
            serde_json::Value::String(s) if s == "onStart" => Ok(SyncSchedule::OnStart),
            serde_json::Value::Number(n) => match n.as_u64().and_then(|m| u32::try_from(m).ok()) {
                Some(m) if m > 0 => Ok(SyncSchedule::Every(m)),
                _ => Err(D::Error::custom("bad sync interval")),
            },
            _ => Err(D::Error::custom("bad sync schedule")),
        }
    }
}

fn default_keep_read_days() -> u32 {
    DEFAULT_KEEP_READ_DAYS
}

fn keep_read_days_or_default<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(u32::deserialize(value).unwrap_or(DEFAULT_KEEP_READ_DAYS))
}

/// A broken account is left out, instead of losing the others.
fn accounts_or_default<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Account>, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    let serde_json::Value::Array(items) = value else {
        return Ok(Vec::new());
    };
    Ok(items
        .into_iter()
        .filter_map(|item| Account::deserialize(item).ok())
        .collect())
}

/// The ID of the account a file from before several accounts gets.
pub const FIRST_ACCOUNT_ID: &str = "1";

/// Turns a file from before several accounts into the current shape: its one account,
/// with the then global sync interval and retention, becomes the first and active one.
fn migrate(value: &mut serde_json::Value) {
    let Some(object) = value.as_object_mut() else {
        return;
    };
    let interval = object.remove("syncIntervalMinutes");
    let keep = object.remove("keepReadDays");
    let Some(old) = object.remove("account") else {
        return;
    };
    if object.contains_key("accounts") {
        return;
    }
    let (Some(server), Some(username)) = (
        old.get("server").and_then(|v| v.as_str()),
        old.get("username").and_then(|v| v.as_str()),
    ) else {
        return;
    };
    // 0 meant manually, which did not sync on start either.
    let sync = match interval.as_ref().and_then(serde_json::Value::as_u64) {
        Some(0) => serde_json::json!("manual"),
        Some(m) => serde_json::json!(m),
        None => serde_json::json!(15),
    };
    object.insert(
        "accounts".into(),
        serde_json::json!([{
            "id": FIRST_ACCOUNT_ID,
            "description": "",
            "kind": "greader",
            "server": server,
            "username": username,
            "sync": sync,
            "keepReadDays": keep.unwrap_or(serde_json::json!(DEFAULT_KEEP_READ_DAYS)),
        }]),
    );
    object.insert("active".into(), serde_json::json!(FIRST_ACCOUNT_ID));
}

trait OkOrDefault {
    fn ok_or_default(self) -> Result<Config, Config>;
}

impl OkOrDefault for Option<Config> {
    fn ok_or_default(self) -> Result<Config, Config> {
        self.ok_or_else(Config::default)
    }
}

impl Config {
    /// A missing file gives the defaults. A file from before several accounts is
    /// migrated. `Err` with the defaults: the file is there but can't be read, and must
    /// not be overwritten before it is kept somewhere.
    pub fn load(path: &Path) -> Result<Self, Self> {
        match std::fs::read(path) {
            Ok(bytes) => Self::parse(&bytes).ok_or_default(),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(_) => Err(Self::default()),
        }
    }

    fn parse(bytes: &[u8]) -> Option<Self> {
        let mut value: serde_json::Value = serde_json::from_slice(bytes).ok()?;
        migrate(&mut value);
        let mut config: Self = serde_json::from_value(value).ok()?;
        if !config
            .accounts
            .iter()
            .any(|a| Some(&a.id) == config.active.as_ref())
        {
            config.active = config.accounts.first().map(|a| a.id.clone());
        }
        let past_ids = config
            .accounts
            .iter()
            .filter_map(|a| a.id.parse::<u64>().ok());
        config.next_id = config.next_id.max(past_ids.max().unwrap_or(0) + 1);
        Some(config)
    }

    pub fn account(&self, id: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.id == id)
    }

    /// Takes an ID no account had before.
    pub fn take_id(&mut self) -> String {
        self.next_id = self.next_id.max(1);
        let id = self.next_id.to_string();
        self.next_id += 1;
        id
    }

    /// Writes to a temporary file first, so a crash never leaves half a file.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        use std::io::Write;
        let tmp = path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        // On disk before the rename, so a power loss can't leave an empty file.
        file.sync_all()?;
        drop(file);
        std::fs::rename(tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keychain_service_is_the_bundle_identifier() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        assert_eq!(conf["identifier"], skvaller_core::KEYCHAIN_SERVICE);
    }

    #[test]
    fn fonts_default_to_inter_and_an_unknown_one_keeps_the_rest() {
        let config = Config::parse(
            br#"{ "account": { "server": "https://rss.example.com", "username": "carl" },
                 "articleFont": "source-serif", "uiFont": "comic" }"#,
        )
        .unwrap();

        assert_eq!(config.settings.article_font, Font::SourceSerif);
        assert_eq!(config.settings.ui_font, Font::Inter);
        assert_eq!(config.settings.headline_font, Font::Inter);
        assert_eq!(config.accounts.len(), 1);
        assert_eq!(Config::default().settings.ui_font, Font::Inter);
        assert_eq!(Config::default().settings.article_font, Font::Literata);

        let config: Config = serde_json::from_str(r#"{ "articleFont": "comic" }"#).unwrap();
        assert_eq!(config.settings.article_font, Font::Literata);
    }

    #[test]
    fn the_theme_defaults_to_system_and_an_unknown_one_keeps_the_rest() {
        let config: Config =
            serde_json::from_str(r#"{ "theme": "espresso", "uiFont": "serif" }"#).unwrap();
        assert_eq!(config.settings.theme, Theme::Espresso);

        let config: Config =
            serde_json::from_str(r#"{ "theme": "neon", "uiFont": "serif" }"#).unwrap();
        assert_eq!(config.settings.theme, Theme::System);
        assert_eq!(config.settings.ui_font, Font::Serif);
    }

    #[test]
    fn settings_sit_next_to_the_accounts_in_the_file() {
        let mut config = Config::default();
        config.settings.confirm_mark_all_read = false;
        config
            .settings
            .shortcuts
            .insert("next".into(), Some("n".into()));
        config.settings.shortcuts.insert("sync".into(), None);

        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["confirmMarkAllRead"], false);
        assert_eq!(json["shortcuts"]["next"], "n");

        let back: Config = serde_json::from_value(json).unwrap();
        assert_eq!(back.settings, config.settings);
    }

    #[test]
    fn a_bad_value_gives_that_settings_default_and_keeps_the_rest() {
        let config = Config::parse(
            br#"{ "accounts": [{ "id": "1", "description": "", "kind": "greader",
                                 "server": "https://rss.example.com", "username": "carl",
                                 "sync": "often", "keepReadDays": -1 },
                               { "id": "2", "kind": "nonsense" }],
                 "confirmMarkAllRead": null, "animations": "yes",
                 "shortcuts": { "next": 5 }, "articleFont": "serif" }"#,
        )
        .unwrap();

        assert_eq!(config.accounts.len(), 1);
        assert_eq!(config.accounts[0].sync, SyncSchedule::Every(15));
        assert_eq!(config.accounts[0].keep_read_days, DEFAULT_KEEP_READ_DAYS);
        assert_eq!(config.active.as_deref(), Some("1"));
        assert!(config.settings.confirm_mark_all_read);
        assert!(config.settings.animations);
        assert!(config.settings.shortcuts.is_empty());
        assert_eq!(config.settings.article_font, Font::Serif);
    }

    #[test]
    fn a_file_with_one_account_becomes_the_first_of_several() {
        let config = Config::parse(
            br#"{ "account": { "server": "https://rss.example.com", "username": "carl" },
                 "syncIntervalMinutes": 30, "keepReadDays": 7, "confirmMarkAllRead": false }"#,
        )
        .unwrap();

        assert_eq!(
            config.accounts,
            [Account {
                id: FIRST_ACCOUNT_ID.into(),
                description: String::new(),
                kind: AccountKind::Greader {
                    server: "https://rss.example.com".into(),
                    username: "carl".into(),
                },
                sync: SyncSchedule::Every(30),
                keep_read_days: 7,
            }]
        );
        assert_eq!(config.active.as_deref(), Some(FIRST_ACCOUNT_ID));
        assert!(!config.settings.confirm_mark_all_read);
        assert_eq!(config.clone().take_id(), "2");

        // Manually did not sync on start either.
        let config = Config::parse(
            br#"{ "account": { "server": "https://a.example", "username": "u" },
                 "syncIntervalMinutes": 0 }"#,
        )
        .unwrap();
        assert_eq!(config.accounts[0].sync, SyncSchedule::Manual);

        // Signed out: no account.
        let config = Config::parse(br#"{ "syncIntervalMinutes": 5 }"#).unwrap();
        assert!(config.accounts.is_empty());
        assert_eq!(config.active, None);
    }

    #[test]
    fn accounts_round_trip_with_their_kind_and_schedule() {
        let mut config = Config::default();
        config.accounts.push(Account {
            id: "3".into(),
            description: "Local".into(),
            kind: AccountKind::Local,
            sync: SyncSchedule::OnStart,
            keep_read_days: 14,
        });
        config.active = Some("3".into());

        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["accounts"][0]["kind"], "local");
        assert_eq!(json["accounts"][0]["sync"], "onStart");
        let back = Config::parse(&serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(back.accounts, config.accounts);
        assert_eq!(back.active, config.active);
        assert_eq!(AccountKind::Local.keychain_key(), None);
    }

    #[test]
    fn reading_settings_default_and_round_trip() {
        let config =
            Config::parse(br#"{ "markReadOn": "later", "sortSubscriptions": "alphabetical" }"#)
                .unwrap();
        assert_eq!(config.settings.mark_read_on, MarkReadOn::Open);
        assert_eq!(
            config.settings.sort_subscriptions,
            SortSubscriptions::Alphabetical
        );
        assert!(!config.settings.mark_read_on_scroll);
        assert!(config.settings.unread_badge);

        let mut config = Config::default();
        config.settings.mark_read_on = MarkReadOn::Delay;
        config.settings.unread_badge = false;
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["markReadOn"], "delay");
        assert_eq!(json["sortSubscriptions"], "foldersFirst");
        let back = Config::parse(&serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(back.settings, config.settings);
    }

    #[test]
    fn ids_are_never_reused() {
        let mut config = Config::default();
        assert_eq!(config.take_id(), "1");
        assert_eq!(config.take_id(), "2");
        let json = serde_json::to_vec(&config).unwrap();
        let mut back = Config::parse(&json).unwrap();
        assert_eq!(back.take_id(), "3");
    }

    #[test]
    fn an_unreadable_file_is_not_taken_for_an_empty_one() {
        let dir = std::env::temp_dir().join(format!("skvaller-config-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        assert!(Config::load(&path).is_ok());
        std::fs::write(&path, b"{ not json").unwrap();
        assert!(Config::load(&path).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
