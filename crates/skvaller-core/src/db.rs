//! The local SQLite cache. The UI reads only from here, and local read and star changes
//! queue here until the next sync.

use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, OptionalExtension, params};

use crate::{Error, Result, api};

const SCHEMA_VERSION: i32 = 1;

const SCHEMA: &str = "
CREATE TABLE folders (
    id    TEXT PRIMARY KEY,
    title TEXT NOT NULL
);
CREATE TABLE feeds (
    id         TEXT PRIMARY KEY,
    title      TEXT NOT NULL,
    folder_id  TEXT REFERENCES folders (id),
    -- 0: no longer subscribed, or never listed. Kept only for its starred articles.
    subscribed INTEGER NOT NULL
);
CREATE TABLE articles (
    id        INTEGER PRIMARY KEY,
    feed_id   TEXT NOT NULL REFERENCES feeds (id) ON DELETE CASCADE,
    title     TEXT NOT NULL,
    author    TEXT NOT NULL,
    url       TEXT NOT NULL,
    published INTEGER NOT NULL, -- Unix milliseconds
    read      INTEGER NOT NULL,
    starred   INTEGER NOT NULL,
    html      TEXT NOT NULL     -- untrusted; last, so scans of the other columns skip it
);
CREATE INDEX articles_by_published ON articles (published DESC);
CREATE INDEX articles_by_feed ON articles (feed_id, published DESC);
-- Local changes not yet sent. One row per article and flag, holding the latest value.
CREATE TABLE pending (
    article_id INTEGER NOT NULL REFERENCES articles (id) ON DELETE CASCADE,
    flag       TEXT NOT NULL CHECK (flag IN ('read', 'starred')),
    value      INTEGER NOT NULL,
    PRIMARY KEY (article_id, flag)
);
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value NOT NULL
);
";

/// Newest crawl time seen from the server, in Unix milliseconds.
const NEWEST_CRAWL: &str = "newest_crawl";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Flag {
    Read,
    Starred,
}

impl Flag {
    /// The column in `articles` and the value of `pending.flag`.
    fn column(self) -> &'static str {
        match self {
            Flag::Read => "read",
            Flag::Starred => "starred",
        }
    }

    pub fn tag(self) -> &'static str {
        match self {
            Flag::Read => api::READ,
            Flag::Starred => api::STARRED,
        }
    }
}

pub(crate) struct FeedRow {
    pub id: String,
    pub title: String,
    /// `(id, title)` of the folder, if any.
    pub folder: Option<(String, String)>,
}

pub(crate) struct ArticleRow {
    pub id: i64,
    pub feed_id: String,
    /// Used only if the feed is not cached yet.
    pub feed_title: String,
    pub title: String,
    pub author: String,
    pub url: String,
    pub published: i64,
    pub html: String,
    pub read: bool,
    pub starred: bool,
}

pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        Self::init(conn)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        match version {
            0 => {
                conn.execute_batch(&format!(
                    "BEGIN; {SCHEMA} PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
                ))?;
            }
            SCHEMA_VERSION => {}
            _ => return Err(Error::CacheTooNew),
        }
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// Marks an article read or unread locally, and queues the change for the server.
    pub fn set_read(&self, id: i64, read: bool) -> Result<()> {
        self.set_flag(id, Flag::Read, read)
    }

    /// Stars or unstars an article locally, and queues the change for the server.
    pub fn set_starred(&self, id: i64, starred: bool) -> Result<()> {
        self.set_flag(id, Flag::Starred, starred)
    }

    fn set_flag(&self, id: i64, flag: Flag, value: bool) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let col = flag.column();
        let changed = tx.execute(
            &format!("UPDATE articles SET {col} = ?2 WHERE id = ?1 AND {col} != ?2"),
            params![id, value],
        )?;
        if changed > 0 {
            tx.execute(
                "INSERT INTO pending (article_id, flag, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT DO UPDATE SET value = excluded.value",
                params![id, col, value],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Queued changes, as `(article ID, flag, value)`.
    pub(crate) fn pending(&self) -> Result<Vec<(i64, Flag, bool)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare("SELECT article_id, flag, value FROM pending")?;
        let rows = stmt.query_map([], |r| {
            let flag = if r.get_ref(1)?.as_str()? == "read" {
                Flag::Read
            } else {
                Flag::Starred
            };
            Ok((r.get(0)?, flag, r.get(2)?))
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Drops queued changes the server has accepted. A change the user made again in the
    /// meantime has a different value and stays queued.
    pub(crate) fn clear_pending(&self, flag: Flag, value: bool, ids: &[i64]) -> Result<()> {
        self.lock().execute(
            "DELETE FROM pending WHERE flag = ?1 AND value = ?2
             AND article_id IN (SELECT value FROM json_each(?3))",
            params![flag.column(), value, json(ids)],
        )?;
        Ok(())
    }

    /// Makes the cached feeds and folders match the server. Articles of removed feeds go
    /// too, except starred ones and ones with a queued change.
    pub(crate) fn replace_subscriptions(&self, feeds: &[FeedRow]) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let has_feeds: bool = tx.query_row(
            "SELECT EXISTS (SELECT 1 FROM feeds WHERE subscribed)",
            [],
            |r| r.get(0),
        )?;
        if feeds.is_empty() && has_feeds {
            // More likely a broken server or proxy than a user who unsubscribed from
            // everything. Refuse rather than wipe the cache.
            return Err(Error::Protocol("the server listed no subscriptions".into()));
        }
        {
            let mut folder = tx.prepare(
                "INSERT INTO folders (id, title) VALUES (?1, ?2)
                 ON CONFLICT DO UPDATE SET title = excluded.title",
            )?;
            let mut feed = tx.prepare(
                "INSERT INTO feeds (id, title, folder_id, subscribed) VALUES (?1, ?2, ?3, 1)
                 ON CONFLICT DO UPDATE SET
                     title = excluded.title, folder_id = excluded.folder_id, subscribed = 1",
            )?;
            for f in feeds {
                if let Some((id, title)) = &f.folder {
                    folder.execute(params![id, title])?;
                }
                feed.execute(params![f.id, f.title, f.folder.as_ref().map(|(id, _)| id)])?;
            }
        }
        let ids: Vec<&str> = feeds.iter().map(|f| f.id.as_str()).collect();
        tx.execute(
            "UPDATE feeds SET subscribed = 0, folder_id = NULL
             WHERE subscribed AND id NOT IN (SELECT value FROM json_each(?1))",
            [json(&ids)],
        )?;
        tx.execute(
            "DELETE FROM articles
             WHERE NOT starred
             AND feed_id IN (SELECT id FROM feeds WHERE NOT subscribed)
             AND id NOT IN (SELECT article_id FROM pending)",
            [],
        )?;
        tx.execute(
            "DELETE FROM feeds WHERE NOT subscribed AND id NOT IN (SELECT feed_id FROM articles)",
            [],
        )?;
        tx.execute(
            "DELETE FROM folders WHERE id NOT IN
             (SELECT folder_id FROM feeds WHERE folder_id IS NOT NULL)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Inserts or updates articles from the server, and returns how many rows changed.
    /// A flag with a queued local change keeps its local value. An article of a feed not
    /// cached yet, such as a starred one from an old subscription, gets an unsubscribed
    /// feed entry.
    pub(crate) fn upsert_articles(&self, articles: &[ArticleRow]) -> Result<usize> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut stored = 0;
        {
            let mut feed = tx.prepare(
                "INSERT INTO feeds (id, title, subscribed) VALUES (?1, ?2, 0)
                 ON CONFLICT DO NOTHING",
            )?;
            // The WHERE skips the write, and the WAL growth, when nothing changed.
            let mut article = tx.prepare(
                "INSERT INTO articles
                     (id, feed_id, title, author, url, published, read, starred, html)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
                 ON CONFLICT (id) DO UPDATE SET
                     feed_id = excluded.feed_id,
                     title = excluded.title,
                     author = excluded.author,
                     url = excluded.url,
                     published = excluded.published,
                     html = excluded.html,
                     read = IIF(EXISTS (SELECT 1 FROM pending
                                        WHERE article_id = ?1 AND flag = 'read'),
                                read, excluded.read),
                     starred = IIF(EXISTS (SELECT 1 FROM pending
                                           WHERE article_id = ?1 AND flag = 'starred'),
                                   starred, excluded.starred)
                 WHERE feed_id != excluded.feed_id
                    OR title != excluded.title
                    OR author != excluded.author
                    OR url != excluded.url
                    OR published != excluded.published
                    OR html != excluded.html
                    OR (read != excluded.read AND NOT EXISTS (SELECT 1 FROM pending
                                                              WHERE article_id = ?1
                                                              AND flag = 'read'))
                    OR (starred != excluded.starred AND NOT EXISTS (SELECT 1 FROM pending
                                                                    WHERE article_id = ?1
                                                                    AND flag = 'starred'))",
            )?;
            for a in articles {
                feed.execute(params![a.feed_id, a.feed_title])?;
                stored += article.execute(params![
                    a.id,
                    a.feed_id,
                    a.title,
                    a.author,
                    a.url,
                    a.published,
                    a.read,
                    a.starred,
                    a.html,
                ])?;
            }
        }
        tx.commit()?;
        Ok(stored)
    }

    /// The IDs in `ids` that are not cached yet.
    pub(crate) fn missing_ids(&self, ids: &[i64]) -> Result<Vec<i64>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT DISTINCT value FROM json_each(?1)
             WHERE value NOT IN (SELECT id FROM articles)",
        )?;
        let rows = stmt.query_map([json(ids)], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Sets `flag` on every cached article from the server's full list: for
    /// [`Flag::Read`], `ids` are the unread articles; for [`Flag::Starred`], the starred
    /// ones. Articles with a queued change for `flag` are left alone.
    pub(crate) fn reconcile(&self, flag: Flag, ids: &[i64]) -> Result<usize> {
        let col = flag.column();
        let target = match flag {
            Flag::Read => "id NOT IN (SELECT value FROM json_each(?1))",
            Flag::Starred => "id IN (SELECT value FROM json_each(?1))",
        };
        let changed = self.lock().execute(
            &format!(
                "UPDATE articles SET {col} = ({target})
                 WHERE {col} != ({target})
                 AND NOT EXISTS (SELECT 1 FROM pending
                                 WHERE article_id = articles.id AND flag = '{col}')"
            ),
            [json(ids)],
        )?;
        Ok(changed)
    }

    /// Removes read, unstarred articles published before `cutoff` (Unix milliseconds),
    /// unless a change to them is still queued.
    pub(crate) fn prune(&self, cutoff: i64) -> Result<usize> {
        Ok(self.lock().execute(
            "DELETE FROM articles
             WHERE read AND NOT starred AND published < ?1
             AND id NOT IN (SELECT article_id FROM pending)",
            [cutoff],
        )?)
    }

    pub(crate) fn newest_crawl(&self) -> Result<Option<i64>> {
        Ok(self
            .lock()
            .query_row(
                "SELECT value FROM meta WHERE key = ?1",
                [NEWEST_CRAWL],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub(crate) fn bump_newest_crawl(&self, ms: i64) -> Result<()> {
        self.lock().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT DO UPDATE SET value = max(value, excluded.value)",
            params![NEWEST_CRAWL, ms],
        )?;
        Ok(())
    }

    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[cfg(test)]
    pub(crate) fn with_conn<T>(&self, f: impl FnOnce(&Connection) -> T) -> T {
        f(&self.lock())
    }
}

fn json<T: serde::Serialize>(values: &[T]) -> String {
    serde_json::to_string(values).expect("a slice of IDs serializes")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(id: &str, folder: Option<&str>) -> FeedRow {
        FeedRow {
            id: id.into(),
            title: id.to_uppercase(),
            folder: folder.map(|f| (f.into(), f.to_uppercase())),
        }
    }

    fn article(id: i64, feed_id: &str, read: bool, starred: bool) -> ArticleRow {
        ArticleRow {
            id,
            feed_id: feed_id.into(),
            feed_title: "Old feed".into(),
            title: format!("Article {id}"),
            author: String::new(),
            url: String::new(),
            published: id * 1000,
            html: String::new(),
            read,
            starred,
        }
    }

    fn flags(db: &Db, id: i64) -> (bool, bool) {
        db.with_conn(|c| {
            c.query_row(
                "SELECT read, starred FROM articles WHERE id = ?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap()
        })
    }

    fn db_with_article() -> Db {
        let db = Db::open_in_memory().unwrap();
        db.replace_subscriptions(&[feed("feed/1", None)]).unwrap();
        db.upsert_articles(&[article(1, "feed/1", false, false)])
            .unwrap();
        db
    }

    #[test]
    fn reopening_keeps_the_schema() {
        let path = std::env::temp_dir().join(format!("skvaller-test-{}.db", std::process::id()));
        Db::open(&path).unwrap();
        Db::open(&path).unwrap();
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn local_change_is_queued() {
        let db = db_with_article();
        db.set_read(1, true).unwrap();
        db.set_starred(1, true).unwrap();
        assert_eq!(flags(&db, 1), (true, true));
        let mut pending = db.pending().unwrap();
        pending.sort_by_key(|p| p.1.column());
        assert_eq!(pending, [(1, Flag::Read, true), (1, Flag::Starred, true)]);
    }

    #[test]
    fn setting_the_current_value_queues_nothing() {
        let db = db_with_article();
        db.set_read(1, false).unwrap();
        assert!(db.pending().unwrap().is_empty());
    }

    #[test]
    fn server_update_does_not_overwrite_a_queued_change() {
        let db = db_with_article();
        db.set_read(1, true).unwrap();
        db.upsert_articles(&[article(1, "feed/1", false, true)])
            .unwrap();
        // Read keeps the queued local value; starred has no queued change and follows the server.
        assert_eq!(flags(&db, 1), (true, true));
    }

    #[test]
    fn reconcile_does_not_overwrite_a_queued_change() {
        let db = db_with_article();
        db.upsert_articles(&[article(2, "feed/1", false, false)])
            .unwrap();
        db.set_read(1, true).unwrap();
        // The server still lists both as unread, and article 2 as starred.
        db.reconcile(Flag::Read, &[1, 2]).unwrap();
        db.reconcile(Flag::Starred, &[2]).unwrap();
        assert_eq!(flags(&db, 1), (true, false));
        assert_eq!(flags(&db, 2), (false, true));
        // Neither is unread on the server any more.
        db.reconcile(Flag::Read, &[]).unwrap();
        assert_eq!(flags(&db, 2), (true, true));
    }

    #[test]
    fn clear_pending_keeps_a_change_made_since() {
        let db = db_with_article();
        db.set_read(1, true).unwrap();
        db.set_read(1, false).unwrap();
        db.clear_pending(Flag::Read, true, &[1]).unwrap();
        assert_eq!(db.pending().unwrap(), [(1, Flag::Read, false)]);
        db.clear_pending(Flag::Read, false, &[1]).unwrap();
        assert!(db.pending().unwrap().is_empty());
    }

    fn counts(db: &Db) -> (i64, i64, i64, i64) {
        db.with_conn(|c| {
            c.query_row(
                "SELECT (SELECT count(*) FROM folders), (SELECT count(*) FROM feeds),
                        (SELECT count(*) FROM articles), (SELECT count(*) FROM pending)",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap()
        })
    }

    #[test]
    fn removed_feed_takes_its_articles_and_empty_folder() {
        let db = Db::open_in_memory().unwrap();
        db.replace_subscriptions(&[feed("feed/1", Some("tech")), feed("feed/2", None)])
            .unwrap();
        db.upsert_articles(&[article(1, "feed/1", false, false)])
            .unwrap();
        db.replace_subscriptions(&[feed("feed/2", None)]).unwrap();
        assert_eq!(counts(&db), (0, 1, 0, 0));
    }

    #[test]
    fn removed_feed_keeps_starred_and_queued_articles_until_sent() {
        let db = Db::open_in_memory().unwrap();
        db.replace_subscriptions(&[feed("feed/1", Some("tech")), feed("feed/2", None)])
            .unwrap();
        db.upsert_articles(&[
            article(1, "feed/1", false, true),
            article(2, "feed/1", false, false),
        ])
        .unwrap();
        db.set_read(2, true).unwrap();
        db.replace_subscriptions(&[feed("feed/2", None)]).unwrap();
        assert_eq!(counts(&db), (0, 2, 2, 1));
        // Once the change is sent, the unstarred article goes.
        db.clear_pending(Flag::Read, true, &[2]).unwrap();
        db.replace_subscriptions(&[feed("feed/2", None)]).unwrap();
        assert_eq!(counts(&db), (0, 2, 1, 0));
    }

    #[test]
    fn article_of_unknown_feed_gets_an_unsubscribed_feed() {
        let db = db_with_article();
        assert_eq!(
            db.upsert_articles(&[article(2, "feed/9", false, true)])
                .unwrap(),
            1
        );
        assert_eq!(db.missing_ids(&[1, 2, 2, 3]).unwrap(), [3]);
        let old_feed = db.with_conn(|c| {
            c.query_row(
                "SELECT title, subscribed FROM feeds WHERE id = 'feed/9'",
                [],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)),
            )
            .unwrap()
        });
        assert_eq!(old_feed, ("Old feed".into(), false));
        // A later subscription to it makes it a normal feed.
        db.replace_subscriptions(&[feed("feed/1", None), feed("feed/9", None)])
            .unwrap();
        assert_eq!(counts(&db), (0, 2, 2, 0));
    }

    #[test]
    fn unchanged_article_is_not_rewritten() {
        let db = db_with_article();
        assert_eq!(
            db.upsert_articles(&[article(1, "feed/1", false, false)])
                .unwrap(),
            0
        );
        assert_eq!(
            db.upsert_articles(&[article(1, "feed/1", true, false)])
                .unwrap(),
            1
        );
        // A server change to a flag with a queued change is not a change.
        db.set_starred(1, true).unwrap();
        assert_eq!(
            db.upsert_articles(&[article(1, "feed/1", true, false)])
                .unwrap(),
            0
        );
    }

    #[test]
    fn empty_subscription_list_does_not_wipe_the_cache() {
        let db = db_with_article();
        assert!(db.replace_subscriptions(&[]).is_err());
        assert_eq!(counts(&db), (0, 1, 1, 0));
        // A new account with no feeds is fine.
        Db::open_in_memory()
            .unwrap()
            .replace_subscriptions(&[])
            .unwrap();
    }

    #[test]
    fn prune_keeps_unread_starred_recent_and_queued_articles() {
        let db = db_with_article(); // 1: unread, published at 1 s
        db.upsert_articles(&[
            article(2, "feed/1", true, true),   // starred
            article(3, "feed/1", true, false),  // old and read: goes
            article(4, "feed/1", true, false),  // queued change
            article(50, "feed/1", true, false), // recent
        ])
        .unwrap();
        db.set_read(4, false).unwrap();
        db.set_read(4, true).unwrap();
        assert_eq!(db.prune(10_000).unwrap(), 1);
        assert_eq!(db.missing_ids(&[1, 2, 3, 4, 50]).unwrap(), [3]);
    }

    #[test]
    fn newest_crawl_only_moves_forward() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.newest_crawl().unwrap(), None);
        db.bump_newest_crawl(20).unwrap();
        db.bump_newest_crawl(10).unwrap();
        assert_eq!(db.newest_crawl().unwrap(), Some(20));
    }
}
