//! Managing the local account's feeds and folders.
//!
//! Only the local account keeps its own subscriptions; a sync account's are managed on
//! its server. Each change is saved to the cache, backed up as OPML and told to
//! [`App::changes`] listeners, so every window shows it at once.

use std::sync::Arc;

use skvaller_core::{Db, subscribe};

use crate::app::{App, Changed, Runtime, now_secs};
use crate::config::AccountKind;
use crate::error::{Error, Result, task_failed};

impl App {
    /// Subscribes the local account to the feed at `url`, or to the one a web page
    /// there links to, in `folder_id` or in no folder. Returns the feed's ID.
    ///
    /// Fetches the feed on the sync runtime, so it can take seconds; the UI waits for it
    /// in the background, as it does for Reader View.
    ///
    /// # Errors
    ///
    /// As [`App::rename_feed`], and [`Error::Core`] with
    /// [`skvaller_core::Error::BadUrl`] or [`skvaller_core::Error::NoFeed`] for an
    /// address without a feed, or another when it can't be fetched.
    ///
    /// # Panics
    ///
    /// When [`App::start_sync`] hasn't run.
    pub async fn add_feed(
        &self,
        account_id: &str,
        url: &str,
        folder_id: Option<&str>,
    ) -> Result<String> {
        let rt = self.local(account_id)?;
        let db = Arc::clone(&rt.db);
        let (url, folder_id) = (url.to_owned(), folder_id.map(str::to_owned));
        let now = now_secs();
        let added = self
            .sync_runtime
            .get()
            .expect("start_sync runs before subscribing")
            .spawn(async move { subscribe(&db, &url, folder_id.as_deref(), now).await });
        let id = added.await.map_err(|e| task_failed(&e))??;
        self.feeds_changed(&rt);
        // Its articles are new to the cache: their thumbnails now, not at the next sync.
        self.fetch_thumbnails(&rt);
        Ok(id)
    }

    /// Removes a feed of the local account, with its articles, starred ones too.
    ///
    /// # Errors
    ///
    /// As [`App::rename_feed`].
    pub fn unsubscribe(&self, account_id: &str, feed_id: &str) -> Result<()> {
        self.edit_local(account_id, |db| db.unsubscribe(feed_id))
    }

    /// Renames a feed of the local account. The title is trimmed.
    ///
    /// # Errors
    ///
    /// [`Error::NotLocal`] for a sync account; [`Error::NoAccount`] or
    /// [`Error::Broken`] for an account without a cache; [`Error::NoTitle`] for an empty
    /// title; and [`Error::Core`] when the cache can't be written.
    pub fn rename_feed(&self, account_id: &str, feed_id: &str, title: &str) -> Result<()> {
        let title = non_empty(title)?;
        self.edit_local(account_id, |db| db.rename_feed(feed_id, title))
    }

    /// Moves a feed of the local account into a folder, or out of any with `None`.
    ///
    /// # Errors
    ///
    /// As [`App::rename_feed`].
    pub fn move_feed(
        &self,
        account_id: &str,
        feed_id: &str,
        folder_id: Option<&str>,
    ) -> Result<()> {
        self.edit_local(account_id, |db| db.move_feed(feed_id, folder_id))
    }

    /// The local account's folder with this title, ignoring case, or else a new one.
    /// Returns its ID. The title is trimmed.
    ///
    /// # Errors
    ///
    /// As [`App::rename_feed`].
    pub fn create_folder(&self, account_id: &str, title: &str) -> Result<String> {
        let title = non_empty(title)?;
        self.edit_local(account_id, |db| db.create_folder(title))
    }

    /// Renames a folder of the local account. The title is trimmed.
    ///
    /// # Errors
    ///
    /// As [`App::rename_feed`].
    pub fn rename_folder(&self, account_id: &str, folder_id: &str, title: &str) -> Result<()> {
        let title = non_empty(title)?;
        self.edit_local(account_id, |db| db.rename_folder(folder_id, title))
    }

    /// Removes a folder of the local account. Its feeds stay, in no folder.
    ///
    /// # Errors
    ///
    /// As [`App::rename_feed`].
    pub fn delete_folder(&self, account_id: &str, folder_id: &str) -> Result<()> {
        self.edit_local(account_id, |db| db.delete_folder(folder_id))
    }

    /// The account's runtime, if it is the local account.
    pub(crate) fn local(&self, account_id: &str) -> Result<Arc<Runtime>> {
        let account = self.account(account_id).ok_or(Error::NoAccount)?;
        if account.kind != AccountKind::Local {
            return Err(Error::NotLocal);
        }
        self.runtime(account_id)
    }

