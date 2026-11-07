//! Tauri commands and the background sync. The UI's commands read and write only the
//! local cache. Network work happens in `run_sync`, which reports through the
//! `sync-status` event. One exception: `full_text` fetches a page the user opens in
//! Reader View, while the UI shows the cached feed content.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use skvaller_core::{
    ArticleSummary, Client, Counts, Db, Error, Feed, Folder, ImportReport, Selection, delete_token,
    export_opml as write_opml, import_opml as read_opml, load_full_text, load_token,
    prefetch_full_text, prefetch_thumbnails, refresh_local, save_token,
    subscribe as subscribe_feed, sync,
};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, FilePath};
use tokio::sync::Notify;
use tokio::time::{Instant, sleep_until};
use tracing::Level;

use crate::config::{
    Account, AccountKind, Config, DEFAULT_KEEP_UNREAD_DAYS, FIRST_ACCOUNT_ID, Settings,
    SyncSchedule, Theme, ThumbnailSize,
};
use crate::sync_gate::{Outcome, SyncGate};

const SYNC_STATUS_EVENT: &str = "sync-status";
/// Sent to every window with the new `Settings` when they change.
const SETTINGS_EVENT: &str = "settings-changed";
/// Sent to every window with a new `Session` when an account is added, changed or
/// removed, or another one becomes active.
const ACCOUNTS_EVENT: &str = "accounts-changed";
/// Sent to every window with the account's ID when a feed's settings change.
const FEEDS_EVENT: &str = "feeds-changed";
/// Sent to every window with the account's ID when articles got thumbnails.
const THUMBNAILS_EVENT: &str = "thumbnails-changed";
/// Sent to the Settings window with the pane to show, when it is asked to open one.
const SETTINGS_PANE_EVENT: &str = "settings-pane";
/// The label of the Settings window, in tauri.conf.json.
const SETTINGS_WINDOW: &str = "settings";

pub struct AppState {
    /// Each account's cache is `<id>.db` in here.
    accounts_dir: PathBuf,
    config_path: PathBuf,
    config: Mutex<Config>,
    /// One per account in the config, except those in `broken`.
    runtimes: Mutex<HashMap<String, Arc<Runtime>>>,
    /// Accounts whose cache didn't open.
    broken: Mutex<HashMap<String, BrokenCache>>,
    /// What the user is told about a config.json that couldn't be read, once the main
    /// window is open.
    config_notice: Option<ConfigNotice>,
    /// The pane the Settings window shows when it opens next.
    settings_pane: Mutex<Option<String>>,
    /// Counts config changes, so a window can tell an older session from a newer one
    /// that arrived first.
    revision: AtomicU64,
}

/// Why an account's cache didn't open.
struct BrokenCache {
    message: String,
    /// It is damaged: starting it over is the way out. Not so for one from a newer
    /// version, which that version still reads.
    corrupt: bool,
}

impl BrokenCache {
    fn new(e: &Error) -> Self {
        Self {
            message: e.to_string(),
            corrupt: e.is_corrupt(),
        }
    }
}

/// What an account needs while the app runs: its cache, its client and its syncs.
pub struct Runtime {
    id: String,
    db: Arc<Db>,
    /// Built from the keychain token on first use. `None` until then, after the server
    /// rejects the token, and always for a local account.
    client: Mutex<Option<Client>>,
    /// The server rejected the saved token. Until the next sign-in, no client is built
    /// from the keychain again, so the app shows the sign-in screen instead of retrying
    /// a dead token every sync.
    token_rejected: AtomicBool,
    /// Why the last sync failed, for a window that shows the account later.
    last_error: Mutex<Option<String>>,
    /// A sync found the cache damaged.
    corrupt: AtomicBool,
    sync_gate: SyncGate,
    /// Reader View prefetch, after a sync. Separate, so syncing is not held up by it.
    prefetch_gate: SyncGate,
    /// Fetching thumbnails, after a sync. Apart from Reader View's, which can take
    /// minutes.
    thumbnail_gate: SyncGate,
    /// Wakes the background sync: the schedule changed, or the account was removed.
    wake: Notify,
    /// The account is gone. Work that still holds the runtime stops at its next check.
    removed: AtomicBool,
    /// The computer woke from sleep since the background sync last looked.
    woke: AtomicBool,
}

impl Runtime {
    fn open(dir: &Path, id: &str) -> skvaller_core::Result<Self> {
        Ok(Self {
            id: id.to_owned(),
            db: Arc::new(Db::open(db_path(dir, id))?),
            client: Mutex::new(None),
            token_rejected: AtomicBool::new(false),
            last_error: Mutex::new(None),
            corrupt: AtomicBool::new(false),
            sync_gate: SyncGate::default(),
            prefetch_gate: SyncGate::default(),
            thumbnail_gate: SyncGate::default(),
            wake: Notify::new(),
            removed: AtomicBool::new(false),
            woke: AtomicBool::new(false),
        })
    }

    fn client(&self) -> Option<Client> {
        lock(&self.client).clone()
    }

    fn is_removed(&self) -> bool {
        self.removed.load(Ordering::Relaxed)
    }

    fn status(&self) -> SyncStatus {
        SyncStatus {
            account_id: self.id.clone(),
            syncing: self.sync_gate.is_running(),
            last_sync: self.db.last_sync().ok().flatten(),
            error: lock(&self.last_error).clone(),
            unauthorized: self.token_rejected.load(Ordering::Relaxed),
            changed: false,
            cache_broken: self.corrupt.load(Ordering::Relaxed),
        }
    }
}

fn db_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.db"))
}

/// SQLite's files next to the cache file, then the cache file itself: moved or removed
/// in this order, a cache is never left without its write-ahead log.
fn db_files(path: &Path) -> [PathBuf; 3] {
    let with = |suffix: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    [with("-wal"), with("-shm"), path.to_owned()]
}

/// Where a damaged cache's files go when it starts over: next to it, for the user or a
/// later version to recover from. A later rebuild replaces them.
fn corrupt_path(file: &Path) -> PathBuf {
    let mut name = file.as_os_str().to_owned();
    name.push(".corrupt");
    PathBuf::from(name)
}

/// The local account's subscriptions as OPML, kept next to its cache: they are nowhere
/// else, and a damaged cache starts over from them.
fn feeds_backup_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.opml"))
}

/// The account's cache, any damaged copy of it and its feeds' backup. False if a file
/// stays, such as one still open on Windows.
fn remove_db_files(dir: &Path, id: &str) -> bool {
    let mut removed = true;
    let cache = db_files(&db_path(dir, id));
    let corrupt = cache.iter().map(|f| corrupt_path(f));
    for file in corrupt
        .chain(cache.clone())
        .chain([feeds_backup_path(dir, id)])
    {
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::event!(
                    name: "cache.remove.failure",
                    Level::WARN,
                    file.path = %file.display(),
                    error.message = %e,
                    "removing {{file.path}} failed: {{error.message}}",
                );
                removed = false;
            }
        }
    }
    removed
}

/// A dialog's title and message.
pub struct ConfigNotice {
    pub title: &'static str,
    pub message: String,
}

/// Turns an I/O error into a message that says what failed, and on which path.
fn failed<'a>(action: &'a str, path: &'a Path) -> impl FnOnce(std::io::Error) -> String + 'a {
    move |e| format!("{action} {} failed: {e}", path.display())
}

