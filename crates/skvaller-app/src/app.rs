//! The accounts and their caches, as the app runs.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{
    Arc, Mutex, MutexGuard, OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard,
};
use std::time::{SystemTime, UNIX_EPOCH};

use skvaller_core::{
    ArticleSummary, Client, Counts, Db, Feed, Folder, ImportReport, Selection, export_opml,
    import_opml, load_full_text,
};
use tokio::sync::{Notify, broadcast, watch};
use tracing::Level;

use crate::config::{Account, AccountKind, Config, FIRST_ACCOUNT_ID, SyncSchedule};
use crate::error::{Error, Result, io};
use crate::gate::Gate;
use crate::sync::SyncStatus;

/// Where the app keeps its settings and caches.
///
/// [`Dirs::system`] gives the system's folders for Skvaller; tests pass temporary
/// folders instead.
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
    /// business following the user between machines.
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

/// A change to an account's cache, as [`App::changes`] tells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// The account whose cache changed.
    pub account_id: String,
    pub what: Changed,
}

/// What kind of data in a cache changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Changed {
    /// Articles were marked read or unread, starred or unstarred: [`App::set_read`] and
    /// [`App::set_starred`].
    Flags,
    /// Every article of a list was marked read: [`App::mark_all_read`].
    AllRead,
    /// Feeds or folders were added, renamed, moved or removed, or a feed's Reader View
    /// turned on or off.
    Feeds,
    /// A sync or a thumbnail fetch stored articles, flags, feeds or thumbnails from
    /// outside the app.
    Articles,
}

/// A change to an account's name or sync settings; fields left `None` stay.
#[derive(Clone, Debug, Default)]
pub struct AccountChange {
    /// Ignored when blank.
    pub description: Option<String>,
    /// Ignored when it is every 0 minutes.
    pub sync: Option<SyncSchedule>,
    /// At least 1.
    pub keep_read_days: Option<u32>,
    /// 0 keeps them.
    pub keep_unread_days: Option<u32>,
    pub sync_on_wake: Option<bool>,
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
    /// Wakes the background sync to look again at whether a sync is due: after a wake
    /// from sleep, a schedule change, or the account's removal.
    pub(crate) wake: Notify,
    /// The computer woke from sleep since the background sync last looked.
    pub(crate) woke: AtomicBool,
    /// The Reader View prefetch started after the last sync, while it may still run.
    pub(crate) prefetch: Mutex<Option<tokio::task::AbortHandle>>,
    /// The thumbnails' fetch started after the last sync or a new feed, while it may
    /// still run.
    pub(crate) thumbnails: Mutex<Option<tokio::task::AbortHandle>>,
}

impl Runtime {
    pub(crate) fn open(dir: &Path, account: &Account) -> skvaller_core::Result<Self> {
        let rt = Self {
            id: account.id.clone(),
            db: Arc::new(Db::open(db_path(dir, &account.id))?),
            client: Mutex::new(None),
            token_rejected: AtomicBool::new(false),
            last_error: Mutex::new(None),
            corrupt: AtomicBool::new(false),
            gate: Gate::default(),
            wake: Notify::new(),
            woke: AtomicBool::new(false),
            prefetch: Mutex::new(None),
            thumbnails: Mutex::new(None),
        };
        // A local cache without feeds, as after an old schema made it start over, gets
        // them back from their backup before a sync backs up the empty list.
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
    config_path: PathBuf,
    pub(crate) config: Mutex<Config>,
    pub(crate) runtimes: RwLock<HashMap<String, Arc<Runtime>>>,
    /// Accounts whose cache didn't open, and why.
    pub(crate) broken: Mutex<HashMap<String, String>>,
    notice: Option<Notice>,
    pub(crate) events: broadcast::Sender<SyncStatus>,
    /// Every change to an account's cache, so each window shows it at once.
    pub(crate) changes: broadcast::Sender<Change>,
    /// Bumped on every change to the settings or the accounts, so each window can show
    /// them at once. Not by a change to a cache: that goes to `changes`.
    config_revision: watch::Sender<u64>,
    /// The runtime [`App::start_sync`] ran on, where every sync runs, away from the UI.
    pub(crate) sync_runtime: OnceLock<tokio::runtime::Handle>,
}

/// How many sync reports, or cache changes, wait for a slow listener before it misses
/// the oldest. A listener that falls behind is told so, and reloads.
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
            config_path,
            config: Mutex::new(config),
            runtimes: RwLock::new(runtimes),
            broken: Mutex::new(broken),
            notice,
            events: broadcast::channel(EVENT_BACKLOG).0,
            changes: broadcast::channel(EVENT_BACKLOG).0,
            config_revision: watch::Sender::new(0),
            sync_runtime: OnceLock::new(),
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

