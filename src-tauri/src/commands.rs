//! Tauri commands and the background sync. The UI's commands read and write only the
//! local cache. Network work happens in `run_sync`, which reports through the
//! `sync-status` event. One exception: `full_text` fetches a page the user opens in
//! Reader View, while the UI shows the cached feed content.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use skvaller_core::{
    ArticleSummary, Client, Counts, Db, Error, Feed, Folder, ImportReport, Selection, delete_token,
    export_opml as write_opml, import_opml as read_opml, load_full_text, load_token,
    prefetch_full_text, refresh_local, save_token, subscribe as subscribe_feed, sync,
};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow, WebviewWindowBuilder};
use tauri_plugin_dialog::{DialogExt, FilePath};
use tokio::sync::Notify;
use tokio::time::{Instant, sleep_until};

use crate::config::{
    Account, AccountKind, Config, FIRST_ACCOUNT_ID, Settings, SyncSchedule, Theme,
};
use crate::sync_gate::{Outcome, SyncGate};

const SYNC_STATUS_EVENT: &str = "sync-status";
/// Sent to every window with the new `Settings` when they change.
const SETTINGS_EVENT: &str = "settings-changed";
/// Sent to every window with a new `Session` when an account is added, changed or
/// removed, or another one becomes active.
const ACCOUNTS_EVENT: &str = "accounts-changed";
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
    /// Why an account's cache didn't open at start.
    broken: Mutex<HashMap<String, String>>,
    /// The pane the Settings window shows when it opens next.
    settings_pane: Mutex<Option<String>>,
    /// Counts config changes, so a window can tell an older session from a newer one
    /// that arrived first.
    revision: AtomicU64,
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
    sync_gate: SyncGate,
    /// Reader View prefetch, after a sync. Separate, so syncing is not held up by it.
    prefetch_gate: SyncGate,
    /// Wakes the background sync: the schedule changed, or the account was removed.
    wake: Notify,
    /// The account is gone. Work that still holds the runtime stops at its next check.
    removed: AtomicBool,
}