impl AppState {
    /// Reads config.json and opens each account's cache. Fails, with a message naming
    /// the file, when a folder can't be found, made or read, config.json can't be
    /// backed up or saved, or the old cache can't be moved. An unreadable config.json
    /// falls back to defaults, but only once a copy of it is kept.
    pub fn load(app: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        // Local, not roaming, on Windows: a cache has no business following the user
        // between machines.
        let data_dir = app
            .path()
            .app_local_data_dir()
            .map_err(|e| format!("Finding Skvaller's data folder failed: {e}"))?;
        let config_dir = app
            .path()
            .app_config_dir()
            .map_err(|e| format!("Finding Skvaller's settings folder failed: {e}"))?;
        let accounts_dir = data_dir.join("accounts");
        std::fs::create_dir_all(&accounts_dir).map_err(failed("Creating", &accounts_dir))?;
        std::fs::create_dir_all(&config_dir).map_err(failed("Creating", &config_dir))?;
        let config_path = config_dir.join("config.json");

        let mut notice = None;
        let mut config = match Config::load(&config_path) {
            Ok(config) => config,
            Err(reason) => {
                // Kept for the user to recover, before a save overwrites it. The caches
                // stay as they are.
                let backup = config_path.with_extension("json.bak");
                std::fs::copy(&config_path, &backup).map_err(|e| {
                    format!(
                        "Skvaller can't read {}, and keeping a copy of it failed: {e}",
                        config_path.display()
                    )
                })?;
                tracing::event!(
                    name: "config.read.failure",
                    Level::WARN,
                    file.path = %config_path.display(),
                    backup.path = %backup.display(),
                    error.message = %reason,
                    "reading {{file.path}} failed, so it was kept as {{backup.path}} and \
                     the settings reset: {{error.message}}",
                );
                notice = Some(ConfigNotice {
                    title: "Settings Reset",
                    message: format!(
                        "Skvaller couldn't read its settings file, so your accounts and \
                         settings were reset, and you'll need to add your accounts again. \
                         The old file is kept at {}.",
                        backup.display()
                    ),
                });
                Config::default()
            }
        };
        for account in &mut config.accounts {
            if account.description.is_empty() {
                account.description = default_description(&account.kind);
            }
        }
        // The one account from before several accounts keeps its cache.
        let old_cache = data_dir.join("cache.db");
        if config.account(FIRST_ACCOUNT_ID).is_some()
            && old_cache.exists()
            && !db_path(&accounts_dir, FIRST_ACCOUNT_ID).exists()
        {
            let new = db_files(&db_path(&accounts_dir, FIRST_ACCOUNT_ID));
            for (from, to) in db_files(&old_cache).iter().zip(&new) {
                if from.exists() {
                    std::fs::rename(from, to).map_err(|e| {
                        format!("Moving {} to {} failed: {e}", from.display(), to.display())
                    })?;
                }
            }
        }
        // Removed accounts whose caches could not be deleted then.
        let removed = std::mem::take(&mut config.removed);
        config.removed = removed
            .into_iter()
            .filter(|id| config.account(id).is_none() && !remove_db_files(&accounts_dir, id))
            .collect();
        // No new account takes the ID of a cache on disk, even one a lost config.json
        // knew of.
        let on_disk_ids = std::fs::read_dir(&accounts_dir)
            .map_err(failed("Reading", &accounts_dir))?
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_str()?
                    .split('.')
                    .next()?
                    .parse::<u64>()
                    .ok()
            });
        config.next_id = config.next_id.max(on_disk_ids.max().map_or(0, |n| n + 1));
        // Saved only when something changed, so a file that reads back the same is left
        // as it is.
        let raw: Option<serde_json::Value> = std::fs::read(&config_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        // An account entry this version can't read, such as a newer one's, would go
        // with the save: keep the file as it was.
        let listed = raw
            .as_ref()
            .and_then(|r| r.get("accounts")?.as_array().map(Vec::len))
            .unwrap_or(0);
        // Not after a reset: then every account was left out, and the file is kept.
        if notice.is_none() && listed > config.accounts.len() {
            let backup = config_path.with_extension("json.bak");
            notice = Some(ConfigNotice {
                title: "Accounts Left Out",
                message: format!(
                    "Skvaller couldn't read some accounts in its settings file, perhaps \
                     because a newer version saved them, so it left them out. The old file \
                     is kept at {}.",
                    backup.display()
                ),
            });
            std::fs::copy(&config_path, &backup).map_err(|e| {
                format!(
                    "Skvaller can't read some accounts in {}, and keeping a copy of it failed: {e}",
                    config_path.display()
                )
            })?;
            tracing::event!(
                name: "config.accounts.read.failure",
                Level::WARN,
                file.path = %config_path.display(),
                backup.path = %backup.display(),
                "some accounts in {{file.path}} can't be read, so they were left out and \
                 the file kept as {{backup.path}}",
            );
        }
        if serde_json::to_value(&config).ok() != raw {
            config
                .save(&config_path)
                .map_err(failed("Saving", &config_path))?;
        }

        // A cache that doesn't open, such as one from a newer version, takes only its
        // own account out.
        let (mut runtimes, mut broken) = (HashMap::new(), HashMap::new());
        for account in &config.accounts {
            match open_runtime(&accounts_dir, account) {
                Ok(runtime) => {
                    runtimes.insert(account.id.clone(), Arc::new(runtime));
                }
                Err(e) => {
                    tracing::event!(
                        name: "cache.open.failure",
                        Level::ERROR,
                        account.id = %account.id,
                        error.message = %e,
                        "opening the cache of account {{account.id}} failed: {{error.message}}",
                    );
                    broken.insert(account.id.clone(), BrokenCache::new(&e));
                }
            }
        }
        Ok(Self {
            accounts_dir,
            config: Mutex::new(config),
            config_path,
            runtimes: Mutex::new(runtimes),
            broken: Mutex::new(broken),
            config_notice: notice,
            settings_pane: Mutex::new(None),
            revision: AtomicU64::new(0),
        })
    }

    /// Set when config.json, or some accounts in it, couldn't be read at startup.
    pub fn config_notice(&self) -> Option<&ConfigNotice> {
        self.config_notice.as_ref()
    }

    fn config(&self) -> Config {
        lock(&self.config).clone()
    }

    /// The accounts, in order, and the active one's ID.
    pub fn accounts_for_menu(&self) -> (Vec<Account>, Option<String>) {
        let config = lock(&self.config);
        (config.accounts.clone(), config.active.clone())
    }

    pub fn theme(&self) -> Theme {
        lock(&self.config).settings.theme
    }

    /// Changes the config and saves it, under the lock, so concurrent changes to
    /// different fields don't undo each other. Returns what `change` returns.
    fn update_config<T>(&self, change: impl FnOnce(&mut Config) -> T) -> CommandResult<T> {
        let mut config = lock(&self.config);
        let mut new = config.clone();
        let result = change(&mut new);
        new.save(&self.config_path).map_err(CommandError::other)?;
        *config = new;
        self.revision.fetch_add(1, Ordering::SeqCst);
        Ok(result)
    }

    /// The account's runtime, if it still exists.
    fn runtime(&self, id: &str) -> CommandResult<Arc<Runtime>> {
        if let Some(rt) = lock(&self.runtimes).get(id) {
            return Ok(rt.clone());
        }
        Err(CommandError::other(match lock(&self.broken).get(id) {
            Some(broken) => format!("this account's cache can't be opened: {}", broken.message),
            None => "the account was removed".to_owned(),
        }))
    }

    fn runtimes(&self) -> Vec<Arc<Runtime>> {
        lock(&self.runtimes).values().cloned().collect()
    }
}

/// The name an account gets until the user changes it: "FreshRSS" or the server's host
/// name, or Local.
fn default_description(kind: &AccountKind) -> String {
    match kind {
        AccountKind::Greader { server, .. } => {
            let rest = server
                .split_once("://")
                .map_or(server.as_str(), |(_, rest)| rest);
            let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
            if path.trim_end_matches('/').ends_with("api/greader.php") {
                "FreshRSS".to_owned()
            } else {
                let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
                host.split(':').next().unwrap_or(host).to_owned()
            }
        }
        AccountKind::Local => "Local".to_owned(),
    }
}

