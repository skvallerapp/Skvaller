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
use tauri::{AppHandle, Emitter, Manager, State};

use crate::config::{Account, Config};

const SYNC_STATUS_EVENT: &str = "sync-status";

pub struct AppState {
    db: Arc<Db>,
    config_path: PathBuf,
    config: Mutex<Config>,
    /// Built from the keychain token on first use. `None` until then, and after the
    /// server rejects the token.
    client: Mutex<Option<Client>>,
    /// Held while a sync runs, so only one runs at a time.
    syncing: tokio::sync::Mutex<()>,
}

impl AppState {
    pub fn load(app: &AppHandle) -> Result<Self, Box<dyn std::error::Error>> {
        let data_dir = app.path().app_data_dir()?;
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
        })
    }

    fn config(&self) -> Config {
        lock(&self.config).clone()
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
    *lock(&state.client) = Some(client.clone());
    Ok(Some(client))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Session {
    /// The saved account, also when signed out, to fill in the sign-in form.
    account: Option<Account>,
    signed_in: bool,
    status: SyncStatus,
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
    Ok(Session {
        account: state.config().account,
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
        let mut config = state.config();
        if let Some(old) = config.account.as_ref().filter(|old| **old != account) {
            state.db.clear()?;
            let key = old.keychain_key();
            blocking(move || delete_token(&key)).await?;
        }
        config.account = Some(account.clone());
        config
            .save(&state.config_path)
            .map_err(CommandError::other)?;
        *lock(&state.config) = config;
        *lock(&state.client) = Some(client);
    }

    tauri::async_runtime::spawn(async move { run_sync(&app).await });
    Ok(Session {
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
pub async fn set_read(state: State<'_, AppState>, id: String, read: bool) -> CommandResult<()> {
    Ok(state.db.set_read(parse_id(&id)?, read)?)
}

#[tauri::command]
pub async fn set_starred(
    state: State<'_, AppState>,
    id: String,
    starred: bool,
) -> CommandResult<()> {
    Ok(state.db.set_starred(parse_id(&id)?, starred)?)
}

fn parse_id(id: &str) -> CommandResult<i64> {
    id.parse()
        .map_err(|_| CommandError::other(format!("bad article ID {id:?}")))
}

/// Syncs on start, then every `sync_interval_minutes`.
pub fn start_background_sync(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        loop {
            run_sync(&app).await;
            let minutes = app
                .state::<AppState>()
                .config()
                .sync_interval_minutes
                .max(1);
            tokio::time::sleep(Duration::from_secs(u64::from(minutes) * 60)).await;
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
    let keep_read_days = state.config().keep_read_days;
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
