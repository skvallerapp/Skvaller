//! Tauri commands and the background sync. The UI's commands read and write only the
//! local cache. Network work happens in `run_sync`, which reports through the
//! `sync-status` event.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use skvaller_core::{
    ArticleSummary, Client, Counts, Db, Error, Feed, Folder, Selection, delete_token, load_token,
    save_token, sync,
};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindowBuilder};
use tokio::sync::Notify;
use tokio::time::{Instant, sleep_until};

use crate::config::{Account, Config, Settings};

const SYNC_STATUS_EVENT: &str = "sync-status";
/// Sent to every window with the new `Settings` when they change.
const SETTINGS_EVENT: &str = "settings-changed";
/// Sent to every window with the `Account` after signing in.
const SIGNED_IN_EVENT: &str = "signed-in";
/// Sent to every window after signing out.
const SIGNED_OUT_EVENT: &str = "signed-out";
/// The label of the Settings window, in tauri.conf.json.
const SETTINGS_WINDOW: &str = "settings";

pub struct AppState {
    db: Arc<Db>,
    config_path: PathBuf,
    config: Mutex<Config>,
    /// Built from the keychain token on first use. `None` until then, and after the
    /// server rejects the token.
    client: Mutex<Option<Client>>,
    /// Held while a sync runs, so only one runs at a time.
    syncing: tokio::sync::Mutex<()>,
    /// Wakes the background sync when the sync interval changes.
    interval_changed: Notify,
}

impl AppState {
    pub fn load(app: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        // Local, not roaming, on Windows: a cache has no business following the user
        // between machines.
        let data_dir = app.path().app_local_data_dir()?;
        let config_dir = app.path().app_config_dir()?;
        std::fs::create_dir_all(&data_dir)?;
        std::fs::create_dir_all(&config_dir)?;
        let config_path = config_dir.join("config.json");
        Ok(Self {
            db: Arc::new(Db::open(data_dir.join("cache.db"))?),
            config: Mutex::new(Config::load(&config_path)),
            config_path,
            client: Mutex::new(None),
            syncing: tokio::sync::Mutex::new(()),
            interval_changed: Notify::new(),
        })
    }

    fn config(&self) -> Config {
        lock(&self.config).clone()
    }

    /// Changes the config and saves it, under the lock, so concurrent changes to
    /// different fields don't undo each other.
    fn update_config(&self, change: impl FnOnce(&mut Config)) -> CommandResult<()> {
        let mut config = lock(&self.config);
        let mut new = config.clone();
        change(&mut new);
        new.save(&self.config_path).map_err(CommandError::other)?;
        *config = new;
        Ok(())
    }

    fn client(&self) -> Option<Client> {
        lock(&self.client).clone()
    }