/// Shows the unread articles of every account on the app icon, or nothing when the
/// setting is off or there are none: a badge count on macOS and Linux, and on Windows,
/// which has none, a drawn overlay on the taskbar button.
pub fn update_badge(app: &AppHandle) {
    let state = app.state::<AppState>();
    let count: i64 = if state.config().settings.unread_badge {
        state
            .runtimes()
            .iter()
            .filter_map(|rt| rt.db.unread_total().ok())
            .sum()
    } else {
        0
    };
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    #[cfg(not(target_os = "windows"))]
    let shown = window.set_badge_count((count > 0).then_some(count));
    #[cfg(target_os = "windows")]
    let shown = window.set_overlay_icon((count > 0).then(|| {
        tauri::image::Image::new_owned(
            crate::badge::draw(count),
            crate::badge::SIZE,
            crate::badge::SIZE,
        )
    }));
    if let Err(e) = shown {
        tracing::event!(
            name: "badge.set.failure",
            Level::WARN,
            error.message = %e,
            "setting the unread badge failed: {{error.message}}",
        );
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// An error as the UI receives it. `kind` is what the UI branches on.
#[derive(Debug, Serialize)]
pub struct CommandError {
    kind: &'static str,
    message: String,
}

impl CommandError {
    fn other(message: impl ToString) -> Self {
        Self {
            kind: "other",
            message: message.to_string(),
        }
    }
}

impl From<Error> for CommandError {
    fn from(e: Error) -> Self {
        let kind = match e {
            Error::Unauthorized => "unauthorized",
            Error::BadUrl => "badUrl",
            Error::Http(_) => "network",
            Error::NoFeed => "noFeed",
            _ if e.is_corrupt() => "corrupt",
            _ => "other",
        };
        Self {
            kind,
            message: e.to_string(),
        }
    }
}

type CommandResult<T> = Result<T, CommandError>;

/// Runs keychain calls off the async runtime: they block, and may wait for the user
/// to unlock the keychain.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> skvaller_core::Result<T> + Send + 'static,
) -> CommandResult<T> {
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(CommandError::other)?
        .map_err(Into::into)
}

/// The API client for the account, if it syncs with a server and has a token in the
/// keychain.
async fn ensure_client(state: &AppState, rt: &Runtime) -> CommandResult<Option<Client>> {
    if let Some(client) = rt.client() {
        return Ok(Some(client));
    }
    if rt.token_rejected.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let Some(account) = state.config().account(&rt.id).cloned() else {
        return Ok(None);
    };
    let AccountKind::Greader { server, .. } = &account.kind else {
        return Ok(None);
    };
    let key = account
        .kind
        .keychain_key()
        .expect("a sync account has a keychain entry");
    let Some(token) = blocking(move || load_token(&key)).await? else {
        return Ok(None);
    };
    let client = Client::new(server, &token)?;
    // Signing in may have happened while the token loaded. Keep this client only if
    // nothing replaced it and the account is still there.
    let mut current = lock(&rt.client);
    if let Some(current) = current.as_ref() {
        return Ok(Some(current.clone()));
    }
    if rt.removed.load(Ordering::Relaxed) || rt.token_rejected.load(Ordering::Relaxed) {
        return Ok(None);
    }
    *current = Some(client.clone());
    Ok(Some(client))
}

/// Whether the account can sync: a local one always can, a sync account needs a token
/// the server has not rejected.
async fn is_signed_in(state: &AppState, rt: &Runtime) -> CommandResult<bool> {
    match state.config().account(&rt.id).map(|a| &a.kind) {
        Some(AccountKind::Local) => Ok(true),
        Some(AccountKind::Greader { .. }) => Ok(ensure_client(state, rt).await?.is_some()),
        None => Ok(false),
    }
}

/// An account as the UI receives it: the saved fields and its unread count.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountInfo {
    #[serde(flatten)]
    account: Account,
    unread: i64,
}

fn account_infos(state: &AppState) -> Vec<AccountInfo> {
    let runtimes = lock(&state.runtimes).clone();
    state
        .config()
        .accounts
        .into_iter()
        .map(|account| AccountInfo {
            unread: runtimes
                .get(&account.id)
                .and_then(|rt| rt.db.unread_total().ok())
                .unwrap_or(0),
            account,
        })
        .collect()
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// Higher for a session made from a newer config.
    revision: u64,
    accounts: Vec<AccountInfo>,
    /// The account the main window shows. `None` only without accounts.
    active_id: Option<String>,
    /// The active account can sync. False when its token was rejected, which shows the
    /// sign-in screen, filled in for it.
    signed_in: bool,
    /// The active account's.
    status: SyncStatus,
    settings: Settings,
}

async fn current_session(state: &AppState) -> CommandResult<Session> {
    let (config, revision) = {
        let config = lock(&state.config);
        (config.clone(), state.revision.load(Ordering::SeqCst))
    };
    let active = config.active.as_deref().map(|id| state.runtime(id));
    let (signed_in, status) = match active {
        // A keychain that can't be read shows the sign-in screen with the reason, rather
        // than failing the session and, with it, every window's account list.
        Some(Ok(rt)) => match is_signed_in(state, &rt).await {
            Ok(signed_in) => (signed_in, rt.status()),
            Err(e) => (
                false,
                SyncStatus {
                    error: Some(e.message),
                    ..rt.status()
                },
            ),
        },
        // Its cache didn't open: the main window says why.
        Some(Err(e)) => {
            let id = config.active.clone().unwrap_or_default();
            let cache_broken = lock(&state.broken).get(&id).is_some_and(|b| b.corrupt);
            (
                true,
                SyncStatus {
                    account_id: id,
                    error: Some(e.message),
                    cache_broken,
                    ..SyncStatus::default()
                },
            )
        }
        None => (false, SyncStatus::default()),
    };
    Ok(Session {
        revision,
        accounts: account_infos(state),
        active_id: config.active,
        signed_in,
        status,
        settings: config.settings,
    })
}

/// Tells every window about the accounts.
async fn emit_session(app: &AppHandle) {
    match current_session(&app.state::<AppState>()).await {
        Ok(session) => {
            let _ = app.emit(ACCOUNTS_EVENT, session);
            crate::menu::rebuild_accounts(app);
        }
        Err(e) => tracing::event!(
            name: "session.read.failure",
            Level::ERROR,
            error.message = %e.message,
            "reading the accounts failed: {{error.message}}",
        ),
    }
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
    /// The account it is about.
    account_id: String,
    syncing: bool,
    /// Unix milliseconds.
    last_sync: Option<i64>,
    /// Why the last sync failed.
    error: Option<String>,
    /// The server rejected the token: the user has to sign in again.
    unauthorized: bool,
    /// The sync that just finished changed the cached articles or the feeds they are in,
    /// so the article list needs loading again.
    changed: bool,
    /// The cache is damaged. `rebuild_cache` starts it over.
    cache_broken: bool,
}

#[tauri::command]
pub async fn session(state: State<'_, AppState>) -> CommandResult<Session> {
    current_session(&state).await
}

#[tauri::command]
pub async fn accounts(state: State<'_, AppState>) -> CommandResult<Vec<AccountInfo>> {
    Ok(account_infos(&state))
}

/// Signs in, saves the token in the keychain and makes the account the active one. An
/// account already added, such as one whose token the server rejected, gets the new
/// token; any other becomes a new account, which syncs at once.
#[tauri::command]
pub async fn sign_in(
    app: AppHandle,
    state: State<'_, AppState>,
    server: String,
    username: String,
    password: String,
) -> CommandResult<Session> {
    let username = username.trim().to_owned();
    let client = Client::login(server.trim(), &username, &password).await?;
    // The API URL that answered, which may have FreshRSS's path added.
    let server = client.base().to_owned();
    let kind = AccountKind::Greader { server, username };
    let (key, token) = (
        kind.keychain_key()
            .expect("a sync account has a keychain entry"),
        client.token().to_owned(),
    );
    blocking(move || save_token(&key, &token)).await?;

    let (rt, added) = add_account(&app, &state, kind)?;
    if added {
        // Its sync loop has started its first sync with the saved token: no need to
        // stop it.
        *lock(&rt.client) = Some(client);
        emit_session(&app).await;
        return current_session(&state).await;
    }
    {
        // A sync still running with the old token would reject the new one when it fails.
        let _no_sync = rt.sync_gate.stop().await;
        *lock(&rt.client) = Some(client);
        rt.token_rejected.store(false, Ordering::Relaxed);
        *lock(&rt.last_error) = None;
    }
    // Windows may still have the rejected status: this one replaces it.
    let _ = app.emit(SYNC_STATUS_EVENT, rt.status());

    emit_session(&app).await;
    let sync_app = app.clone();
    tauri::async_runtime::spawn(async move { run_sync(&sync_app, &rt).await });
    current_session(&state).await
}