    /// Makes the account the one the main window shows, saved for the next start.
    ///
    /// # Errors
    ///
    /// [`Error::NoAccount`] or [`Error::Broken`] for an account without a cache, and
    /// [`Error::Io`] when config.json can't be saved.
    pub fn set_active(&self, account_id: &str) -> Result<()> {
        self.runtime(account_id)?;
        self.update_config(|config| config.active = Some(account_id.to_owned()))?;
        self.announce();
        Ok(())
    }

    /// Changes the settings as `change` says, saved to config.json, and tells every
    /// window.
    ///
    /// The change is made to the settings as they are when saved, so it can't undo
    /// another made meanwhile.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when config.json can't be saved; the settings stay as they were.
    pub fn change_settings(&self, change: impl FnOnce(&mut crate::config::Settings)) -> Result<()> {
        self.update_config(|config| change(&mut config.settings))?;
        // Turned off: no image host hears from the app from now on, not even for the
        // rest of a run under way.
        if self.settings().thumbnail_size == crate::ThumbnailSize::None {
            for rt in self.runtimes() {
                stop(&rt.thumbnails);
            }
        }
        self.announce();
        Ok(())
    }

    /// Changes the account's name or sync settings, saved and told to every window. A
    /// schedule change reaches its background sync at once.
    ///
    /// # Errors
    ///
    /// [`Error::NoAccount`] or [`Error::Broken`] for an account without a cache, and
    /// [`Error::Io`] when config.json can't be saved; the account stays as it was.
    pub fn update_account(&self, account_id: &str, change: &AccountChange) -> Result<()> {
        let rt = self.runtime(account_id)?;
        let mut schedule_changed = false;
        self.update_config(|config| {
            let Some(account) = config.accounts.iter_mut().find(|a| a.id == account_id) else {
                return;
            };
            if let Some(description) = change.description.as_deref().map(str::trim)
                && !description.is_empty()
            {
                description.clone_into(&mut account.description);
            }
            // Every 0 minutes would sync without end, and config.json can't load it.
            if let Some(sync) = change.sync.filter(|&s| s != SyncSchedule::Every(0)) {
                schedule_changed = account.sync != sync;
                account.sync = sync;
            }
            if let Some(days) = change.keep_read_days {
                account.keep_read_days = days.max(1);
            }
            if let Some(days) = change.keep_unread_days {
                account.keep_unread_days = days;
            }
            if let Some(on) = change.sync_on_wake {
                schedule_changed |= account.sync_on_wake != on;
                account.sync_on_wake = on;
            }
        })?;
        if schedule_changed {
            rt.wake.notify_one();
        }
        self.announce();
        Ok(())
    }

    /// The main window's panes, as last saved.
    #[must_use]
    pub fn layout(&self) -> crate::Layout {
        lock(&self.config).layout
    }

    /// Saves the main window's panes to config.json.
    ///
    /// The windows aren't told: only the main window shows the layout, and it changed it.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when config.json can't be saved.
    pub fn set_layout(&self, layout: crate::Layout) -> Result<()> {
        self.update_config(|config| config.layout = layout)
    }

