//! Adding, signing in to and removing accounts, and their subscriptions as OPML, as the
//! Tauri version's commands do.
//!
//! Each change saves config.json first and only then takes, and tells every window
//! through [`App::watch_config`].

use std::sync::Arc;
use std::sync::atomic::Ordering;

use skvaller_core::{Client, ImportReport, delete_token, export_opml, import_opml, save_token};
use tracing::Level;

use crate::app::{App, Runtime, default_description, lock, read, remove_db_files, stop, write};
use crate::config::{Account, AccountKind, Config, DEFAULT_KEEP_UNREAD_DAYS, SyncSchedule};
use crate::error::{Error, Result};

/// The largest OPML file read: far more than any real subscription list.
const MAX_OPML_BYTES: usize = 10 * 1024 * 1024;

impl App {
    /// Signs in to a Google Reader API server, saves the token in the keychain, and
    /// makes the account the active one. Returns its ID.
    ///
    /// An account already added, such as one whose token the server rejected, gets the
    /// new token and syncs again; any other becomes a new account, which syncs at once.
    /// The password goes only to the server.
    ///
    /// # Errors
    ///
    /// [`Error::Core`] when signing in fails: `Unauthorized` for a wrong user name or
    /// password, `BadUrl` for an address that isn't one, `Http` for the network, or
    /// `Keychain`. [`Error::Io`] when config.json can't be saved.
    ///
    /// # Panics
    ///
    /// When [`App::start_sync`] hasn't run.
    pub async fn sign_in(
        self: &Arc<Self>,
        server: &str,
        username: &str,
        password: &str,
    ) -> Result<String> {
        let app = Arc::clone(self);
        let (server, username, password) = (
            server.trim().to_owned(),
            username.trim().to_owned(),
            password.to_owned(),
        );
        // On the sync runtime, which the network client needs, away from the UI.
        self.sync_runtime
            .get()
            .expect("start_sync runs before signing in")
            .spawn(async move { app.sign_in_now(&server, &username, &password).await })
            .await
            .map_err(|e| Error::Core(skvaller_core::Error::Local(e.to_string())))?
    }

    async fn sign_in_now(
        self: &Arc<Self>,
        server: &str,
        username: &str,
        password: &str,
    ) -> Result<String> {
        let client = Client::login(server, username, password).await?;
        // The API address that answered, which may have FreshRSS's path added.
        let kind = AccountKind::Greader {
            server: client.base().to_owned(),
            username: username.to_owned(),
        };
        let key = kind
            .keychain_key()
            .expect("a sync account has a keychain entry");
        let token = client.token().to_owned();
        tokio::task::spawn_blocking(move || save_token(&key, &token))
            .await
            .map_err(|e| Error::Core(skvaller_core::Error::Local(e.to_string())))??;

        let (rt, added) = self.add_account(&kind)?;
        if added {
            // Its first sync, just started, may use this client or the saved token.
            *lock(&rt.client) = Some(client);
        } else {
            // A sync still running with the old token would reject the new one.
            let no_sync = rt.gate.stop().await;
            *lock(&rt.client) = Some(client);
            rt.token_rejected.store(false, Ordering::Relaxed);
            *lock(&rt.last_error) = None;
            drop(no_sync);
            self.sync_soon(Arc::clone(&rt));
        }
        self.announce();
        Ok(rt.id.clone())
    }

    /// Adds the local account, whose feeds Skvaller fetches itself, and makes it active.
    /// There is at most one: with one already, that one becomes active. Returns its ID.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] when config.json can't be saved, and [`Error::Core`] when its cache
    /// can't be made.
    pub fn add_local_account(self: &Arc<Self>) -> Result<String> {
        let (rt, _) = self.add_account(&AccountKind::Local)?;
        self.announce();
        Ok(rt.id.clone())
    }