impl Runtime {
    fn open(dir: &Path, id: &str) -> skvaller_core::Result<Self> {
        Ok(Self {
            id: id.to_owned(),
            db: Arc::new(Db::open(db_path(dir, id))?),
            client: Mutex::new(None),
            token_rejected: AtomicBool::new(false),
            last_error: Mutex::new(None),
            sync_gate: SyncGate::default(),
            prefetch_gate: SyncGate::default(),
            wake: Notify::new(),
            removed: AtomicBool::new(false),
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

/// False if a file stays, such as one still open on Windows.
fn remove_db_files(dir: &Path, id: &str) -> bool {
    let mut removed = true;
    for file in db_files(&db_path(dir, id)) {
        match std::fs::remove_file(&file) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!("Removing {} failed: {e}", file.display());
                removed = false;
            }
        }
    }
    removed
}

impl AppState {
    pub fn load(app: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        // Local, not roaming, on Windows: a cache has no business following the user
        // between machines.
        let data_dir = app.path().app_local_data_dir()?;
        let config_dir = app.path().app_config_dir()?;
        let accounts_dir = data_dir.join("accounts");
        std::fs::create_dir_all(&accounts_dir)?;
        std::fs::create_dir_all(&config_dir)?;
        let config_path = config_dir.join("config.json");

        let mut config = match Config::load(&config_path) {
            Ok(config) => config,
            Err(defaults) => {
                // Kept for the user to recover, before a save overwrites it. The caches
                // stay as they are.
                let backup = config_path.with_extension("json.bak");
                eprintln!(
                    "config.json can't be read; keeping it as {}",
                    backup.display()
                );
                std::fs::copy(&config_path, &backup)?;
                defaults
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
                    std::fs::rename(from, to)?;
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
        let on_disk_ids = std::fs::read_dir(&accounts_dir)?.flatten().filter_map(|e| {
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
        if listed > config.accounts.len() {
            let backup = config_path.with_extension("json.bak");
            eprintln!(
                "Some accounts in config.json can't be read; keeping it as {}",
                backup.display()
            );
            std::fs::copy(&config_path, &backup)?;
        }
        if serde_json::to_value(&config).ok() != raw {
            config.save(&config_path)?;
        }

        // A cache that doesn't open, such as one from a newer version, takes only its
        // own account out.
        let (mut runtimes, mut broken) = (HashMap::new(), HashMap::new());
        for account in &config.accounts {
            match Runtime::open(&accounts_dir, &account.id) {
                Ok(runtime) => {
                    runtimes.insert(account.id.clone(), Arc::new(runtime));
                }
                Err(e) => {
                    eprintln!("Opening the cache of account {} failed: {e}", account.id);
                    broken.insert(account.id.clone(), e.to_string());
                }
            }
        }
        Ok(Self {
            accounts_dir,
            config: Mutex::new(config),
            config_path,
            runtimes: Mutex::new(runtimes),
            broken: Mutex::new(broken),
            settings_pane: Mutex::new(None),
            revision: AtomicU64::new(0),
        })
    }

    fn config(&self) -> Config {
        lock(&self.config).clone()
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
            Some(e) => format!("this account's cache can't be opened: {e}"),
            None => "the account was removed".to_owned(),
        }))
    }

    fn runtimes(&self) -> Vec<Arc<Runtime>> {
        lock(&self.runtimes).values().cloned().collect()
    }
}

/// The name an account gets until the user changes it: "FreshRSS" or the server's host
/// name, or where the local account's feeds are.
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
        AccountKind::Local if cfg!(target_os = "macos") => "On This Mac".to_owned(),
        AccountKind::Local => "On This Device".to_owned(),
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
        Some(Ok(rt)) => (is_signed_in(state, &rt).await?, rt.status()),
        // Its cache didn't open: the main window says why.
        Some(Err(e)) => (
            true,
            SyncStatus {
                account_id: config.active.clone().unwrap_or_default(),
                error: Some(e.message),
                ..SyncStatus::default()
            },
        ),
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
        }
        Err(e) => eprintln!("Reading the accounts failed: {}", e.message),
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
    let server = server.trim().trim_end_matches('/').to_owned();
    let username = username.trim().to_owned();
    let client = Client::login(&server, &username, &password).await?;
    let kind = AccountKind::Greader { server, username };
    let (key, token) = (
        kind.keychain_key()
            .expect("a sync account has a keychain entry"),
        client.token().to_owned(),
    );
    blocking(move || save_token(&key, &token)).await?;

    let rt = add_account(&app, &state, kind)?;
    {
        // A sync still running with the old token would reject the new one when it fails.
        let _no_sync = rt.sync_gate.stop().await;
        *lock(&rt.client) = Some(client);
        rt.token_rejected.store(false, Ordering::Relaxed);
        *lock(&rt.last_error) = None;
    }

    emit_session(&app).await;
    let sync_app = app.clone();
    tauri::async_runtime::spawn(async move { run_sync(&sync_app, &rt).await });
    current_session(&state).await
}

/// Makes the account of `kind` active, adding it first if there is none: then it gets
/// a new ID and its background sync starts.
fn add_account(
    app: &AppHandle,
    state: &AppState,
    kind: AccountKind,
) -> CommandResult<Arc<Runtime>> {
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
        Err(existing) => return state.runtime(&existing),
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
        });
        config.active = Some(id.clone());
        Ok(())
    });
    match added {
        Ok(Ok(())) => {
            lock(&state.runtimes).insert(id, rt.clone());
            start_sync_loop(app.clone(), rt.clone());
            Ok(rt)
        }
        Ok(Err(other)) => {
            drop(rt);
            remove_db_files(&state.accounts_dir, &id);
            state.runtime(&other)
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
    state.update_config(|config| config.active = Some(id))?;
    let session = current_session(&state).await?;
    let _ = app.emit(ACCOUNTS_EVENT, session.clone());
    Ok(session)
}

/// Changes an account's name or sync settings. Fields left out stay.
#[tauri::command]
pub async fn update_account(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
    description: Option<String>,
    sync: Option<SyncSchedule>,
    keep_read_days: Option<u32>,
) -> CommandResult<()> {
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
            Some(rt) => Some((rt.sync_gate.stop().await, rt.prefetch_gate.stop().await)),
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
        eprintln!(
            "Removing an account: removing the token failed: {}",
            e.message
        );
    }
    if !remove_db_files(&state.accounts_dir, &id) {
        let _ = state.update_config(|config| config.removed.push(id.clone()));
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
    Ok(state
        .runtime(&account_id)?
        .db
        .article_html(parse_id(&id)?)?)
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
    load_full_text(&rt.db, parse_id(&id)?, retry)
        .await?
        .ok_or_else(|| CommandError::other("the page has nothing to show in Reader View"))
}

/// Opens the feed's articles in Reader View, or stops doing so.
#[tauri::command]
pub async fn set_feed_reader_view(
    state: State<'_, AppState>,
    account_id: String,
    feed_id: String,
    on: bool,
) -> CommandResult<()> {
    Ok(state
        .runtime(&account_id)?
        .db
        .set_feed_reader_view(&feed_id, on)?)
}

#[tauri::command]
pub async fn set_read(
    state: State<'_, AppState>,
    account_id: String,
    ids: Vec<String>,
    read: bool,
) -> CommandResult<()> {
    Ok(state
        .runtime(&account_id)?
        .db
        .set_read(&parse_ids(&ids)?, read)?)
}

#[tauri::command]
pub async fn set_starred(
    state: State<'_, AppState>,
    account_id: String,
    ids: Vec<String>,
    starred: bool,
) -> CommandResult<()> {
    Ok(state
        .runtime(&account_id)?
        .db
        .set_starred(&parse_ids(&ids)?, starred)?)
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
    state: State<'_, AppState>,
    account_id: String,
    url: String,
    folder_id: Option<String>,
) -> CommandResult<String> {
    let rt = local(&state, &account_id)?;
    Ok(subscribe_feed(&rt.db, &url, folder_id.as_deref(), now_secs()).await?)
}

/// Removes a feed of the local account, with its articles.
#[tauri::command]
pub async fn unsubscribe(
    state: State<'_, AppState>,
    account_id: String,
    feed_id: String,
) -> CommandResult<()> {
    Ok(local(&state, &account_id)?.db.unsubscribe(&feed_id)?)
}

#[tauri::command]
pub async fn rename_feed(
    state: State<'_, AppState>,
    account_id: String,
    feed_id: String,
    title: String,
) -> CommandResult<()> {
    let title = non_empty(&title)?;
    Ok(local(&state, &account_id)?
        .db
        .rename_feed(&feed_id, title)?)
}

/// Moves a feed of the local account into a folder, or out of any with `None`.
#[tauri::command]
pub async fn move_feed(
    state: State<'_, AppState>,
    account_id: String,
    feed_id: String,
    folder_id: Option<String>,
) -> CommandResult<()> {
    Ok(local(&state, &account_id)?
        .db
        .move_feed(&feed_id, folder_id.as_deref())?)
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
    Ok(local(&state, &account_id)?
        .db
        .rename_folder(&folder_id, title)?)
}

/// Removes a folder of the local account. Its feeds stay, in no folder.
#[tauri::command]
pub async fn delete_folder(
    state: State<'_, AppState>,
    account_id: String,
    folder_id: String,
) -> CommandResult<()> {
    Ok(local(&state, &account_id)?.db.delete_folder(&folder_id)?)
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
    let mut theme_changed = false;
    state.update_config(|config| {
        theme_changed = config.settings.theme != settings.theme;
        config.settings = settings.clone();
    })?;
    if theme_changed {
        app.set_theme(settings.theme.native());
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

fn parse_ids(ids: &[String]) -> CommandResult<Vec<i64>> {
    ids.iter().map(|id| parse_id(id)).collect()
}

fn parse_id(id: &str) -> CommandResult<i64> {
    id.parse()
        .map_err(|_| CommandError::other(format!("bad article ID {id:?}")))
}

/// Starts every account's background sync.
pub fn start_background_sync(app: AppHandle) {
    for rt in app.state::<AppState>().runtimes() {
        start_sync_loop(app.clone(), rt);
    }
}

/// When the account's schedule says: on start, and then every so many minutes after
/// the last automatic sync. A changed schedule applies at once. Ends when the account
/// is removed.
fn start_sync_loop(app: AppHandle, rt: Arc<Runtime>) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let mut last: Option<Instant> = None;
        loop {
            if rt.removed.load(Ordering::Relaxed) {
                break;
            }
            let Some(schedule) = state.config().account(&rt.id).map(|a| a.sync) else {
                break;
            };
            let due = match (schedule, last) {
                (SyncSchedule::Manual, _) | (SyncSchedule::OnStart, Some(_)) => None,
                (_, None) => Some(Instant::now()),
                (SyncSchedule::Every(m), Some(last)) => {
                    Some(last + Duration::from_secs(u64::from(m) * 60))
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
                    run_sync(&app, &rt).await;
                    last = Some(Instant::now());
                }
                () = rt.wake.notified() => {}
            }
        }
    });
}