    /// Changes in the settings or the accounts: the revision changes on each. A change
    /// to an account's cache goes to [`App::changes`] instead.
    #[must_use]
    pub fn watch_config(&self) -> watch::Receiver<u64> {
        self.config_revision.subscribe()
    }

    /// Tells every window that the settings or the accounts changed.
    pub(crate) fn announce(&self) {
        self.config_revision.send_modify(|r| *r += 1);
    }

    /// Hears every change to an account's cache that a window shows, from every method
    /// that makes one, as it is made. Reader View pages, stored by [`App::full_text`]
    /// and the prefetch after a sync, aren't told: no list or sidebar shows them.
    #[must_use]
    pub fn changes(&self) -> broadcast::Receiver<Change> {
        self.changes.subscribe()
    }

    /// Tells every window that the account's cache changed.
    pub(crate) fn cache_changed(&self, account_id: &str, what: Changed) {
        // No listener yet is fine: a window that opens later reads the cache as it is.
        let _ = self.changes.send(Change {
            account_id: account_id.to_owned(),
            what,
        });
    }

    /// Changes the config and saves it, giving what `change` gives. The change takes only
    /// once saved, so the app never shows what the file doesn't have.
    pub(crate) fn update_config<T>(&self, change: impl FnOnce(&mut Config) -> T) -> Result<T> {
        let mut config = lock(&self.config);
        let mut next = config.clone();
        let out = change(&mut next);
        next.save(&self.config_path)
            .map_err(io("Saving", &self.config_path))?;
        *config = next;
        Ok(out)
    }