    /// Makes the account of `kind` active, adding it first if there is none: then it
    /// gets a new ID and its background sync starts. Says whether it was added.
    fn add_account(self: &Arc<Self>, kind: &AccountKind) -> Result<(Arc<Runtime>, bool)> {
        let existing = |config: &Config| {
            config
                .accounts
                .iter()
                .find(|a| a.kind == *kind)
                .map(|a| a.id.clone())
        };
        // The ID is taken under the config's lock, so two additions never share one.
        let id = self.update_config(|config| match existing(config) {
            Some(id) => {
                config.active = Some(id.clone());
                Err(id)
            }
            None => Ok(config.take_id()),
        })?;
        let id = match id {
            Ok(id) => id,
            Err(existing) => return Ok((self.runtime(&existing)?, false)),
        };
        let account = {
            let config = lock(&self.config);
            let mut description = default_description(kind);
            if config.accounts.iter().any(|a| a.description == description)
                && let AccountKind::Greader { username, .. } = kind
            {
                description = format!("{description} ({username})");
            }
            Account {
                id: id.clone(),
                description,
                kind: kind.clone(),
                sync: SyncSchedule::default(),
                keep_read_days: skvaller_core::DEFAULT_KEEP_READ_DAYS,
                keep_unread_days: DEFAULT_KEEP_UNREAD_DAYS,
                sync_on_wake: false,
            }
        };
        let rt = Arc::new(Runtime::open(&self.accounts_dir, &account)?);
        let added = self.update_config(|config| {
            // Added meanwhile, as by a second click: that one stays.
            if let Some(other) = existing(config) {
                config.active = Some(other.clone());
                return Err(other);
            }
            config.accounts.push(account);
            config.active = Some(id.clone());
            Ok(())
        });
        match added {
            Ok(Ok(())) => {
                write(&self.runtimes).insert(id, Arc::clone(&rt));
                self.start_loop(Arc::clone(&rt));
                Ok((rt, true))
            }
            Ok(Err(other)) => {
                drop(rt);
                remove_db_files(&self.accounts_dir, &id);
                Ok((self.runtime(&other)?, false))
            }
            Err(e) => {
                drop(rt);
                remove_db_files(&self.accounts_dir, &id);
                Err(e)
            }
        }
    }

    /// Removes the account: from config.json, its token from the keychain and its
    /// cache, queued changes included. Another account becomes active.
    ///
    /// Once the account is gone from config.json, it is removed. Removing the token and
    /// the cache after that is best effort: a failure is logged, and a cache left
    /// behind, such as one still open on Windows, is removed on the next start.
    ///
    /// # Errors
    ///
    /// [`Error::NoAccount`] for an account already gone, and [`Error::Io`] when
    /// config.json can't be saved; the account then stays as it was.
    pub async fn remove_account(&self, account_id: &str) -> Result<()> {
        let account = self.account(account_id).ok_or(Error::NoAccount)?;
        // One whose cache didn't open has no runtime.
        let rt = read(&self.runtimes).get(account_id).cloned();
        {
            // No sync may write while the cache goes.
            let _no_sync = match &rt {
                Some(rt) => Some(rt.gate.stop().await),
                None => None,
            };
            self.update_config(|config| {
                config.accounts.retain(|a| a.id != account_id);
                if config.active.as_deref() == Some(account_id) {
                    config.active = config.accounts.first().map(|a| a.id.clone());
                }
            })?;
            // Only once saved: until then, the account goes on as before.
            write(&self.runtimes).remove(account_id);
            lock(&self.broken).remove(account_id);
            if let Some(rt) = &rt {
                // Its background sync looks again, finds the account gone, and ends.
                rt.wake.notify_one();
                *lock(&rt.client) = None;
                // Aborted between writes, at an await: no write is cut short.
                stop(&rt.prefetch);
                stop(&rt.thumbnails);
            }
        }
        drop(rt);

        if let Some(key) = account.kind.keychain_key()
            && let Err(e) = delete_token(&key)
        {
            tracing::event!(
                name: "account.token.remove.failure",
                Level::WARN,
                error.message = %e,
                "removing an account's token failed: {{error.message}}",
            );
        }
        if !remove_db_files(&self.accounts_dir, account_id)
            && let Err(e) = self.update_config(|config| config.removed.push(account_id.to_owned()))
        {
            tracing::event!(
                name: "account.cache.remove.failure",
                Level::WARN,
                account.id = %account_id,
                error.message = %e,
                "the cache of removed account {{account.id}} stays on disk: {{error.message}}",
            );
        }
        self.announce();
        Ok(())
    }