/// Makes the account of `kind` active, adding it first if there is none: then it gets
/// a new ID and its background sync starts. Says whether it was added.
fn add_account(
    app: &AppHandle,
    state: &AppState,
    kind: AccountKind,
) -> CommandResult<(Arc<Runtime>, bool)> {
    let existing = |config: &Config| {
        config
            .accounts
            .iter()
            .find(|a| a.kind == kind)
            .map(|a| a.id.clone())
    };
    // The ID is taken under the config's lock, so two additions never share one.
    let id = state.update_config(|config| match existing(config) {
        Some(id) => {
            config.active = Some(id.clone());
            Err(id)
        }
        None => Ok(config.take_id()),
    })?;
    let id = match id {
        Ok(id) => id,
        Err(existing) => return Ok((state.runtime(&existing)?, false)),
    };
    let rt = Arc::new(Runtime::open(&state.accounts_dir, &id)?);
    let added = state.update_config(|config| {
        // Added meanwhile, by a second click: that one stays.
        if let Some(other) = existing(config) {
            config.active = Some(other.clone());
            return Err(other);
        }
        let mut description = default_description(&kind);
        if config.accounts.iter().any(|a| a.description == description)
            && let AccountKind::Greader { username, .. } = &kind
        {
            description = format!("{description} ({username})");
        }
        config.accounts.push(Account {
            id: id.clone(),
            description,
            kind: kind.clone(),
            sync: SyncSchedule::default(),
            keep_read_days: skvaller_core::DEFAULT_KEEP_READ_DAYS,
            keep_unread_days: DEFAULT_KEEP_UNREAD_DAYS,
            sync_on_wake: false,
        });
        config.active = Some(id.clone());
        Ok(())
    });
    match added {
        Ok(Ok(())) => {
            lock(&state.runtimes).insert(id, rt.clone());
            start_sync_loop(app.clone(), rt.clone());
            Ok((rt, true))
        }
        Ok(Err(other)) => {
            drop(rt);
            remove_db_files(&state.accounts_dir, &id);
            Ok((state.runtime(&other)?, false))
        }
        Err(e) => {
            drop(rt);
            remove_db_files(&state.accounts_dir, &id);
            Err(e)
        }
    }
}

/// Adds the local account, whose feeds Skvaller fetches itself, and makes it active.
/// There is at most one: with one already, that one becomes active.
#[tauri::command]
pub async fn add_local_account(
    app: AppHandle,
    state: State<'_, AppState>,
) -> CommandResult<Session> {
    add_account(&app, &state, AccountKind::Local)?;
    emit_session(&app).await;
    current_session(&state).await
}

/// Shows another account in the main window. Every window gets the new session too.
#[tauri::command]
pub async fn set_active_account(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> CommandResult<Session> {
    state.runtime(&id)?;
    // Checked again under the lock, in case the account was removed meanwhile.
    let exists = state.update_config(|config| {
        let exists = config.account(&id).is_some();
        if exists {
            config.active = Some(id);
        }
        exists
    })?;
    if !exists {
        return Err(CommandError::other("the account was removed"));
    }
    let session = current_session(&state).await?;
    let _ = app.emit(ACCOUNTS_EVENT, session.clone());
    crate::menu::rebuild_accounts(&app);
    Ok(session)
}

/// Changes to an account's name or sync settings. Fields left out stay.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountChange {
    description: Option<String>,
    sync: Option<SyncSchedule>,
    keep_read_days: Option<u32>,
    keep_unread_days: Option<u32>,
    sync_on_wake: Option<bool>,
}

#[tauri::command]
pub async fn update_account(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    change: AccountChange,
) -> CommandResult<()> {
    let AccountChange {
        description,
        sync,
        keep_read_days,
        keep_unread_days,
        sync_on_wake,
    } = change;
    let rt = state.runtime(&id)?;
    let mut schedule_changed = false;
    state.update_config(|config| {
        if let Some(account) = config.accounts.iter_mut().find(|a| a.id == id) {
            if let Some(description) = description.map(|d| d.trim().to_owned())
                && !description.is_empty()
            {
                account.description = description;
            }
            if let Some(sync) = sync {
                schedule_changed = account.sync != sync;
                account.sync = sync;
            }
            if let Some(days) = keep_read_days {
                account.keep_read_days = days.max(1);
            }
            if let Some(days) = keep_unread_days {
                account.keep_unread_days = days;
            }
            if let Some(on) = sync_on_wake {
                // So a sync after waking it asked for stops at once.
                schedule_changed |= account.sync_on_wake != on;
                account.sync_on_wake = on;
            }
        }
    })?;
    if schedule_changed {
        rt.wake.notify_one();
    }
    emit_session(&app).await;
    Ok(())
}