    /// The account's unread articles, for the account switcher.
    ///
    /// # Errors
    ///
    /// As [`App::library`].
    pub fn unread_total(&self, account_id: &str) -> Result<i64> {
        Ok(self.runtime(account_id)?.db.unread_total()?)
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

    /// The account's article `id` as the list shows it, if it is cached.
    ///
    /// # Errors
    ///
    /// As [`App::library`].
    pub fn article(&self, account_id: &str, id: &str) -> Result<Option<ArticleSummary>> {
        Ok(self.runtime(account_id)?.db.article(id)?)
    }

    /// The account's articles in `selection` that match `query`, newest first, from the
    /// cache only. See [`skvaller_core::Db::search`].
    ///
    /// # Errors
    ///
    /// As [`App::library`].
    pub fn search(
        &self,
        account_id: &str,
        selection: &Selection,
        query: &str,
    ) -> Result<Vec<ArticleSummary>> {
        Ok(self.runtime(account_id)?.db.search(selection, query)?)
    }

    /// The article's HTML, as the feed gave it: untrusted, never to run scripts.
    ///
    /// # Errors
    ///
    /// As [`App::library`].
    pub fn article_html(&self, account_id: &str, article_id: &str) -> Result<Option<String>> {
        Ok(self.runtime(account_id)?.db.article_html(article_id)?)
    }

    /// The article's Reader View: the text of its web page, from the cache or else
    /// fetched, on the sync runtime. Untrusted, as the feed's HTML is.
    ///
    /// `Ok(None)` when the page has nothing readable, or was given up on and `retry` is
    /// false.
    ///
    /// # Errors
    ///
    /// As [`App::library`], and [`Error::Core`] when the page can't be fetched.
    ///
    /// # Panics
    ///
    /// When [`App::start_sync`] hasn't run.
    pub async fn full_text(
        &self,
        account_id: &str,
        article_id: &str,
        retry: bool,
    ) -> Result<Option<String>> {
        let db = Arc::clone(&self.runtime(account_id)?.db);
        let id = article_id.to_owned();
        let fetch = self
            .sync_runtime
            .get()
            .expect("start_sync runs before Reader View")
            .spawn(async move { load_full_text(&db, &id, retry).await });
        match fetch.await {
            Ok(page) => Ok(page?),
            Err(e) => {
                // A panic is a bug, worth a log; a shutdown cancels it, and nothing
                // waits for the page then. Either way the article has no Reader View.
                if e.is_panic() {
                    tracing::event!(
                        name: "full_text.load.panic",
                        Level::ERROR,
                        article.id = %article_id,
                        error.message = %e,
                        "Reader View of article {{article.id}} panicked: {{error.message}}",
                    );
                }
                Ok(None)
            }
        }
    }

    /// Opens the feed's articles in Reader View from now on, or stops, and tells every
    /// window.
    ///
    /// # Errors
    ///
    /// As [`App::library`].
    pub fn set_feed_reader_view(&self, account_id: &str, feed_id: &str, on: bool) -> Result<()> {
        self.runtime(account_id)?
            .db
            .set_feed_reader_view(feed_id, on)?;
        self.cache_changed(account_id, Changed::Feeds);
        Ok(())
    }

    /// Marks articles read or unread in the cache. The next sync sends the change, and
    /// a sync never undoes it before then.
    ///
    /// # Errors
    ///
    /// As [`App::library`].
    pub fn set_read(&self, account_id: &str, ids: &[String], read: bool) -> Result<()> {
        self.runtime(account_id)?.db.set_read(ids, read)?;
        self.cache_changed(account_id, Changed::Flags);
        Ok(())
    }

    /// Marks every article of a list read, as [`App::set_read`] does, and tells the
    /// windows so as [`Changed::AllRead`], not as a read change.
    ///
    /// # Errors
    ///
    /// As [`App::library`].
    pub fn mark_all_read(&self, account_id: &str, ids: &[String]) -> Result<()> {
        self.runtime(account_id)?.db.set_read(ids, true)?;
        self.cache_changed(account_id, Changed::AllRead);
        Ok(())
    }

    /// Stars or unstars articles in the cache, sent on the next sync as
    /// [`App::set_read`]'s changes are.
    ///
    /// # Errors
    ///
    /// As [`App::library`].
    pub fn set_starred(&self, account_id: &str, ids: &[String], starred: bool) -> Result<()> {
        self.runtime(account_id)?.db.set_starred(ids, starred)?;
        self.cache_changed(account_id, Changed::Flags);
        Ok(())
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
        if let Some(rt) = read(&self.runtimes).get(id) {
            return Ok(Arc::clone(rt));
        }
        Err(lock(&self.broken)
            .get(id)
            .map_or(Error::NoAccount, |why| Error::Broken(why.clone())))
    }

    /// Every account's runtime, as now.
    pub(crate) fn runtimes(&self) -> Vec<Arc<Runtime>> {
        read(&self.runtimes).values().cloned().collect()
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

/// Adds feeds from an OPML file to the local account, adding that account if there is none.
///
/// For testing on a machine without accounts. Run it before [`App::load`], with no app
/// running on the same folders.
///
/// # Errors
///
/// As [`App::load`], and [`Error::Core`] when the file isn't OPML or can't be stored.
pub fn import_local(dirs: &Dirs, opml: &[u8]) -> Result<ImportReport> {
    Ok(import_opml(&local_db(dirs)?, opml)?)
}

/// Adds `count` synthetic articles to the local account, adding that account if there is
/// none: a way to test the article list at 5,000+ items. Returns how many were stored.
///
/// # Errors
///
/// As [`App::load`], and [`Error::Core`] when the articles can't be stored.
pub fn seed_local(dirs: &Dirs, count: usize) -> Result<usize> {
    Ok(skvaller_core::seed_articles(
        &local_db(dirs)?,
        count,
        now_secs() * 1000,
    )?)
}

/// Now, in Unix seconds.
pub(crate) fn now_secs() -> i64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
}

/// Opens the local account's cache, making the account active, or adding it first.
fn local_db(dirs: &Dirs) -> Result<Db> {
    let app = App::load(dirs)?;
    let id = app.update_config(|config| {
        let local = config
            .accounts
            .iter()
            .find(|a| a.kind == AccountKind::Local);
        let id = if let Some(account) = local {
            account.id.clone()
        } else {
            let id = config.take_id();
            let description = default_description(&AccountKind::Local);
            config
                .accounts
                .push(Account::new(id.clone(), description, AccountKind::Local));
            id
        };
        config.active = Some(id.clone());
        id
    })?;
    Ok(Db::open(db_path(&app.accounts_dir, &id))?)
}

impl App {
    /// Starts the account's background task for `slot` on the sync runtime, unless
    /// `skip` says not to, one still runs, or the account was removed. `skip` is asked
    /// under the slot's lock, which whatever stops the task takes too.
    pub(crate) fn spawn_once<F>(
        &self,
        rt: &Runtime,
        slot: &Mutex<Option<tokio::task::AbortHandle>>,
        skip: impl FnOnce() -> bool,
        work: impl FnOnce(Arc<Runtime>) -> F,
    ) where
        F: Future<Output = ()> + Send + 'static,
    {
        let mut running = lock(slot);
        if skip() || running.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        // Removed meanwhile: its removal takes the slot's lock only after taking the
        // account from the runtimes, so a task started here is one it aborts.
        let (Some(runtime), Ok(rt)) = (self.sync_runtime.get(), self.runtime(&rt.id)) else {
            return;
        };
        *running = Some(runtime.spawn(work(rt)).abort_handle());
    }
}

/// Aborts the background task in `slot`, if one runs. It stops at its next await, so
/// no write to the cache is cut short.
pub(crate) fn stop(slot: &Mutex<Option<tokio::task::AbortHandle>>) {
    if let Some(task) = lock(slot).take() {
        task.abort();
    }
}

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn read<T>(lock: &RwLock<T>) -> RwLockReadGuard<'_, T> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn write<T>(lock: &RwLock<T>) -> RwLockWriteGuard<'_, T> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
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

/// Moves a single-account `cache.db` from the data folder to the first account's place.
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

/// The name an account gets until the user changes it: `FreshRSS`, `The Old Reader`, the
/// server's host name, or `Local`.
pub(crate) fn default_description(kind: &AccountKind) -> String {
    match kind {
        AccountKind::Greader { server, .. } => {
            let rest = server
                .split_once("://")
                .map_or(server.as_str(), |(_, rest)| rest);
            let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
            let host = host.rsplit_once('@').map_or(host, |(_, host)| host);
            let host = host.split(':').next().unwrap_or(host);
            if path.trim_end_matches('/').ends_with("api/greader.php") {
                "FreshRSS".to_owned()
            } else if host == "theoldreader.com" {
                "The Old Reader".to_owned()
            } else {
                host.to_owned()
            }
        }
        AccountKind::Local => "Local".to_owned(),
    }
}

pub(crate) fn db_path(dir: &Path, id: &str) -> PathBuf {
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
pub(crate) fn remove_db_files(dir: &Path, id: &str) -> bool {
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
        // A backup of no feeds, which the OPML reader doesn't take, restores nothing.
        Ok(xml) if !xml.contains("<outline") => Ok(()),
        Ok(xml) => import_opml(db, xml.as_bytes())
            .map(drop)
            .map_err(|e| e.to_string()),
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
pub(crate) mod tests {
    use super::*;

    pub(crate) fn dirs() -> (tempfile::TempDir, Dirs) {
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
    fn a_sync_account_is_named_after_its_service_or_host() {
        let name = |server: &str| {
            default_description(&AccountKind::Greader {
                server: server.into(),
                username: "me".into(),
            })
        };
        assert_eq!(name("https://rss.example.org/api/greader.php"), "FreshRSS");
        assert_eq!(name("https://theoldreader.com"), "The Old Reader");
        assert_eq!(
            name("https://news.example.org:8080/reader"),
            "news.example.org"
        );
    }

    #[test]
    fn changed_settings_are_saved_and_announced() {
        let (_tmp, dirs) = dirs();
        let app = App::load(&dirs).unwrap();
        let watch = app.watch_config();
        app.change_settings(|s| s.theme = crate::Theme::Espresso)
            .unwrap();

        assert!(watch.has_changed().unwrap());
        assert_eq!(app.settings().theme, crate::Theme::Espresso);
        let saved = App::load(&dirs).unwrap();
        assert_eq!(saved.settings().theme, crate::Theme::Espresso);
    }

    #[test]
    fn an_account_change_keeps_a_name_a_schedule_and_at_least_a_day() {
        let (_tmp, dirs) = dirs();
        let opml = r#"<opml version="2.0"><body>
            <outline text="A" type="rss" xmlUrl="https://a.example/feed"/>
        </body></opml>"#;
        import_local(&dirs, opml.as_bytes()).unwrap();
        let app = App::load(&dirs).unwrap();
        let id = app.accounts()[0].id.clone();
        let change = |c: AccountChange| app.update_account(&id, &c).unwrap();

        change(AccountChange {
            description: Some("  News  ".into()),
            keep_read_days: Some(0),
            sync: Some(SyncSchedule::Manual),
            ..AccountChange::default()
        });
        change(AccountChange {
            description: Some("   ".into()),
            sync: Some(SyncSchedule::Every(0)),
            ..AccountChange::default()
        });

        let account = &App::load(&dirs).unwrap().accounts()[0];
        assert_eq!(account.description, "News");
        assert_eq!(account.keep_read_days, 1);
        assert_eq!(account.sync, SyncSchedule::Manual);
    }

    #[test]
    fn imported_feeds_land_in_one_local_account() {
        let (_tmp, dirs) = dirs();
        let opml = r#"<opml version="2.0"><body>
            <outline text="A" type="rss" xmlUrl="https://a.example/feed"/>
        </body></opml>"#;

        import_local(&dirs, opml.as_bytes()).unwrap();
        import_local(&dirs, opml.as_bytes()).unwrap();

        let app = App::load(&dirs).unwrap();
        let accounts = app.accounts();
        assert_eq!(accounts.len(), 1);
        assert_eq!(app.active(), Some(accounts[0].id.clone()));
        assert_eq!(app.library(&accounts[0].id).unwrap().feeds.len(), 1);
        assert!(matches!(app.library("9"), Err(Error::NoAccount)));
    }

    #[test]
    fn the_active_account_is_saved_for_the_next_start() {
        let (_tmp, dirs) = dirs();
        std::fs::create_dir_all(&dirs.config).unwrap();
        let raw = br#"{ "accounts": [{ "id": "1", "description": "A", "kind": "local" },
                                     { "id": "2", "description": "B", "kind": "local" }],
                        "active": "1" }"#;
        std::fs::write(dirs.config_file(), raw).unwrap();
        let app = App::load(&dirs).unwrap();

        app.set_active("2").unwrap();

        assert_eq!(app.active().as_deref(), Some("2"));
        assert_eq!(App::load(&dirs).unwrap().active().as_deref(), Some("2"));
        assert!(matches!(app.set_active("9"), Err(Error::NoAccount)));
        assert_eq!(app.unread_total("2").unwrap(), 0);
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

        let mut changes = app.changes();
        let config = app.watch_config();

        app.set_read(&id, &first, true).unwrap();
        app.set_starred(&id, &first, true).unwrap();
        app.mark_all_read(&id, &first).unwrap();

        assert_eq!(unread(&app), 2);
        let article = &app.articles(&id, &all).unwrap()[0];
        assert!(article.read && article.starred);
        let heard = |what| Change {
            account_id: id.clone(),
            what,
        };
        assert_eq!(changes.try_recv().unwrap(), heard(Changed::Flags));
        assert_eq!(changes.try_recv().unwrap(), heard(Changed::Flags));
        assert_eq!(changes.try_recv().unwrap(), heard(Changed::AllRead));
        assert!(!config.has_changed().unwrap(), "not a config change");
    }
}