    fn is_syncing(&self) -> bool {
        self.syncing.try_lock().is_err()
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

/// The API client for the saved account, if it has a token in the keychain.
async fn ensure_client(state: &AppState) -> CommandResult<Option<Client>> {
    if let Some(client) = state.client() {
        return Ok(Some(client));
    }
    let Some(account) = state.config().account else {
        return Ok(None);
    };
    let key = account.keychain_key();
    let Some(token) = blocking(move || load_token(&key)).await? else {
        return Ok(None);
    };
    let client = Client::new(&account.server, &token)?;
    // Signing in or out may have happened while the token loaded. Keep this client only
    // if nothing replaced it and the account is still the same.
    let mut current = lock(&state.client);
    if let Some(current) = current.as_ref() {
        return Ok(Some(current.clone()));
    }
    if state.config().account.as_ref() != Some(&account) {
        return Ok(None);
    }
    *current = Some(client.clone());
    Ok(Some(client))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// The saved account, also when signed out, to fill in the sign-in form.
    account: Option<Account>,
    signed_in: bool,
    status: SyncStatus,
    settings: Settings,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SyncStatus {
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
    let signed_in = ensure_client(&state).await?.is_some();
    let config = state.config();
    Ok(Session {
        settings: config.settings,
        account: config.account,
        signed_in,
        status: SyncStatus {
            syncing: state.is_syncing(),
            last_sync: state.db.last_sync()?,
            ..SyncStatus::default()
        },
    })
}

/// Signs in, saves the token in the keychain and starts a sync. Signing in to a different
/// account empties the cache first.
#[tauri::command]
pub async fn sign_in(
    app: AppHandle,
    state: State<'_, AppState>,
    server: String,
    username: String,
    password: String,
) -> CommandResult<Session> {
    let server = server.trim().trim_end_matches('/').to_owned();
    let client = Client::login(&server, username.trim(), &password).await?;
    let account = Account {
        server,
        username: username.trim().to_owned(),
    };
    let (key, token) = (account.keychain_key(), client.token().to_owned());
    blocking(move || save_token(&key, &token)).await?;

    {
        // No sync of the old account may write while the cache is emptied.
        let _no_sync = state.syncing.lock().await;
        // The cache belongs to the saved account. With none saved, it may hold what a
        // failed sign-out left behind.
        let old = state.config().account;
        if old.as_ref() != Some(&account) {
            state.db.clear()?;
        }
        if let Some(old) = old.filter(|old| *old != account) {
            let key = old.keychain_key();
            if let Err(e) = blocking(move || delete_token(&key)).await {
                eprintln!(
                    "Signing in: removing the old account's token failed: {}",
                    e.message
                );
            }
        }
        state.update_config(|config| config.account = Some(account.clone()))?;
        *lock(&state.client) = Some(client);
    }

    let _ = app.emit(SIGNED_IN_EVENT, &account);
    tauri::async_runtime::spawn(async move { run_sync(&app).await });
    Ok(Session {
        settings: state.config().settings,
        account: Some(account),
        signed_in: true,
        status: SyncStatus {
            syncing: true,
            last_sync: state.db.last_sync()?,
            ..SyncStatus::default()
        },
    })
}

/// Syncs now, unless a sync is already running. The outcome arrives as a `sync-status`
/// event.
#[tauri::command]
pub async fn sync_now(app: AppHandle) -> CommandResult<()> {
    run_sync(&app).await;
    Ok(())
}

#[derive(Serialize)]
pub struct Library {
    folders: Vec<Folder>,
    feeds: Vec<Feed>,
    counts: Counts,
}

/// Folders, feeds and unread and starred counts, for the sidebar.
#[tauri::command]
pub async fn library(state: State<'_, AppState>) -> CommandResult<Library> {
    Ok(Library {
        folders: state.db.folders()?,
        feeds: state.db.feeds()?,
        counts: state.db.counts()?,
    })
}

#[tauri::command]
pub async fn articles(
    state: State<'_, AppState>,
    selection: Selection,
) -> CommandResult<Vec<ArticleSummary>> {
    Ok(state.db.articles(&selection)?)
}

#[tauri::command]
pub async fn article_html(state: State<'_, AppState>, id: String) -> CommandResult<Option<String>> {
    Ok(state.db.article_html(parse_id(&id)?)?)
}

#[tauri::command]
pub async fn set_read(
    state: State<'_, AppState>,
    ids: Vec<String>,
    read: bool,
) -> CommandResult<()> {
    Ok(state.db.set_read(&parse_ids(&ids)?, read)?)
}

#[tauri::command]
pub async fn set_starred(
    state: State<'_, AppState>,
    ids: Vec<String>,
    starred: bool,
) -> CommandResult<()> {
    Ok(state.db.set_starred(&parse_ids(&ids)?, starred)?)
}

/// Saves the settings and sends them to every window.
#[tauri::command]
pub async fn set_settings(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> CommandResult<()> {
    let mut interval_changed = false;
    state.update_config(|config| {
        interval_changed = config.settings.sync_interval_minutes != settings.sync_interval_minutes;
        config.settings = settings.clone();
    })?;
    if interval_changed {
        state.interval_changed.notify_one();
    }
    let _ = app.emit(SETTINGS_EVENT, settings);
    Ok(())
}

/// Signs out: removes the account from the config, its token from the keychain and every
/// article from the cache, queued changes included. Closes the Settings window, where
/// signing out happens.
///
/// Once the account is gone from the config, the user is signed out. Removing the token
/// and the articles after that is best effort: a failure is logged, and the next sign-in
/// empties the cache anyway.
#[tauri::command]
pub async fn sign_out(app: AppHandle, state: State<'_, AppState>) -> CommandResult<()> {
    // No sync may write while the cache is emptied.
    let _no_sync = state.syncing.lock().await;
    let account = state.config().account;
    state.update_config(|config| config.account = None)?;
    *lock(&state.client) = None;

    if let Some(account) = account {
        let key = account.keychain_key();
        if let Err(e) = blocking(move || delete_token(&key)).await {
            eprintln!("Signing out: removing the token failed: {}", e.message);
        }
    }
    if let Err(e) = state.db.clear() {
        eprintln!("Signing out: emptying the cache failed: {e}");
    }

    let _ = app.emit(SIGNED_OUT_EVENT, ());
    if let Some(window) = app.get_webview_window(SETTINGS_WINDOW) {
        let _ = window.close();
    }
    Ok(())
}

/// Opens the Settings window, or brings it to the front when it is open. Async because
/// creating a window from a synchronous command deadlocks on Windows.
#[tauri::command]
pub async fn open_settings(app: AppHandle) -> CommandResult<()> {
    show_settings_window(&app).map_err(CommandError::other)
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

/// Syncs on start, then every `sync_interval_minutes` after the last automatic sync.
/// With the interval set to manually (0), it never syncs on its own, not on start either.
/// A changed interval applies at once.
pub fn start_background_sync(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let mut last: Option<Instant> = None;
        loop {
            let minutes = state.config().settings.sync_interval_minutes;
            let due = match (minutes, last) {
                (0, _) => None,
                (_, None) => Some(Instant::now()),
                (m, Some(last)) => Some(last + Duration::from_secs(u64::from(m) * 60)),
            };
            let wait = async {
                match due {
                    Some(due) => sleep_until(due).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = wait => {
                    run_sync(&app).await;
                    last = Some(Instant::now());
                }
                () = state.interval_changed.notified() => {}
            }
        }
    });
}

/// One sync, reported through `sync-status`. Does nothing while another sync runs or
/// when no one is signed in.
async fn run_sync(app: &AppHandle) {
    let state = app.state::<AppState>();
    let Ok(_running) = state.syncing.try_lock() else {
        return;
    };
    let last_sync = || state.db.last_sync().ok().flatten();
    let emit = |status: SyncStatus| {
        let _ = app.emit(SYNC_STATUS_EVENT, status);
    };

    let client = match ensure_client(&state).await {
        Ok(Some(client)) => client,
        Ok(None) => return,
        Err(e) => {
            emit(SyncStatus {
                last_sync: last_sync(),
                error: Some(e.message),
                ..SyncStatus::default()
            });
            return;
        }
    };
    emit(SyncStatus {
        syncing: true,
        last_sync: last_sync(),
        ..SyncStatus::default()
    });

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let keep_read_days = state.config().settings.keep_read_days.max(1);
    let status = match sync(&client, &state.db, now, keep_read_days).await {
        Ok(_) => SyncStatus {
            last_sync: last_sync(),
            ..SyncStatus::default()
        },
        Err(e) => {
            let unauthorized = matches!(e, Error::Unauthorized);
            if unauthorized {
                *lock(&state.client) = None;
            }
            SyncStatus {
                last_sync: last_sync(),
                error: Some(e.to_string()),
                unauthorized,
                ..SyncStatus::default()
            }
        }
    };
    emit(status);
}