/// Removes the account: from the config, its token from the keychain and its cache,
/// queued changes included. Another account becomes active. Without any left, the
/// Settings window, where removing happens, closes.
///
/// Once the account is gone from the config, it is removed. Removing the token and the
/// cache after that is best effort: a failure is logged, and a cache left behind, such as
/// one still open on Windows, is removed on the next start.
#[tauri::command]
pub async fn remove_account(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> CommandResult<()> {
    let account = state
        .config()
        .account(&id)
        .cloned()
        .ok_or_else(|| CommandError::other("the account was removed"))?;
    // One whose cache didn't open has no runtime.
    let rt = lock(&state.runtimes).get(&id).cloned();
    {
        // No sync may write while the cache goes.
        let _no_sync = match &rt {
            Some(rt) => Some((
                rt.sync_gate.stop().await,
                rt.prefetch_gate.stop().await,
                rt.thumbnail_gate.stop().await,
            )),
            None => None,
        };
        state.update_config(|config| {
            config.accounts.retain(|a| a.id != id);
            if config.active.as_deref() == Some(id.as_str()) {
                config.active = config.accounts.first().map(|a| a.id.clone());
            }
        })?;
        // Only once saved: until then, the account goes on as before.
        lock(&state.runtimes).remove(&id);
        lock(&state.broken).remove(&id);
        if let Some(rt) = &rt {
            rt.removed.store(true, Ordering::Relaxed);
            rt.wake.notify_one();
            *lock(&rt.client) = None;
        }
    }
    drop(rt);
    let account = Some(account);

    if let Some(key) = account.and_then(|a| a.kind.keychain_key())
        && let Err(e) = blocking(move || delete_token(&key)).await
    {
        tracing::event!(
            name: "account.token.remove.failure",
            Level::WARN,
            error.message = %e.message,
            "removing an account's token failed: {{error.message}}",
        );
    }
    update_badge(&app);
    if !remove_db_files(&state.accounts_dir, &id)
        && let Err(e) = state.update_config(|config| config.removed.push(id.clone()))
    {
        tracing::event!(
            name: "account.cache.remove.failure",
            Level::WARN,
            account.id = %id,
            error.message = %e.message,
            "the cache of removed account {{account.id}} stays on disk: {{error.message}}",
        );
    }

    emit_session(&app).await;
    if state.config().accounts.is_empty()
        && let Some(window) = app.get_webview_window(SETTINGS_WINDOW)
    {
        let _ = window.close();
    }
    Ok(())
}

/// Syncs the account now, unless it is syncing already. The outcome arrives as a
/// `sync-status` event.
#[tauri::command]
pub async fn sync_now(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> CommandResult<()> {
    let rt = state.runtime(&account_id)?;
    run_sync(&app, &rt).await;
    Ok(())
}

#[derive(Serialize)]
pub struct Library {
    folders: Vec<Folder>,
    feeds: Vec<Feed>,
    counts: Counts,
}

/// Folders, feeds and unread and starred counts, for the sidebar.
///
/// This and the other commands for the main window name the account they are for, so
/// one sent just before a switch of accounts still acts on its own.
#[tauri::command]
pub async fn library(state: State<'_, AppState>, account_id: String) -> CommandResult<Library> {
    let db = &state.runtime(&account_id)?.db;
    Ok(Library {
        folders: db.folders()?,
        feeds: db.feeds()?,
        counts: db.counts()?,
    })
}

#[tauri::command]
pub async fn articles(
    state: State<'_, AppState>,
    account_id: String,
    selection: Selection,
) -> CommandResult<Vec<ArticleSummary>> {
    Ok(state.runtime(&account_id)?.db.articles(&selection)?)
}

#[tauri::command]
pub async fn article_html(
    state: State<'_, AppState>,
    account_id: String,
    id: String,
) -> CommandResult<Option<String>> {
    Ok(state.runtime(&account_id)?.db.article_html(&id)?)
}

/// The article's Reader View text, extracted from its web page: cached, or else
/// fetched. The UI shows the feed's content meanwhile. A page given up on is fetched
/// again only with `retry`, when the user asks. Untrusted, like the feed's HTML.
#[tauri::command]
pub async fn full_text(
    state: State<'_, AppState>,
    account_id: String,
    id: String,
    retry: bool,
) -> CommandResult<String> {
    let rt = state.runtime(&account_id)?;
    load_full_text(&rt.db, &id, retry)
        .await?
        .ok_or_else(|| CommandError::other("the page has nothing to show in Reader View"))
}

/// Opens the feed's articles in Reader View, or stops doing so.
#[tauri::command]
pub async fn set_feed_reader_view(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    feed_id: String,
    on: bool,
) -> CommandResult<()> {
    state
        .runtime(&account_id)?
        .db
        .set_feed_reader_view(&feed_id, on)?;
    let _ = app.emit(FEEDS_EVENT, &account_id);
    Ok(())
}

#[tauri::command]
pub async fn set_read(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    ids: Vec<String>,
    read: bool,
) -> CommandResult<()> {
    state.runtime(&account_id)?.db.set_read(&ids, read)?;
    update_badge(&app);
    Ok(())
}

#[tauri::command]
pub async fn set_starred(
    state: State<'_, AppState>,
    account_id: String,
    ids: Vec<String>,
    starred: bool,
) -> CommandResult<()> {
    Ok(state.runtime(&account_id)?.db.set_starred(&ids, starred)?)
}

/// The account, if it is the local one: only its feeds and folders are managed here. A
/// sync account's are managed on its server.
fn local(state: &AppState, account_id: &str) -> CommandResult<Arc<Runtime>> {
    let rt = state.runtime(account_id)?;
    match state.config().account(account_id).map(|a| &a.kind) {
        Some(AccountKind::Local) => Ok(rt),
        _ => Err(CommandError::other(
            "this account's feeds are managed on its server",
        )),
    }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Subscribes the local account to the feed at `url`, or the one a web page there
/// links to. Returns the feed's ID. Fetches it, so it can take seconds.
#[tauri::command]
pub async fn subscribe(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    url: String,
    folder_id: Option<String>,
) -> CommandResult<String> {
    let rt = local(&state, &account_id)?;
    let id = subscribe_feed(&rt.db, &url, folder_id.as_deref(), now_secs()).await?;
    back_up_feeds(&state, &rt);
    update_badge(&app);
    // Its articles are on screen now, not at the next sync.
    let app = app.clone();
    tauri::async_runtime::spawn(async move { fetch_thumbnails(&app, &rt).await });
    Ok(id)
}

/// Removes a feed of the local account, with its articles.
#[tauri::command]
pub async fn unsubscribe(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    feed_id: String,
) -> CommandResult<()> {
    let rt = local(&state, &account_id)?;
    rt.db.unsubscribe(&feed_id)?;
    back_up_feeds(&state, &rt);
    update_badge(&app);
    Ok(())
}

#[tauri::command]
pub async fn rename_feed(
    state: State<'_, AppState>,
    account_id: String,
    feed_id: String,
    title: String,
) -> CommandResult<()> {
    let title = non_empty(&title)?;
    let rt = local(&state, &account_id)?;
    rt.db.rename_feed(&feed_id, title)?;
    back_up_feeds(&state, &rt);
    Ok(())
}

/// Moves a feed of the local account into a folder, or out of any with `None`.
#[tauri::command]
pub async fn move_feed(
    state: State<'_, AppState>,
    account_id: String,
    feed_id: String,
    folder_id: Option<String>,
) -> CommandResult<()> {
    let rt = local(&state, &account_id)?;
    rt.db.move_feed(&feed_id, folder_id.as_deref())?;
    back_up_feeds(&state, &rt);
    Ok(())
}

/// The local account's folder with this title, or else a new one. Returns its ID.
#[tauri::command]
pub async fn create_folder(
    state: State<'_, AppState>,
    account_id: String,
    title: String,
) -> CommandResult<String> {
    let title = non_empty(&title)?;
    Ok(local(&state, &account_id)?.db.create_folder(title)?)
}

#[tauri::command]
pub async fn rename_folder(
    state: State<'_, AppState>,
    account_id: String,
    folder_id: String,
    title: String,
) -> CommandResult<()> {
    let title = non_empty(&title)?;
    let rt = local(&state, &account_id)?;
    rt.db.rename_folder(&folder_id, title)?;
    back_up_feeds(&state, &rt);
    Ok(())
}

/// Removes a folder of the local account. Its feeds stay, in no folder.
#[tauri::command]
pub async fn delete_folder(
    state: State<'_, AppState>,
    account_id: String,
    folder_id: String,
) -> CommandResult<()> {
    let rt = local(&state, &account_id)?;
    rt.db.delete_folder(&folder_id)?;
    back_up_feeds(&state, &rt);
    Ok(())
}

/// Writes the local account's subscriptions to their backup next to its cache, unless
/// they are there as they are. Does nothing for a sync account, whose server has them.
/// Best effort: a failure is logged.
fn back_up_feeds(state: &AppState, rt: &Runtime) {
    let Some(account) = state.config().account(&rt.id).cloned() else {
        return;
    };
    if account.kind != AccountKind::Local {
        return;
    }
    let path = feeds_backup_path(&state.accounts_dir, &rt.id);
    let written = write_opml(&rt.db, &account.description)
        .map_err(|e| e.to_string())
        .and_then(|xml| {
            if std::fs::read_to_string(&path).is_ok_and(|old| old == xml) {
                return Ok(());
            }
            // Renamed into place, so a crash midway leaves the old backup whole.
            let partial = path.with_extension("opml.partial");
            std::fs::write(&partial, xml)
                .and_then(|()| std::fs::rename(&partial, &path))
                .map_err(|e| e.to_string())
        });
    if let Err(e) = written {
        tracing::event!(
            name: "feeds.backup.failure",
            Level::WARN,
            account.id = %rt.id,
            error.message = %e,
            "backing up the feeds of account {{account.id}} failed: {{error.message}}",
        );
    }
}

/// The account's runtime, at startup. A local account whose cache has no feeds, as when
/// a new version's cache started over or the file went, gets them back from their
/// backup, before a sync backs up the empty list over it. One the user emptied has an
/// empty backup.
fn open_runtime(dir: &Path, account: &Account) -> skvaller_core::Result<Runtime> {
    let runtime = Runtime::open(dir, &account.id)?;
    if account.kind == AccountKind::Local && runtime.db.feeds().is_ok_and(|f| f.is_empty()) {
        restore_feeds(dir, &account.id, &runtime.db);
    }
    Ok(runtime)
}

/// Gives the local account's emptied cache its feeds back from their backup, if there
/// is one. Best effort: a failure is logged.
fn restore_feeds(dir: &Path, account_id: &str, db: &Db) {
    let restored = match std::fs::read_to_string(feeds_backup_path(dir, account_id)) {
        Ok(xml) => read_opml(db, &xml).map(drop).map_err(|e| e.to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    };
    if let Err(e) = restored {
        tracing::event!(
            name: "feeds.restore.failure",
            Level::ERROR,
            account.id = %account_id,
            error.message = %e,
            "restoring the feeds of account {{account.id}} failed: {{error.message}}",
        );
    }
}

/// Starts a damaged cache over: its files move aside, an empty cache takes their place,
/// and the account syncs. A sync account gets its articles from the server again;
/// the local account gets its feeds from their backup. Changes queued in the damaged
/// cache are lost.
#[tauri::command]
pub async fn rebuild_cache(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
) -> CommandResult<()> {
    let account = state
        .config()
        .account(&account_id)
        .cloned()
        .ok_or_else(|| CommandError::other("the account was removed"))?;
    if lock(&state.broken)
        .get(&account_id)
        .is_some_and(|b| !b.corrupt)
    {
        return Err(CommandError::other("this account's cache isn't damaged"));
    }
    let old = lock(&state.runtimes).get(&account_id).cloned();
    {
        // No sync may write while the files move.
        let _no_sync = match &old {
            Some(rt) => Some((
                rt.sync_gate.stop().await,
                rt.prefetch_gate.stop().await,
                rt.thumbnail_gate.stop().await,
            )),
            None => None,
        };
        lock(&state.runtimes).remove(&account_id);
        if let Some(rt) = &old {
            rt.removed.store(true, Ordering::Relaxed);
            rt.wake.notify_one();
        }
    }
    drop(old);

    let dir = &state.accounts_dir;
    let rebuilt = db_files(&db_path(dir, &account_id))
        .iter()
        .filter(|file| file.exists())
        .try_for_each(|file| std::fs::rename(file, corrupt_path(file)))
        .map_err(|e| Error::Local(e.to_string()))
        .and_then(|()| Runtime::open(dir, &account_id));
    let rt = match rebuilt {
        Ok(rt) => Arc::new(rt),
        Err(e) => {
            // Such as a file still open on Windows. The next start tries it again.
            let message = format!("starting the cache over failed: {e}");
            lock(&state.broken).insert(
                account_id,
                BrokenCache {
                    message: message.clone(),
                    corrupt: true,
                },
            );
            emit_session(&app).await;
            return Err(CommandError::other(message));
        }
    };
    if account.kind == AccountKind::Local {
        restore_feeds(dir, &account_id, &rt.db);
    }
    lock(&state.broken).remove(&account_id);
    lock(&state.runtimes).insert(account_id, rt.clone());
    start_sync_loop(app.clone(), rt.clone());
    update_badge(&app);
    emit_session(&app).await;
    // With a manual schedule, the loop doesn't sync on its own.
    tauri::async_runtime::spawn(async move { run_sync(&app, &rt).await });
    Ok(())
}

/// Larger OPML files are not read. A thousand feeds take a few hundred kilobytes.
const MAX_OPML_BYTES: u64 = 10 * 1024 * 1024;

/// Asks for an OPML file and adds its feeds to the local account `id`, which then
/// refreshes. `None` if the user cancelled.
#[tauri::command]
pub async fn import_opml(
    app: AppHandle,
    state: State<'_, AppState>,
    window: WebviewWindow,
    id: String,
) -> CommandResult<Option<ImportReport>> {
    local(&state, &id)?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_parent(&window)
        .add_filter("OPML", &["opml", "xml"])
        .pick_file(|path| {
            let _ = tx.send(path);
        });
    let Some(path) = rx.await.ok().flatten() else {
        return Ok(None);
    };
    // The account may have gone while the dialog was open.
    let rt = local(&state, &id)?;
    let path = file_path(path)?;
    let too_large = std::fs::metadata(&path).is_ok_and(|m| m.len() > MAX_OPML_BYTES);
    if too_large {
        return Err(CommandError::other(
            "the file is too large for an OPML file",
        ));
    }
    // Reading and adding thousands of feeds takes a moment: off the async runtime.
    let db = rt.db.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        let xml = std::fs::read(&path).map_err(CommandError::other)?;
        read_opml(&db, &String::from_utf8_lossy(&xml)).map_err(CommandError::from)
    })
    .await
    .map_err(CommandError::other)??;
    back_up_feeds(&state, &rt);
    if report.added > 0 {
        tauri::async_runtime::spawn(async move {
            // After any refresh that is running, which started before the import.
            while rt.sync_gate.is_running() && !rt.is_removed() {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
            run_sync(&app, &rt).await;
        });
    }
    Ok(Some(report))
}

/// Asks where to save, and writes the account's subscriptions there as OPML. False if
/// the user cancelled.
#[tauri::command]
pub async fn export_opml(
    app: AppHandle,
    state: State<'_, AppState>,
    window: WebviewWindow,
    id: String,
) -> CommandResult<bool> {
    let rt = state.runtime(&id)?;
    let description = state
        .config()
        .account(&id)
        .map(|a| a.description.clone())
        .unwrap_or_default();
    let xml = write_opml(&rt.db, &description)?;
    // No characters a file system refuses in the suggested name.
    let name: String = description
        .chars()
        .map(|c| if "/\\:*?\"<>|".contains(c) { '-' } else { c })
        .collect();
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_parent(&window)
        .add_filter("OPML", &["opml"])
        .set_file_name(format!("{} Subscriptions.opml", name.trim()))
        .save_file(|path| {
            let _ = tx.send(path);
        });
    let Some(path) = rx.await.ok().flatten() else {
        return Ok(false);
    };
    std::fs::write(file_path(path)?, xml).map_err(CommandError::other)?;
    Ok(true)
}

fn file_path(path: FilePath) -> CommandResult<PathBuf> {
    path.into_path().map_err(CommandError::other)
}

fn non_empty(title: &str) -> CommandResult<&str> {
    let title = title.trim();
    if title.is_empty() {
        return Err(CommandError::other("the name is empty"));
    }
    Ok(title)
}

/// Saves the settings and sends them to every window.
#[tauri::command]
pub async fn set_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> CommandResult<()> {
    let (mut theme_changed, mut badge_changed) = (false, false);
    state.update_config(|config| {
        theme_changed = config.settings.theme != settings.theme;
        badge_changed = config.settings.unread_badge != settings.unread_badge;
        config.settings = settings.clone();
    })?;
    if theme_changed {
        app.set_theme(settings.theme.native());
    }
    if badge_changed {
        update_badge(&app);
    }
    // Thumbnails turned off: no image host hears from the app from now on, not even for
    // the rest of a run under way.
    if settings.thumbnail_size == ThumbnailSize::None {
        for rt in state.runtimes() {
            drop(rt.thumbnail_gate.stop().await);
        }
    }
    let _ = app.emit(SETTINGS_EVENT, settings);
    Ok(())
}

/// Opens the Settings window, or brings it to the front when it is open, showing `pane`
/// if given. Async because creating a window from a synchronous command deadlocks on
/// Windows.
#[tauri::command]
pub async fn open_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    pane: Option<String>,
) -> CommandResult<()> {
    // A window that is open gets the event; a new one asks on load.
    let open = app.get_webview_window(SETTINGS_WINDOW).is_some();
    *lock(&state.settings_pane) = if open { None } else { pane.clone() };
    if let (true, Some(pane)) = (open, &pane) {
        let _ = app.emit_to(SETTINGS_WINDOW, SETTINGS_PANE_EVENT, pane);
    }
    show_settings_window(&app).map_err(CommandError::other)
}

/// The pane `open_settings` asked for, once.
#[tauri::command]
pub async fn take_settings_pane(state: State<'_, AppState>) -> CommandResult<Option<String>> {
    Ok(lock(&state.settings_pane).take())
}

/// Opens or focuses the Settings window, as defined in tauri.conf.json.
pub fn show_settings_window(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW) {
        window.unminimize()?;
        return window.set_focus();
    }
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == SETTINGS_WINDOW)
        .expect("tauri.conf.json defines the Settings window");
    WebviewWindowBuilder::from_config(app, config)?.build()?;
    Ok(())
}

