//! The accounts and their caches, as the app runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use skvaller_core::{
    ArticleSummary, Client, Counts, Db, Feed, Folder, ImportReport, Selection, export_opml,
    import_opml,
};
use tokio::sync::broadcast;
use tracing::Level;

use crate::config::{
    Account, AccountKind, Config, DEFAULT_KEEP_UNREAD_DAYS, FIRST_ACCOUNT_ID, SyncSchedule,
};
use crate::error::{Error, Result, io};
use crate::gate::Gate;
use crate::sync::SyncStatus;

/// Where the app keeps its settings and caches.
///
/// [`Dirs::system`] gives the folders the Tauri version of Skvaller used, so its
/// accounts carry over; tests pass temporary folders instead.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Dirs {
    /// Holds `accounts/<id>.db`, one cache per account.
    pub data: PathBuf,
    /// Holds `config.json`.
    pub config: PathBuf,
}

impl Dirs {
    /// Makes a [`Dirs`] from the two folders.
    #[must_use]
    pub fn new(data: PathBuf, config: PathBuf) -> Self {
        Self { data, config }
    }

    /// The system's folders for Skvaller, named after its bundle identifier.
    ///
    /// The cache is in the local data folder, not the roaming one on Windows: it has no
    /// business following the user between machines. These are the folders Tauri's
    /// `app_local_data_dir` and `app_config_dir` gave the Tauri version.
    ///
    /// # Errors
    ///
    /// [`Error::NoFolder`] when the system has no such folder, as without a home folder.
    pub fn system() -> Result<Self> {
        let id = skvaller_core::KEYCHAIN_SERVICE;
        let data = dirs::data_local_dir().ok_or(Error::NoFolder("data"))?;
        let config = dirs::config_dir().ok_or(Error::NoFolder("settings"))?;
        Ok(Self::new(data.join(id), config.join(id)))
    }

    fn accounts(&self) -> PathBuf {
        self.data.join("accounts")
    }

    fn config_file(&self) -> PathBuf {
        self.config.join("config.json")
    }
}

/// What the user is told about a `config.json` that couldn't be fully read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    /// The dialog's title.
    pub title: &'static str,
    /// The dialog's text.
    pub message: String,
}

/// Folders, feeds and unread and starred counts, for the sidebar.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Library {
    /// The account's folders.
    pub folders: Vec<Folder>,
    /// The account's feeds, in and out of folders.
    pub feeds: Vec<Feed>,
    /// Unread and starred articles per feed.
    pub counts: Counts,
}

/// What one account needs while the app runs: its cache, its client and its syncs.
#[derive(Debug)]
pub(crate) struct Runtime {
    pub(crate) id: String,
    pub(crate) db: Arc<Db>,
    /// Built from the keychain token on first use. `None` until then, after the server
    /// rejects the token, and always for a local account.
    pub(crate) client: Mutex<Option<Client>>,
    /// The server rejected the saved token, so none is read from the keychain again
    /// until the user signs in.
    pub(crate) token_rejected: AtomicBool,
    /// Why the last sync failed.
    pub(crate) last_error: Mutex<Option<String>>,
    /// A sync found the cache damaged.
    pub(crate) corrupt: AtomicBool,
    pub(crate) gate: Gate,
}

impl Runtime {
    fn open(dir: &Path, account: &Account) -> skvaller_core::Result<Self> {
        let rt = Self {
            id: account.id.clone(),
            db: Arc::new(Db::open(db_path(dir, &account.id))?),
            client: Mutex::new(None),
            token_rejected: AtomicBool::new(false),
            last_error: Mutex::new(None),
            corrupt: AtomicBool::new(false),
            gate: Gate::default(),
        };
        // A local cache without feeds, as after a new version's cache started over,
        // gets them back from their backup before a sync backs up the empty list.
        if account.kind == AccountKind::Local && rt.db.feeds().is_ok_and(|f| f.is_empty()) {
            restore_feeds(dir, &account.id, &rt.db);
        }
        Ok(rt)
    }

    pub(crate) fn status(&self) -> SyncStatus {
        SyncStatus {
            account_id: self.id.clone(),
            syncing: self.gate.is_running(),
            last_sync: self.db.last_sync().ok().flatten(),
            error: lock(&self.last_error).clone(),
            unauthorized: self.token_rejected.load(Ordering::Relaxed),
            changed: false,
            cache_broken: self.corrupt.load(Ordering::Relaxed),
        }
    }
}

