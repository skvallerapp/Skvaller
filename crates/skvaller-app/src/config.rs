//! The settings and accounts in `config.json`.
//!
//! Tokens are not here; they live in the keychain. Reading is forgiving: an unknown or
//! broken value gives that setting's default, and an account that can't be read is left
//! out, instead of failing the whole file.

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
    #[serde(deserialize_with = "layout_or_default")]
    pub layout: Layout,
}

/// A pane's width range and default, in CSS pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneWidth {
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

impl PaneWidth {
    /// `width` within the range, or the default when it isn't a number.
    #[must_use]
    pub fn clamp(self, width: f64) -> f64 {
        if width.is_finite() {
            width.clamp(self.min, self.max)
        } else {
            self.default
        }
    }
}

/// The sidebar's width.
pub const SIDEBAR_WIDTH: PaneWidth = PaneWidth {
    min: 180.0,
    max: 360.0,
    default: 230.0,
};

/// The article list's width.
pub const LIST_WIDTH: PaneWidth = PaneWidth {
    min: 260.0,
    max: 560.0,
    default: 340.0,
};

/// The main window's panes, as the user last left them.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Layout {
    pub sidebar_open: bool,
    pub sidebar_width: f64,
    pub list_width: f64,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            sidebar_open: true,
            sidebar_width: SIDEBAR_WIDTH.default,
            list_width: LIST_WIDTH.default,
        }
    }
}

/// The layout, each part within its range or else its default.
fn layout_or_default<'de, D: Deserializer<'de>>(d: D) -> Result<Layout, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    let width = |key: &str, range: PaneWidth| {
        value
            .get(key)
            .and_then(serde_json::Value::as_f64)
            .map_or(range.default, |w| range.clamp(w))
    };
    Ok(Layout {
        sidebar_open: value.get("sidebarOpen") != Some(&serde_json::Value::Bool(false)),
        sidebar_width: width("sidebarWidth", SIDEBAR_WIDTH),
        list_width: width("listWidth", LIST_WIDTH),
    })
}

/// What the Settings window changes for every account. The UI receives and sends all of it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent on/off settings, as the file has them"
)]
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
    /// The size of the main window's text: the sidebar, the article list and the article.
    #[serde(deserialize_with = "or_default")]
    pub font_size: FontSize,
    /// How wide the article's text column is.
    #[serde(deserialize_with = "or_default")]
    pub line_width: LineWidth,
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
    #[serde(deserialize_with = "or_default")]
    pub thumbnail_size: ThumbnailSize,
    /// The UI's language, as a BCP 47 tag such as `sv` or `pt-BR`. `None` follows the
    /// system. The UI knows the languages it has.
    #[serde(deserialize_with = "or_default")]
    pub language: Option<String>,
    #[serde(deserialize_with = "or_default")]
    pub clock: Clock,
    #[serde(deserialize_with = "or_default")]
    pub date_format: DateFormat,
}

/// How times show: as the system's region shows them, or on a chosen clock.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Clock {
    #[default]
    System,
    /// 00:00 to 23:59.
    H24,
    /// 12:00 AM to 11:59 PM.
    H12,
}

/// How dates show: as the system's region shows them, or in a chosen order. Month names
/// follow the system's region either way.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DateFormat {
    #[default]
    System,
    /// 12 Oct 2026.
    DayMonth,
    /// Oct 12, 2026.
    MonthDay,
    /// 2026-10-12.
    Iso,
}

/// How large the article list shows each article's thumbnail.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ThumbnailSize {
    /// No thumbnails.
    None,
    /// A small square beside the text.
    Small,
    /// A larger square beside the text.
    #[default]
    Medium,
    /// As wide as the list, under the headline.
    Large,
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

/// The size of the main window's text, in nine steps. M is the size the app is drawn at
/// without the setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FontSize {
    Xxxs,
    Xxs,
    Xs,
    S,
    #[default]
    M,
    L,
    Xl,
    Xxl,
    Xxxl,
}

impl FontSize {
    /// From the smallest to the largest.
    pub const ALL: [Self; 9] = [
        Self::Xxxs,
        Self::Xxs,
        Self::Xs,
        Self::S,
        Self::M,
        Self::L,
        Self::Xl,
        Self::Xxl,
        Self::Xxxl,
    ];

    /// Steps from M: -4 for XXXS to 4 for XXXL.
    #[must_use]
    pub fn step(self) -> i32 {
        self.index() - 4
    }

    /// One step larger, or this one if it is the largest.
    #[must_use]
    pub fn larger(self) -> Self {
        Self::at(self.index() + 1)
    }