/// Starts every account's background sync.
pub fn start_background_sync(app: AppHandle) {
    for rt in app.state::<AppState>().runtimes() {
        start_sync_loop(app.clone(), rt);
    }
    start_wake_watch(app);
}

/// How often the clock is checked for a sleep.
const WAKE_CHECK: Duration = Duration::from_secs(30);
/// More time than this past a check's wait means the computer slept. Large, so a stalled
/// process, such as under App Nap, or a clock set forward isn't taken for a sleep; a
/// sleep shorter than this needs no sync anyway.
const WAKE_SLACK: Duration = Duration::from_secs(5 * 60);
/// The network takes a moment to come back after a wake.
const WAKE_DELAY: Duration = Duration::from_secs(15);
/// Retries of a sync on wake that failed, likely before the network was back.
const WAKE_RETRIES: u32 = 2;

/// Whether the wall clock moved on so much more than a wait of `waited` that the
/// computer must have slept. A timer doesn't run while it sleeps; the clock does.
fn slept(before: SystemTime, now: SystemTime, waited: Duration) -> bool {
    now.duration_since(before)
        .is_ok_and(|passed| passed > waited + WAKE_SLACK)
}

/// Tells every account's background sync when the computer wakes, as no system event
/// says so on every platform: those with sync on wake sync, and the others catch up
/// on a schedule the sleep held up.
fn start_wake_watch(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut before = SystemTime::now();
        loop {
            tokio::time::sleep(WAKE_CHECK).await;
            let now = SystemTime::now();
            if slept(before, now, WAKE_CHECK) {
                for rt in app.state::<AppState>().runtimes() {
                    rt.woke.store(true, Ordering::Relaxed);
                    rt.wake.notify_one();
                }
            }
            before = now;
        }
    });
}