/// One sync of the account, reported through `sync-status`. Does nothing while another
/// sync of it runs or when it is signed out.
async fn run_sync(app: &AppHandle, rt: &Arc<Runtime>) {
    let state = app.state::<AppState>();
    let status = |patch: SyncStatus| SyncStatus {
        account_id: rt.id.clone(),
        last_sync: rt.db.last_sync().ok().flatten(),
        ..patch
    };
    let emit = |status: SyncStatus| {
        let _ = app.emit(SYNC_STATUS_EVENT, status);
    };

    let work = async {
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
            None => refresh_local(&rt.db, now, keep_read_days).await,
        };
        Some(match synced {
            Ok(_) => status(SyncStatus::default()),
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

    match rt.sync_gate.run(work).await {
        Outcome::Done(Some(status)) => {
            let synced = status.error.is_none();
            *lock(&rt.last_error) = status.error.clone();
            emit(status);
            if synced {
                let rt = rt.clone();
                tauri::async_runtime::spawn(async move { prefetch(&rt).await });
            }
        }
        Outcome::Stopped => emit(status(SyncStatus::default())),
        Outcome::Done(None) | Outcome::Busy => {}
    }
}

/// Fetches Reader View pages for new articles, which can take minutes. Skipped while
/// the last sync's prefetch still runs; removing the account stops it.
async fn prefetch(rt: &Runtime) {
    if rt.is_removed() {
        return;
    }
    if let Outcome::Done(Err(e)) = rt.prefetch_gate.run(prefetch_full_text(&rt.db)).await {
        eprintln!("Prefetching Reader View failed: {e}");
    }
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
        assert!(default_description(&AccountKind::Local).starts_with("On This "));
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