    /// One step smaller, or this one if it is the smallest.
    #[must_use]
    pub fn smaller(self) -> Self {
        Self::at(self.index() - 1)
    }

    fn index(self) -> i32 {
        // Nine variants, in order.
        self as i32
    }

    fn at(index: i32) -> Self {
        Self::ALL[usize::try_from(index.clamp(0, 8)).unwrap_or(0)]
    }
}

/// How wide the article's text column is.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LineWidth {
    Narrow,
    #[default]
    Medium,
    Wide,
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
/// a bad `animations` must give `true`, not `bool`'s `false`.
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
            font_size: FontSize::default(),
            line_width: LineWidth::default(),
            shortcuts: BTreeMap::new(),
            sort_subscriptions: SortSubscriptions::default(),
            mark_read_on: MarkReadOn::default(),
            mark_read_on_scroll: false,
            unread_badge: true,
            thumbnail_size: ThumbnailSize::default(),
            language: None,
            clock: Clock::default(),
            date_format: DateFormat::default(),
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
    #[serde(default, deserialize_with = "or_default")]
    pub sync: SyncSchedule,
    /// How long read, unstarred articles stay cached, in days.
    #[serde(
        default = "default_keep_read_days",
        deserialize_with = "keep_read_days_or_default"
    )]
    pub keep_read_days: u32,
    /// How long unread, unstarred articles stay, in days; 0 keeps them. Only the local
    /// account's: a sync account's server lists its unread articles, and its retention
    /// limits them.
    #[serde(
        default = "default_keep_unread_days",
        deserialize_with = "keep_unread_days_or_default"
    )]
    pub keep_unread_days: u32,
    /// Sync when the computer wakes from sleep.
    #[serde(default, deserialize_with = "or_default")]
    pub sync_on_wake: bool,
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
    #[must_use]
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
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            SyncSchedule::Manual => serializer.serialize_str("manual"),
            SyncSchedule::OnStart => serializer.serialize_str("onStart"),
            SyncSchedule::Every(minutes) => serializer.serialize_u32(*minutes),
        }
    }
}

impl<'de> Deserialize<'de> for SyncSchedule {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        match serde_json::Value::deserialize(deserializer)? {
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

/// Half a year: long enough that nothing the user may still read goes, short enough that
/// busy feeds don't grow the cache without end.
pub const DEFAULT_KEEP_UNREAD_DAYS: u32 = 180;

fn default_keep_unread_days() -> u32 {
    DEFAULT_KEEP_UNREAD_DAYS
}

fn keep_unread_days_or_default<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(u32::deserialize(value).unwrap_or(DEFAULT_KEEP_UNREAD_DAYS))
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
    // In this format 0 means manual sync, which also skips the sync on start.
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

impl Config {
    /// Reads the settings file at `path`.
    ///
    /// A missing file gives the defaults. A file from before several accounts is
    /// migrated.
    ///
    /// # Errors
    ///
    /// The reason, when the file is there but can't be read or parsed. It must not be
    /// overwritten before it is kept somewhere.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read(path) {
            Ok(bytes) => Self::parse(&bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        migrate(&mut value);
        let mut config: Self = serde_json::from_value(value).map_err(|e| e.to_string())?;
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
        Ok(config)
    }

    #[must_use]
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