/// When the account's schedule says: on start, and then every so many minutes after
/// the last automatic sync, by the wall clock so a sleep doesn't push it back. Also on
/// waking from sleep, when the account asks for it. A changed schedule applies at
/// once. Ends when the account is removed.
fn start_sync_loop(app: AppHandle, rt: Arc<Runtime>) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        // The last automatic sync, and the last that worked.
        let (mut last, mut last_ok): (Option<SystemTime>, Option<SystemTime>) = (None, None);
        // A sync after waking still due: the retries it has left, and whether the account's
        // sync on wake asked for it, rather than a schedule the sleep held up.
        let mut wake_sync: Option<(u32, bool)> = None;
        loop {
            if rt.removed.load(Ordering::Relaxed) {
                break;
            }
            let Some(account) = state.config().account(&rt.id).cloned() else {
                break;
            };
            let ago = |t: SystemTime| SystemTime::now().duration_since(t).unwrap_or_default();
            if rt.woke.swap(false, Ordering::Relaxed) {
                let overdue = match (account.sync, last) {
                    (SyncSchedule::Every(m), Some(l)) => {
                        ago(l) >= Duration::from_secs(u64::from(m) * 60)
                    }
                    _ => false,
                };
                let synced_lately = last_ok.is_some_and(|l| ago(l) < WAKE_SLACK);
                // The schedule's own sync may have run on waking, before the network.
                let failed_lately = last.is_some_and(|l| ago(l) < WAKE_SLACK) && last != last_ok;
                if account.sync_on_wake && !synced_lately {
                    wake_sync = Some((WAKE_RETRIES, true));
                } else if overdue || failed_lately {
                    wake_sync = Some((WAKE_RETRIES, false));
                }
            }
            // Turned off meanwhile.
            if !account.sync_on_wake {
                wake_sync = wake_sync.filter(|&(_, asked)| !asked);
            }
            let due = match (account.sync, last) {
                _ if wake_sync.is_some() => Some(Instant::now() + WAKE_DELAY),
                (SyncSchedule::Manual, _) | (SyncSchedule::OnStart, Some(_)) => None,
                (_, None) => Some(Instant::now()),
                (SyncSchedule::Every(m), Some(last)) => {
                    let since = SystemTime::now().duration_since(last).unwrap_or_default();
                    let every = Duration::from_secs(u64::from(m) * 60);
                    Some(Instant::now() + every.saturating_sub(since))
                }
            };
            let wait = async {
                match due {
                    Some(due) => sleep_until(due).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = wait => {
                    let ran = run_sync(&app, &rt).await;
                    // Another sync running: a sync after waking waits for its turn.
                    if ran == Ran::Busy && wake_sync.is_some() {
                        continue;
                    }
                    let now = SystemTime::now();
                    last = Some(now);
                    wake_sync = match ran {
                        Ran::Synced => {
                            last_ok = Some(now);
                            None
                        }
                        // Likely the network, not back yet after waking: again in a moment.
                        Ran::Failed { retry: true } => wake_sync
                            .and_then(|(n, asked)| Some((n.checked_sub(1)?, asked))),
                        Ran::Failed { retry: false } | Ran::Busy | Ran::Skipped => None,
                    };
                }
                () = rt.wake.notified() => {}
            }
        }
    });
}

/// How a sync went, for the background sync's retries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ran {
    Synced,
    /// `retry`: worth trying again soon, as anything but a rejected sign-in may be the
    /// network.
    Failed {
        retry: bool,
    },
    /// Another sync of the account was running.
    Busy,
    /// Signed out, removed, or stopped.
    Skipped,
}

/// One sync of the account, reported through `sync-status`. Does nothing while another
/// sync of it runs or when it is signed out.
async fn run_sync(app: &AppHandle, rt: &Arc<Runtime>) -> Ran {
    let state = app.state::<AppState>();
    let status = |patch: SyncStatus| SyncStatus {
        account_id: rt.id.clone(),
        last_sync: rt.db.last_sync().ok().flatten(),
        ..patch
    };
    let emit = |status: SyncStatus| {
        let _ = app.emit(SYNC_STATUS_EVENT, status);
    };

    let sync_once = async {
        let account = state.config().account(&rt.id).cloned()?;
        // `None` for the local account, which fetches its feeds itself.
        let client = match account.kind {
            AccountKind::Greader { .. } => match ensure_client(&state, rt).await {
                Ok(Some(client)) => Some(client),
                Ok(None) => return None,
                Err(e) => {
                    return Some(status(SyncStatus {
                        error: Some(e.message),
                        ..SyncStatus::default()
                    }));
                }
            },
            AccountKind::Local => None,
        };
        emit(status(SyncStatus {
            syncing: true,
            ..SyncStatus::default()
        }));

        let (now, keep_read_days) = (now_secs(), account.keep_read_days.max(1));
        let synced = match &client {
            Some(client) => sync(client, &rt.db, now, keep_read_days).await,
            None => {
                let keep_unread_days = Some(account.keep_unread_days).filter(|&d| d > 0);
                refresh_local(&rt.db, now, keep_read_days, keep_unread_days).await
            }
        };
        Some(match synced {
            Ok(report) => {
                if client.is_none() {
                    back_up_feeds(&state, rt);
                }
                status(SyncStatus {
                    changed: report.changed_articles(),
                    ..SyncStatus::default()
                })
            }
            Err(e) if e.is_corrupt() => {
                rt.corrupt.store(true, Ordering::Relaxed);
                status(SyncStatus {
                    error: Some(e.to_string()),
                    cache_broken: true,
                    ..SyncStatus::default()
                })
            }
            Err(e) => {
                let unauthorized = matches!(e, Error::Unauthorized);
                if unauthorized {
                    rt.token_rejected.store(true, Ordering::Relaxed);
                    *lock(&rt.client) = None;
                    // Sessions from before this say the account is signed in.
                    state.revision.fetch_add(1, Ordering::SeqCst);
                }
                status(SyncStatus {
                    error: Some(e.to_string()),
                    unauthorized,
                    ..SyncStatus::default()
                })
            }
        })
    };

    // Reported while the gate is held: a sign-in that stops syncs reports after it, so
    // its fresh status is the last a window gets.
    let work = async {
        let status = sync_once.await?;
        // Once per error, not on every try while offline.
        if let Some(error) = &status.error
            && lock(&rt.last_error).as_ref() != Some(error)
        {
            if status.cache_broken {
                tracing::event!(
                    name: "sync.run.failure",
                    Level::ERROR,
                    account.id = %rt.id,
                    error.message = %error,
                    "syncing account {{account.id}} failed, its cache is damaged: {{error.message}}",
                );
            } else {
                tracing::event!(
                    name: "sync.run.failure",
                    Level::WARN,
                    account.id = %rt.id,
                    error.message = %error,
                    "syncing account {{account.id}} failed: {{error.message}}",
                );
            }
        }
        *lock(&rt.last_error) = status.error.clone();
        let ran = match status.error {
            None => Ran::Synced,
            Some(_) => Ran::Failed {
                retry: !status.unauthorized,
            },
        };
        emit(status);
        update_badge(app);
        Some(ran)
    };

    match rt.sync_gate.run(work).await {
        Outcome::Done(Some(ran)) => {
            if ran == Ran::Synced {
                let (full_text, thumbnails) = (rt.clone(), rt.clone());
                tauri::async_runtime::spawn(async move { prefetch(&full_text).await });
                let app = app.clone();
                tauri::async_runtime::spawn(
                    async move { fetch_thumbnails(&app, &thumbnails).await },
                );
            }
            ran
        }
        Outcome::Busy => Ran::Busy,
        // What stops a sync reports the status after it: a sign-in its fresh one, and a
        // removed account needs none.
        Outcome::Stopped | Outcome::Done(None) => Ran::Skipped,
    }
}