    /// Adds the feeds of an OPML file's text to the local account, then syncs it to
    /// fetch them.
    ///
    /// # Errors
    ///
    /// [`Error::NotLocal`] for a sync account, whose server keeps its subscriptions;
    /// [`Error::NoAccount`] or [`Error::Broken`] for an account without a cache; and
    /// [`Error::Core`] for a file that is too large or not OPML.
    pub fn import_opml(self: &Arc<Self>, account_id: &str, xml: &str) -> Result<ImportReport> {
        let rt = self.local(account_id)?;
        if xml.len() > MAX_OPML_BYTES {
            return Err(Error::Core(skvaller_core::Error::Local(
                "the file is too large for an OPML file".into(),
            )));
        }
        let report = import_opml(&rt.db, xml)?;
        self.back_up_feeds(&rt);
        if report.added > 0 {
            self.sync_soon(rt);
        }
        Ok(report)
    }

    /// The account's subscriptions as OPML, and a file name to suggest for them.
    ///
    /// # Errors
    ///
    /// [`Error::NoAccount`] or [`Error::Broken`] for an account without a cache, and
    /// [`Error::Core`] when its feeds can't be read.
    pub fn export_opml(&self, account_id: &str) -> Result<(String, String)> {
        let rt = self.runtime(account_id)?;
        let description = self
            .account(account_id)
            .map(|a| a.description)
            .unwrap_or_default();
        let xml = export_opml(&rt.db, &description)?;
        // No characters a file system refuses in the suggested name.
        let name: String = description
            .chars()
            .map(|c| if "/\\:*?\"<>|".contains(c) { '-' } else { c })
            .collect();
        Ok((xml, format!("{} Subscriptions.opml", name.trim())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::db_path;
    use crate::app::tests::dirs;

    const OPML: &str = r#"<opml version="2.0"><body>
        <outline text="A" type="rss" xmlUrl="https://a.example/feed"/>
    </body></opml>"#;

    #[test]
    fn the_local_account_is_added_once_and_made_active() {
        let (_tmp, dirs) = dirs();
        let app = Arc::new(App::load(&dirs).unwrap());
        let watch = app.watch_config();

        let id = app.add_local_account().unwrap();

        assert!(watch.has_changed().unwrap());
        assert_eq!(app.active().as_deref(), Some(id.as_str()));
        assert_eq!(app.add_local_account().unwrap(), id);
        assert_eq!(app.accounts().len(), 1);
        let saved = App::load(&dirs).unwrap();
        assert_eq!(saved.accounts()[0].kind, AccountKind::Local);
    }

    #[test]
    fn opml_goes_into_the_local_account_and_back_out() {
        let (_tmp, dirs) = dirs();
        let app = Arc::new(App::load(&dirs).unwrap());
        let id = app.add_local_account().unwrap();

        let report = app.import_opml(&id, OPML).unwrap();
        let (xml, name) = app.export_opml(&id).unwrap();

        assert_eq!(report.added, 1);
        assert!(xml.contains("https://a.example/feed"));
        assert_eq!(name, "Local Subscriptions.opml");
    }

    #[tokio::test]
    async fn a_removed_account_loses_its_cache_and_the_next_becomes_active() {
        let (_tmp, dirs) = dirs();
        let app = Arc::new(App::load(&dirs).unwrap());
        let id = app.add_local_account().unwrap();
        app.import_opml(&id, OPML).unwrap();
        let cache = db_path(&app.accounts_dir, &id);
        assert!(cache.exists());

        app.remove_account(&id).await.unwrap();

        assert_eq!(app.accounts().len(), 0);
        assert_eq!(app.active(), None);
        assert!(!cache.exists());
        assert!(matches!(app.status(&id), Err(Error::NoAccount)));
        assert!(matches!(
            app.remove_account(&id).await,
            Err(Error::NoAccount)
        ));
        assert_eq!(App::load(&dirs).unwrap().accounts().len(), 0);
    }
}