    /// Writes the settings file at `path`.
    ///
    /// Writes to a temporary file first, so a crash never leaves half a file.
    ///
    /// # Errors
    ///
    /// When the file can't be written, flushed to disk or renamed into place.
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
                                 "sync": "often", "keepReadDays": -1, "keepUnreadDays": "x" },
                               { "id": "2", "kind": "nonsense" }],
                 "confirmMarkAllRead": null, "animations": "yes",
                 "shortcuts": { "next": 5 }, "articleFont": "serif" }"#,
        )
        .unwrap();

        assert_eq!(config.accounts.len(), 1);
        assert_eq!(config.accounts[0].sync, SyncSchedule::Every(15));
        assert_eq!(config.accounts[0].keep_read_days, DEFAULT_KEEP_READ_DAYS);
        assert_eq!(
            config.accounts[0].keep_unread_days,
            DEFAULT_KEEP_UNREAD_DAYS
        );
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
                keep_unread_days: DEFAULT_KEEP_UNREAD_DAYS,
                sync_on_wake: false,
            }]
        );
        assert_eq!(config.active.as_deref(), Some(FIRST_ACCOUNT_ID));
        assert!(!config.settings.confirm_mark_all_read);
        assert_eq!(config.clone().take_id(), "2");

        // 0 is manual, not on start.
        let config = Config::parse(
            br#"{ "account": { "server": "https://a.example", "username": "u" },
                 "syncIntervalMinutes": 0 }"#,
        )
        .unwrap();
        assert_eq!(config.accounts[0].sync, SyncSchedule::Manual);

        // Signed out: no account.
        let config = Config::parse(br#"{ "syncIntervalMinutes": 5 }"#).unwrap();
        assert_eq!(config.accounts, []);
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
            keep_unread_days: 0,
            sync_on_wake: true,
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
    fn the_layout_loads_within_its_ranges() {
        let config = Config::parse(b"{}").unwrap();
        assert_eq!(config.layout, Layout::default());
        let config = Config::parse(
            br#"{ "layout": { "sidebarOpen": false, "sidebarWidth": 9999, "listWidth": "wide" } }"#,
        )
        .unwrap();
        assert_eq!(
            config.layout,
            Layout {
                sidebar_open: false,
                sidebar_width: SIDEBAR_WIDTH.max,
                list_width: LIST_WIDTH.default,
            }
        );
        let config = Config::parse(br#"{ "layout": 5 }"#).unwrap();
        assert_eq!(config.layout, Layout::default());

        let mut config = Config::default();
        config.layout.list_width = 400.5;
        let json = serde_json::to_vec(&config).unwrap();
        assert_eq!(Config::parse(&json).unwrap().layout, config.layout);
    }

    #[test]
    fn reading_settings_default_and_round_trip() {
        let config = Config::parse(
            br#"{ "markReadOn": "later", "sortSubscriptions": "alphabetical",
                              "thumbnailSize": "huge", "language": 5,
                              "clock": "h12", "dateFormat": "us" }"#,
        )
        .unwrap();
        assert_eq!(config.settings.thumbnail_size, ThumbnailSize::Medium);
        assert_eq!(config.settings.language, None);
        assert_eq!(config.settings.clock, Clock::H12);
        assert_eq!(config.settings.date_format, DateFormat::System);
        assert_eq!(config.settings.mark_read_on, MarkReadOn::Open);
        assert_eq!(
            config.settings.sort_subscriptions,
            SortSubscriptions::Alphabetical
        );
        assert!(!config.settings.mark_read_on_scroll);
        assert!(config.settings.unread_badge);
        assert_eq!(config.settings.font_size, FontSize::M);
        assert_eq!(config.settings.line_width, LineWidth::Medium);

        let mut config = Config::default();
        config.settings.mark_read_on = MarkReadOn::Delay;
        config.settings.unread_badge = false;
        config.settings.thumbnail_size = ThumbnailSize::Large;
        config.settings.language = Some("sv".into());
        config.settings.clock = Clock::H24;
        config.settings.date_format = DateFormat::DayMonth;
        config.settings.font_size = FontSize::Xxl;
        config.settings.line_width = LineWidth::Narrow;
        let json = serde_json::to_value(&config).unwrap();
        assert_eq!(json["fontSize"], "xxl");
        assert_eq!(json["lineWidth"], "narrow");
        assert_eq!(json["markReadOn"], "delay");
        assert_eq!(json["language"], "sv");
        assert_eq!(json["thumbnailSize"], "large");
        assert_eq!(json["clock"], "h24");
        assert_eq!(json["dateFormat"], "dayMonth");
        assert_eq!(json["sortSubscriptions"], "foldersFirst");
        let back = Config::parse(&serde_json::to_vec(&json).unwrap()).unwrap();
        assert_eq!(back.settings, config.settings);
    }

    #[test]
    fn the_font_size_steps_within_its_range() {
        assert_eq!(FontSize::M.step(), 0);
        assert_eq!(FontSize::Xxxs.step(), -4);
        assert_eq!(FontSize::Xxxl.step(), 4);
        assert_eq!(FontSize::M.larger(), FontSize::L);
        assert_eq!(FontSize::M.smaller(), FontSize::S);
        assert_eq!(FontSize::Xxxl.larger(), FontSize::Xxxl);
        assert_eq!(FontSize::Xxxs.smaller(), FontSize::Xxxs);
        let config = Config::parse(br#"{ "fontSize": "huge", "lineWidth": 3 }"#).unwrap();
        assert_eq!(config.settings.font_size, FontSize::M);
        assert_eq!(config.settings.line_width, LineWidth::Medium);
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
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        Config::load(&path).unwrap();
        std::fs::write(&path, b"{ not json").unwrap();
        Config::load(&path).unwrap_err();
    }
}