/// Fetches Reader View pages for new articles, which can take minutes. Skipped while
/// the last sync's prefetch still runs; removing the account stops it.
async fn prefetch(rt: &Runtime) {
    if rt.is_removed() {
        return;
    }
    if let Outcome::Done(Err(e)) = rt.prefetch_gate.run(prefetch_full_text(&rt.db)).await {
        tracing::event!(
            name: "full_text.prefetch.failure",
            Level::WARN,
            account.id = %rt.id,
            error.message = %e,
            "prefetching Reader View for account {{account.id}} failed: {{error.message}}",
        );
    }
}

/// Fetches thumbnails for new articles, telling the windows as they arrive. Skipped
/// while the last sync's run still goes, and while thumbnails are off: no image host
/// hears from the app then. The articles wait, for a sync after they are turned on.
/// Removing the account stops it.
async fn fetch_thumbnails(app: &AppHandle, rt: &Runtime) {
    if rt.is_removed() {
        return;
    }
    let stored = || {
        let _ = app.emit(THUMBNAILS_EVENT, &rt.id);
    };
    // Read inside the gate: the setting is saved before turning them off stops a run, so
    // either that stops this run or this run sees them off.
    let work = async {
        let off = app.state::<AppState>().config().settings.thumbnail_size == ThumbnailSize::None;
        if off {
            return Ok(());
        }
        prefetch_thumbnails(&rt.db, stored).await
    };
    if let Outcome::Done(Err(e)) = rt.thumbnail_gate.run(work).await {
        tracing::event!(
            name: "thumbnail.prefetch.failure",
            Level::WARN,
            account.id = %rt.id,
            error.message = %e,
            "fetching thumbnails for account {{account.id}} failed: {{error.message}}",
        );
    }
}

/// Serves the `thumb` protocol: `thumb://localhost/<account ID>%2F<article ID>` (on
/// Windows, `http://thumb.localhost/...`) is the article's cached thumbnail JPEG, as
/// `convertFileSrc` makes the URL. The UI's only way to show a thumbnail: none is ever
/// loaded from its image host.
pub fn serve_thumbnail(
    app: &AppHandle,
    request: &tauri::http::Request<Vec<u8>>,
    responder: tauri::UriSchemeResponder,
) {
    // Both IDs are digits, letters and minus signs, so a slash is the only thing encoded.
    let path = request
        .uri()
        .path()
        .trim_start_matches('/')
        .replace("%2F", "/");
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let jpeg = path.split_once('/').and_then(|(account, article)| {
            let rt = app.try_state::<AppState>()?.runtime(account).ok()?;
            rt.db.thumbnail(article).ok().flatten()
        });
        let response = match jpeg {
            // An article's thumbnail never changes once stored.
            Some(jpeg) => tauri::http::Response::builder()
                .header(tauri::http::header::CONTENT_TYPE, "image/jpeg")
                .header(
                    tauri::http::header::CACHE_CONTROL,
                    "max-age=31536000, immutable",
                )
                .body(jpeg),
            None => tauri::http::Response::builder()
                .status(tauri::http::StatusCode::NOT_FOUND)
                .body(Vec::new()),
        };
        // The builder fails only on invalid headers, and these are constant.
        if let Ok(response) = response {
            responder.respond(response);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn greader(server: &str) -> AccountKind {
        AccountKind::Greader {
            server: server.into(),
            username: "carl".into(),
        }
    }

    #[test]
    fn an_account_is_named_after_its_service_or_host() {
        assert_eq!(
            default_description(&greader("https://rss.example.com/api/greader.php")),
            "FreshRSS"
        );
        assert_eq!(
            default_description(&greader("https://rss.example.com/i/api/greader.php/")),
            "FreshRSS"
        );
        assert_eq!(
            default_description(&greader("https://reader.example.org:8443/reader/api/0")),
            "reader.example.org"
        );
        assert_eq!(default_description(&AccountKind::Local), "Local");
    }

    #[test]
    fn a_clock_far_ahead_of_the_wait_means_the_computer_slept() {
        let before = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let after = |secs| before + Duration::from_secs(secs);
        assert!(!slept(before, after(30), WAKE_CHECK));
        assert!(!slept(before, after(300), WAKE_CHECK));
        assert!(slept(before, after(331), WAKE_CHECK));
        assert!(slept(before, after(8 * 3600), WAKE_CHECK));
        // A clock set back is no sleep.
        assert!(!slept(after(100), before, WAKE_CHECK));
    }

    #[test]
    fn a_local_cache_without_feeds_gets_them_from_the_backup() {
        let dir = std::env::temp_dir().join(format!("skvaller-restore-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let account = |id: &str, kind: AccountKind| Account {
            id: id.into(),
            description: "Local".into(),
            kind,
            sync: SyncSchedule::OnStart,
            keep_read_days: 30,
            keep_unread_days: 0,
            sync_on_wake: true,
        };
        let opml = r#"<?xml version="1.0"?><opml version="2.0"><head><title>Local</title></head>
            <body><outline text="Kernel Notes" type="rss" xmlUrl="https://kernel.example/feed.xml"/></body></opml>"#;
        // Caches from an older version, each next to a feeds' backup.
        for id in ["1", "2"] {
            std::fs::write(feeds_backup_path(&dir, id), opml).unwrap();
            rusqlite::Connection::open(db_path(&dir, id))
                .unwrap()
                .execute_batch("CREATE TABLE old (x); PRAGMA user_version = 7;")
                .unwrap();
        }
        // A cache that went, and one the user emptied, backed up as such.
        std::fs::write(feeds_backup_path(&dir, "3"), opml).unwrap();
        let empty = opml.replace(
            r#"<outline text="Kernel Notes" type="rss" xmlUrl="https://kernel.example/feed.xml"/>"#,
            "",
        );
        std::fs::write(feeds_backup_path(&dir, "4"), empty).unwrap();

        let local = open_runtime(&dir, &account("1", AccountKind::Local)).unwrap();
        assert_eq!(local.db.feeds().unwrap().len(), 1);
        drop(local);
        // Opened again: nothing is restored twice.
        let local = open_runtime(&dir, &account("1", AccountKind::Local)).unwrap();
        assert_eq!(local.db.feeds().unwrap().len(), 1);
        drop(local);
        let gone = open_runtime(&dir, &account("3", AccountKind::Local)).unwrap();
        assert_eq!(gone.db.feeds().unwrap().len(), 1);
        drop(gone);
        let emptied = open_runtime(&dir, &account("4", AccountKind::Local)).unwrap();
        assert!(emptied.db.feeds().unwrap().is_empty());
        drop(emptied);
        // A sync account gets its feeds from the server.
        let synced = open_runtime(&dir, &account("2", greader("https://rss.example"))).unwrap();
        assert!(synced.db.feeds().unwrap().is_empty());
        drop(synced);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_cache_moves_after_its_sqlite_files() {
        let files = db_files(Path::new("/data/accounts/1.db"));
        assert_eq!(
            files.map(|f| f.display().to_string()),
            [
                "/data/accounts/1.db-wal",
                "/data/accounts/1.db-shm",
                "/data/accounts/1.db"
            ]
        );
    }
}