/// The accounts, their caches and their background syncs.
///
/// Reads come from each account's local cache only; nothing here waits for the network
/// except the syncs, which run in the background (see [`App::start_sync`]) and report
/// through [`App::subscribe`].
#[derive(Debug)]
pub struct App {
    pub(crate) accounts_dir: PathBuf,
    pub(crate) config: Mutex<Config>,
    pub(crate) runtimes: HashMap<String, Arc<Runtime>>,
    /// Accounts whose cache didn't open, and why.
    broken: HashMap<String, String>,
    notice: Option<Notice>,
    pub(crate) events: broadcast::Sender<SyncStatus>,
}

/// How many sync reports wait for a slow listener before it misses the oldest. A
/// listener that falls behind only misses statuses, and reloads on the next one.
const EVENT_BACKLOG: usize = 64;

impl App {
    /// Reads `config.json` and opens each account's cache.
    ///
    /// A `config.json` that can't be read is copied to `config.json.bak` and the app
    /// starts with defaults, with a [`Notice`] for the user. An account entry this
    /// version can't read, such as a newer version's, is left out the same way. A cache
    /// that doesn't open takes only its own account out.
    ///
    /// # Errors
    ///
    /// [`Error::Backup`] when `config.json` can't be read and can't be copied either:
    /// starting would later overwrite the only copy of the user's accounts.
    /// [`Error::Io`] when a folder can't be made or read, or the config can't be saved.
    #[expect(
        clippy::too_many_lines,
        reason = "one startup sequence, in the order it must run"
    )]
    pub fn load(dirs: &Dirs) -> Result<Self> {
        let accounts_dir = dirs.accounts();
        let config_path = dirs.config_file();
        std::fs::create_dir_all(&accounts_dir).map_err(io("Creating", &accounts_dir))?;
        std::fs::create_dir_all(&dirs.config).map_err(io("Creating", &dirs.config))?;

        let mut notice = None;
        let mut config = match Config::load(&config_path) {
            Ok(config) => config,
            Err(reason) => {
                let backup = back_up(&config_path)?;
                tracing::event!(
                    name: "config.read.failure",
                    Level::WARN,
                    file.path = %config_path.display(),
                    backup.path = %backup.display(),
                    error.message = %reason,
                    "reading {{file.path}} failed, so it was kept as {{backup.path}} and \
                     the settings reset: {{error.message}}",
                );
                notice = Some(Notice {
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
        move_old_cache(&dirs.data, &accounts_dir, &config)?;
        let removed = std::mem::take(&mut config.removed);
        config.removed = removed
            .into_iter()
            .filter(|id| config.account(id).is_none() && !remove_db_files(&accounts_dir, id))
            .collect();
        // No new account takes the ID of a cache on disk, even one a lost config knew.
        let on_disk = std::fs::read_dir(&accounts_dir)
            .map_err(io("Reading", &accounts_dir))?
            .flatten()
            .filter_map(|e| {
                e.file_name()
                    .to_str()?
                    .split('.')
                    .next()?
                    .parse::<u64>()
                    .ok()
            });
        config.next_id = config.next_id.max(on_disk.max().map_or(0, |n| n + 1));

        let raw: Option<serde_json::Value> = std::fs::read(&config_path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok());
        let listed = raw
            .as_ref()
            .and_then(|r| r.get("accounts")?.as_array().map(Vec::len))
            .unwrap_or(0);
        // Not after a reset: then every account was left out, and the file is kept.
        if notice.is_none() && listed > config.accounts.len() {
            let backup = back_up(&config_path)?;
            tracing::event!(
                name: "config.accounts.read.failure",
                Level::WARN,
                file.path = %config_path.display(),
                backup.path = %backup.display(),
                "some accounts in {{file.path}} can't be read, so they were left out and \
                 the file kept as {{backup.path}}",
            );
            notice = Some(Notice {
                title: "Accounts Left Out",
                message: format!(
                    "Skvaller couldn't read some accounts in its settings file, perhaps \
                     because a newer version saved them, so it left them out. The old file \
                     is kept at {}.",
                    backup.display()
                ),
            });
        }
        // Saved only when something changed, so a file that reads back the same stays.
        if serde_json::to_value(&config).ok() != raw {
            config
                .save(&config_path)
                .map_err(io("Saving", &config_path))?;
        }

        let (mut runtimes, mut broken) = (HashMap::new(), HashMap::new());
        for account in &config.accounts {
            match Runtime::open(&accounts_dir, account) {
                Ok(rt) => {
                    runtimes.insert(account.id.clone(), Arc::new(rt));
                }
                Err(e) => {
                    tracing::event!(
                        name: "cache.open.failure",
                        Level::ERROR,
                        account.id = %account.id,
                        error.message = %e,
                        "opening the cache of account {{account.id}} failed: {{error.message}}",
                    );
                    broken.insert(account.id.clone(), e.to_string());
                }
            }
        }
        Ok(Self {
            accounts_dir,
            config: Mutex::new(config),
            runtimes,
            broken,
            notice,
            events: broadcast::channel(EVENT_BACKLOG).0,
        })
    }

    /// What to tell the user about `config.json`, if it couldn't be fully read.
    #[must_use]
    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    /// The accounts, in the order the user keeps them.
    #[must_use]
    pub fn accounts(&self) -> Vec<Account> {
        lock(&self.config).accounts.clone()
    }

    /// The account the main window shows. `None` only without accounts.
    #[must_use]
    pub fn active(&self) -> Option<String> {
        lock(&self.config).active.clone()
    }

    /// The settings shared by all accounts.
    #[must_use]
    pub fn settings(&self) -> crate::config::Settings {
        lock(&self.config).settings.clone()
    }

    /// The account's folders, feeds and counts.
    ///
    /// # Errors
    ///
    /// [`Error::NoAccount`] or [`Error::Broken`] for an account without a cache, and
    /// [`Error::Core`] when the cache can't be read.
    pub fn library(&self, account_id: &str) -> Result<Library> {
        let db = &self.runtime(account_id)?.db;
        Ok(Library {
            folders: db.folders()?,
            feeds: db.feeds()?,
            counts: db.counts()?,
        })
    }

    /// The account's articles in `selection`, newest first, without their HTML.
    ///
    /// # Errors
    ///
    /// As [`App::library`].
    pub fn articles(&self, account_id: &str, selection: &Selection) -> Result<Vec<ArticleSummary>> {
        Ok(self.runtime(account_id)?.db.articles(selection)?)
    }

    /// The article's HTML, as the feed gave it: untrusted, never to run scripts.
    ///
    /// # Errors
    ///
    /// As [`App::library`].
    pub fn article_html(&self, account_id: &str, article_id: &str) -> Result<Option<String>> {
        Ok(self.runtime(account_id)?.db.article_html(article_id)?)
    }

    /// Marks articles read or unread in the cache. The next sync sends the change, and
    /// a sync never undoes it before then.
    ///
    /// # Errors
    ///
    /// As [`App::library`].
    pub fn set_read(&self, account_id: &str, ids: &[String], read: bool) -> Result<()> {
        Ok(self.runtime(account_id)?.db.set_read(ids, read)?)
    }

    /// Stars or unstars articles in the cache, sent on the next sync as
    /// [`App::set_read`]'s changes are.
    ///
    /// # Errors
    ///
    /// As [`App::library`].
    pub fn set_starred(&self, account_id: &str, ids: &[String], starred: bool) -> Result<()> {
        Ok(self.runtime(account_id)?.db.set_starred(ids, starred)?)
    }

    /// The account's sync status now.
    ///
    /// # Errors
    ///
    /// [`Error::NoAccount`] or [`Error::Broken`] for an account without a cache.
    pub fn status(&self, account_id: &str) -> Result<SyncStatus> {
        Ok(self.runtime(account_id)?.status())
    }

    /// Hears every account's sync status as it changes.
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<SyncStatus> {
        self.events.subscribe()
    }

    pub(crate) fn account(&self, id: &str) -> Option<Account> {
        lock(&self.config).account(id).cloned()
    }

    pub(crate) fn runtime(&self, id: &str) -> Result<Arc<Runtime>> {
        if let Some(rt) = self.runtimes.get(id) {
            return Ok(Arc::clone(rt));
        }
        Err(self
            .broken
            .get(id)
            .map_or(Error::NoAccount, |why| Error::Broken(why.clone())))
    }

    /// Writes the local account's feeds next to its cache, as OPML.
    ///
    /// They are nowhere else, and a damaged cache starts over from them. Best effort: a
    /// failure is logged.
    pub(crate) fn back_up_feeds(&self, rt: &Runtime) {
        let Some(account) = self.account(&rt.id) else {
            return;
        };
        if account.kind != AccountKind::Local {
            return;
        }
        let path = feeds_backup_path(&self.accounts_dir, &rt.id);
        let written = export_opml(&rt.db, &account.description)
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
}

/// Adds feeds from OPML to the local account, adding that account if there is none.
///
/// For testing on a machine without accounts. Run it before [`App::load`], with no app
/// running on the same folders.
///
/// # Errors
///
/// As [`App::load`], and [`Error::Core`] when the OPML can't be read or stored.
pub fn import_local(dirs: &Dirs, opml: &str) -> Result<ImportReport> {
    Ok(import_opml(&local_db(dirs)?, opml)?)
}

/// Adds `count` synthetic articles to the local account, adding that account if there is
/// none: a way to test the article list at 5,000+ items. Returns how many were stored.
///
/// # Errors
///
/// As [`App::load`], and [`Error::Core`] when the articles can't be stored.
pub fn seed_local(dirs: &Dirs, count: usize) -> Result<usize> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(0));
    Ok(skvaller_core::seed_articles(&local_db(dirs)?, count, now)?)
}

/// Opens the local account's cache, making the account active, or adding it first.
fn local_db(dirs: &Dirs) -> Result<Db> {
    let app = App::load(dirs)?;
    let config_path = dirs.config_file();
    let mut config = lock(&app.config).clone();
    let local = config
        .accounts
        .iter()
        .find(|a| a.kind == AccountKind::Local);
    let id = if let Some(account) = local {
        account.id.clone()
    } else {
        let id = config.take_id();
        config.accounts.push(Account {
            id: id.clone(),
            description: default_description(&AccountKind::Local),
            kind: AccountKind::Local,
            sync: SyncSchedule::default(),
            keep_read_days: skvaller_core::DEFAULT_KEEP_READ_DAYS,
            keep_unread_days: DEFAULT_KEEP_UNREAD_DAYS,
            sync_on_wake: false,
        });
        id
    };
    config.active = Some(id.clone());
    config
        .save(&config_path)
        .map_err(io("Saving", &config_path))?;
    Ok(Db::open(db_path(&app.accounts_dir, &id))?)
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Copies `config.json` to `config.json.bak`, for the user to recover from.
fn back_up(config_path: &Path) -> Result<PathBuf> {
    let backup = config_path.with_extension("json.bak");
    std::fs::copy(config_path, &backup).map_err(|source| Error::Backup {
        path: config_path.to_owned(),
        source,
    })?;
    Ok(backup)
}

/// Moves the one cache from before several accounts to the first account's place.
fn move_old_cache(data_dir: &Path, accounts_dir: &Path, config: &Config) -> Result<()> {
    let old = data_dir.join("cache.db");
    let first = db_path(accounts_dir, FIRST_ACCOUNT_ID);
    if config.account(FIRST_ACCOUNT_ID).is_none() || !old.exists() || first.exists() {
        return Ok(());
    }
    for (from, to) in db_files(&old).iter().zip(&db_files(&first)) {
        if from.exists() {
            std::fs::rename(from, to).map_err(io("Moving", from))?;
        }
    }
    Ok(())
}

/// The name an account gets until the user changes it: `FreshRSS`, the server's host
/// name, or `Local`.
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

fn db_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.db"))
}

/// SQLite's files next to the cache, then the cache: moved or removed in this order, a
/// cache is never left without its write-ahead log.
fn db_files(path: &Path) -> [PathBuf; 3] {
    let with = |suffix: &str| {
        let mut name = path.as_os_str().to_owned();
        name.push(suffix);
        PathBuf::from(name)
    };
    [with("-wal"), with("-shm"), path.to_owned()]
}

fn feeds_backup_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.opml"))
}

/// Removes a removed account's cache files. False if one stays, such as one still open
/// on Windows.
fn remove_db_files(dir: &Path, id: &str) -> bool {
    let cache = db_files(&db_path(dir, id));
    let corrupt = cache.iter().map(|f| {
        let mut name = f.as_os_str().to_owned();
        name.push(".corrupt");
        PathBuf::from(name)
    });
    let mut removed = true;
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

/// Gives the local account's emptied cache its feeds back from their backup. Best
/// effort: a failure is logged.
fn restore_feeds(dir: &Path, account_id: &str, db: &Db) {
    let restored = match std::fs::read_to_string(feeds_backup_path(dir, account_id)) {
        Ok(xml) => import_opml(db, &xml).map(drop).map_err(|e| e.to_string()),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dirs() -> (tempfile::TempDir, Dirs) {
        let tmp = tempfile::tempdir().unwrap();
        let dirs = Dirs::new(tmp.path().join("data"), tmp.path().join("config"));
        (tmp, dirs)
    }

    #[test]
    fn a_first_start_makes_the_folders_and_has_no_accounts() {
        let (_tmp, dirs) = dirs();
        let app = App::load(&dirs).unwrap();
        assert_eq!(app.accounts(), []);
        assert_eq!(app.active(), None);
        assert_eq!(app.notice(), None);
        assert!(dirs.accounts().is_dir());
    }

    #[test]
    fn an_unreadable_config_is_kept_before_the_settings_reset() {
        let (_tmp, dirs) = dirs();
        std::fs::create_dir_all(&dirs.config).unwrap();
        std::fs::write(dirs.config_file(), b"{ not json").unwrap();

        let app = App::load(&dirs).unwrap();

        assert_eq!(app.notice().unwrap().title, "Settings Reset");
        let backup = dirs.config_file().with_extension("json.bak");
        assert_eq!(std::fs::read(backup).unwrap(), b"{ not json");
    }

    #[test]
    fn startup_stops_when_an_unreadable_config_cant_be_kept() {
        let (_tmp, dirs) = dirs();
        // A folder where the file should be: it can't be read, or copied.
        std::fs::create_dir_all(dirs.config_file()).unwrap();

        let error = App::load(&dirs).unwrap_err();

        assert!(matches!(error, Error::Backup { .. }), "{error}");
        assert!(dirs.config_file().is_dir(), "left as it was");
    }

    #[test]
    fn an_account_this_version_cant_read_is_left_out_and_the_file_kept() {
        let (_tmp, dirs) = dirs();
        std::fs::create_dir_all(&dirs.config).unwrap();
        let raw = br#"{ "accounts": [{ "id": "1", "description": "", "kind": "local" },
                                     { "id": "2", "kind": "future" }] }"#;
        std::fs::write(dirs.config_file(), raw).unwrap();

        let app = App::load(&dirs).unwrap();

        assert_eq!(app.notice().unwrap().title, "Accounts Left Out");
        assert_eq!(app.accounts().len(), 1);
        assert_eq!(app.accounts()[0].description, "Local");
        assert_eq!(
            std::fs::read(dirs.config_file().with_extension("json.bak")).unwrap(),
            raw
        );
    }

    #[test]
    fn imported_feeds_land_in_one_local_account() {
        let (_tmp, dirs) = dirs();
        let opml = r#"<opml version="2.0"><body>
            <outline text="A" type="rss" xmlUrl="https://a.example/feed"/>
        </body></opml>"#;

        import_local(&dirs, opml).unwrap();
        import_local(&dirs, opml).unwrap();

        let app = App::load(&dirs).unwrap();
        let accounts = app.accounts();
        assert_eq!(accounts.len(), 1);
        assert_eq!(app.active(), Some(accounts[0].id.clone()));
        assert_eq!(app.library(&accounts[0].id).unwrap().feeds.len(), 1);
        assert!(matches!(app.library("9"), Err(Error::NoAccount)));
    }

    #[test]
    fn read_and_starred_changes_reach_the_cache_and_its_counts() {
        let (_tmp, dirs) = dirs();
        seed_local(&dirs, 3).unwrap();
        let app = App::load(&dirs).unwrap();
        let id = app.active().unwrap();
        let all = skvaller_core::Selection {
            view: skvaller_core::View::All,
            scope: skvaller_core::Scope::All,
        };
        let first = vec![app.articles(&id, &all).unwrap()[0].id.clone()];
        let unread = |app: &App| -> i64 {
            app.library(&id)
                .unwrap()
                .counts
                .unread_by_feed
                .values()
                .sum()
        };
        assert_eq!(unread(&app), 3);

        app.set_read(&id, &first, true).unwrap();
        app.set_starred(&id, &first, true).unwrap();

        assert_eq!(unread(&app), 2);
        let article = &app.articles(&id, &all).unwrap()[0];
        assert!(article.read && article.starred);
    }
}