    /// Runs `edit` on the local account's cache, then backs up the subscriptions and
    /// tells every window.
    fn edit_local<T>(
        &self,
        account_id: &str,
        edit: impl FnOnce(&Db) -> skvaller_core::Result<T>,
    ) -> Result<T> {
        let rt = self.local(account_id)?;
        let done = edit(&rt.db)?;
        self.feeds_changed(&rt);
        Ok(done)
    }

    /// Backs up the changed subscriptions and tells every window.
    pub(crate) fn feeds_changed(&self, rt: &Runtime) {
        self.back_up_feeds(rt);
        self.cache_changed(&rt.id, Changed::Feeds);
    }
}

fn non_empty(title: &str) -> Result<&str> {
    match title.trim() {
        "" => Err(Error::NoTitle),
        title => Ok(title),
    }
}

#[cfg(test)]
mod tests {
    use crate::app::tests::dirs;
    use crate::app::{App, Change, Changed, import_local};
    use crate::error::Error;

    const OPML: &str = r#"<opml version="2.0"><body>
        <outline text="A" type="rss" xmlUrl="https://a.example/feed"/>
    </body></opml>"#;

    /// The local account's ID and its one feed's.
    fn local(app: &App) -> (String, String) {
        let id = app.active().unwrap();
        let feed = app.library(&id).unwrap().feeds[0].id.clone();
        (id, feed)
    }

    #[test]
    fn feeds_and_folders_change_in_the_cache_and_its_backup() {
        let (_tmp, dirs) = dirs();
        import_local(&dirs, OPML.as_bytes()).unwrap();
        let app = App::load(&dirs).unwrap();
        let (id, feed) = local(&app);
        let config = app.watch_config();
        let mut changes = app.changes();

        let folder = app.create_folder(&id, "  Tech  ").unwrap();
        app.move_feed(&id, &feed, Some(&folder)).unwrap();
        app.rename_feed(&id, &feed, " Renamed ").unwrap();
        app.rename_folder(&id, &folder, "News").unwrap();

        let feeds = Change {
            account_id: id.clone(),
            what: Changed::Feeds,
        };
        for _ in 0..4 {
            assert_eq!(changes.try_recv().unwrap(), feeds);
        }
        assert!(!config.has_changed().unwrap(), "not a config change");
        let lib = app.library(&id).unwrap();
        assert_eq!(lib.folders[0].title, "News");
        assert_eq!(lib.feeds[0].title, "Renamed");
        assert_eq!(lib.feeds[0].folder_id.as_deref(), Some(folder.as_str()));
        assert_eq!(app.create_folder(&id, "news").unwrap(), folder);
        let backup =
            std::fs::read_to_string(dirs.data.join(format!("accounts/{id}.opml"))).unwrap();
        assert!(
            backup.contains("Renamed") && backup.contains("News"),
            "{backup}"
        );

        app.delete_folder(&id, &folder).unwrap();
        let lib = app.library(&id).unwrap();
        assert_eq!(lib.folders.len(), 0);
        assert_eq!(lib.feeds[0].folder_id, None);

        app.unsubscribe(&id, &feed).unwrap();
        assert_eq!(app.library(&id).unwrap().feeds.len(), 0);
    }

    #[test]
    fn an_empty_title_is_refused() {
        let (_tmp, dirs) = dirs();
        import_local(&dirs, OPML.as_bytes()).unwrap();
        let app = App::load(&dirs).unwrap();
        let (id, feed) = local(&app);

        assert!(matches!(
            app.rename_feed(&id, &feed, "  "),
            Err(Error::NoTitle)
        ));
        assert!(matches!(app.create_folder(&id, ""), Err(Error::NoTitle)));
        assert_eq!(app.library(&id).unwrap().feeds[0].title, "A");
    }

    #[test]
    fn a_sync_accounts_feeds_are_left_to_its_server() {
        let (_tmp, dirs) = dirs();
        std::fs::create_dir_all(&dirs.config).unwrap();
        let raw = br#"{ "accounts": [{ "id": "1", "description": "S", "kind": "greader",
                                       "server": "https://rss.example.com", "username": "u" }] }"#;
        std::fs::write(dirs.config.join("config.json"), raw).unwrap();
        let app = App::load(&dirs).unwrap();

        assert!(matches!(
            app.create_folder("1", "Tech"),
            Err(Error::NotLocal)
        ));
        assert!(matches!(
            app.unsubscribe("1", "feed/1"),
            Err(Error::NotLocal)
        ));
        assert!(matches!(
            app.unsubscribe("9", "feed/1"),
            Err(Error::NoAccount)
        ));
    }
}
