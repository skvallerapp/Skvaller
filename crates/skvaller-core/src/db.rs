//! The local SQLite cache. The UI reads only from here, and local read and star changes
//! queue here until the next sync.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Mutex, MutexGuard, PoisonError};

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use crate::full_text::MAX_ATTEMPTS;
use crate::{Error, Result, api};

const SCHEMA_VERSION: i32 = 4;

/// Schema version 2. A new cache gets it plus the migrations after it, so each table
/// is written down once.
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
    subscribed INTEGER NOT NULL,
    icon_url   TEXT, -- the favicon URL the server lists
    icon_from  TEXT, -- the URL `icon` was fetched from; NULL until a fetch finishes
    icon       TEXT  -- a data: URL; NULL if there is none or the fetch failed
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
    summary   TEXT NOT NULL,    -- plain text, for the article list
    html      TEXT NOT NULL     -- untrusted; last, so scans of the other columns skip it
);
CREATE INDEX articles_by_published ON articles (published DESC);
CREATE INDEX articles_by_feed ON articles (feed_id, published DESC);
CREATE INDEX articles_unread ON articles (published DESC) WHERE NOT read;
CREATE INDEX articles_starred ON articles (published DESC) WHERE starred;
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

/// Schema version 1 to 2: feed icons.
const MIGRATE_1: &str = "
ALTER TABLE feeds ADD COLUMN icon_url TEXT;
ALTER TABLE feeds ADD COLUMN icon_from TEXT;
ALTER TABLE feeds ADD COLUMN icon TEXT;
";

/// Schema version 2 to 3: Reader View.
const MIGRATE_2: &str = "
-- 1: open the feed's articles in Reader View. Local, not synced.
ALTER TABLE feeds ADD COLUMN reader_view INTEGER NOT NULL DEFAULT 0;
-- Reader View: the article's full text, extracted from its web page.
CREATE TABLE full_text (
    article_id INTEGER PRIMARY KEY REFERENCES articles (id) ON DELETE CASCADE,
    html       TEXT,             -- untrusted; NULL until extracted
    attempts   INTEGER NOT NULL  -- failed fetches; at full_text::MAX_ATTEMPTS, given up
);
-- A new URL is a new page.
CREATE TRIGGER full_text_follows_url AFTER UPDATE OF url ON articles
WHEN old.url IS NOT new.url
BEGIN
    DELETE FROM full_text WHERE article_id = new.id;
END;
";

/// Schema version 3 to 4: the local account, whose feeds Skvaller fetches itself.
const MIGRATE_3: &str = "
-- The feed's address and its web site's, as the server lists them or, in the local
-- account, where it is fetched from.
ALTER TABLE feeds ADD COLUMN url TEXT;
ALTER TABLE feeds ADD COLUMN site_url TEXT;
-- The local account's: how fetching went last time.
ALTER TABLE feeds ADD COLUMN etag TEXT;
ALTER TABLE feeds ADD COLUMN last_modified TEXT;
ALTER TABLE feeds ADD COLUMN fetch_error TEXT; -- why the last fetch failed
-- The local account's entries the feed still lists. One pruned from `articles` stays
-- here, so the next fetch doesn't bring it back as new.
CREATE TABLE seen (
    article_id INTEGER PRIMARY KEY,
    feed_id    TEXT NOT NULL REFERENCES feeds (id) ON DELETE CASCADE
);
";

/// Newest crawl time seen from the server, in Unix milliseconds.
const NEWEST_CRAWL: &str = "newest_crawl";
/// When the last sync finished, in Unix milliseconds.
const LAST_SYNC: &str = "last_sync";

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

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum FullText {
    Html(String),
    /// Fetching it failed this many times.
    Failed(u32),
}

pub(crate) struct FeedRow {
    pub id: String,
    pub title: String,
    /// `(id, title)` of the folder, if any.
    pub folder: Option<(String, String)>,
    pub icon_url: Option<String>,
    /// The feed's address and its web site's, if the server lists them.
    pub url: Option<String>,
    pub site_url: Option<String>,
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
    pub read: bool,
    pub starred: bool,
    pub summary: String,
    pub html: String,
}

#[derive(Debug, Serialize)]
pub struct Folder {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Feed {
    pub id: String,
    pub title: String,
    pub folder_id: Option<String>,
    /// False for a feed kept only for the starred articles of an old subscription.
    pub subscribed: bool,
    /// The favicon as a `data:` URL.
    pub icon: Option<String>,
    /// Open its articles in Reader View. A local setting, not synced.
    pub reader_view: bool,
    /// The feed's address and its web site's: always for the local account's feeds, as
    /// the server lists them for a sync account's.
    pub url: Option<String>,
    pub site_url: Option<String>,
    /// The local account's feeds: why the last fetch failed.
    pub fetch_error: Option<String>,
}

/// A local account's feed, as fetching it needs.
pub(crate) struct LocalFeed {
    pub id: String,
    pub url: String,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

/// A local account's feed to add.
pub(crate) struct NewFeed<'a> {
    pub id: &'a str,
    pub url: &'a str,
    pub title: &'a str,
    pub site_url: Option<&'a str>,
    pub folder_id: Option<&'a str>,
    pub icon_url: Option<&'a str>,
}

/// An article without its HTML, for the article list.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArticleSummary {
    /// The item ID as a decimal string: it can exceed what a JavaScript number holds.
    pub id: String,
    pub feed_id: String,
    pub title: String,
    pub author: String,
    pub url: String,
    /// Unix milliseconds.
    pub published: i64,
    pub summary: String,
    pub read: bool,
    pub starred: bool,
}

/// Which articles to list: those in `view` from `scope`. Serialized like the UI's
/// `Selection` type.
#[derive(Debug, Deserialize)]
pub struct Selection {
    pub view: View,
    pub scope: Scope,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum View {
    Unread,
    Starred,
    All,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Scope {
    All,
    Folder { id: String },
    Feed { id: String },
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Counts {
    /// Unread articles per feed ID. Feeds without any are left out.
    pub unread_by_feed: HashMap<String, i64>,
    /// Starred articles per feed ID. Feeds without any are left out.
    pub starred_by_feed: HashMap<String, i64>,
}

pub struct Db {
    conn: Mutex<Connection>,
    /// A second connection for the UI's reads, so they don't wait behind a sync's
    /// writes. WAL lets it read while the other one writes. In-memory caches have none.
    reader: Option<Mutex<Connection>>,
}

impl Db {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        let mut db = Self::init(conn)?;
        let reader = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        db.reader = Some(Mutex::new(reader));
        Ok(db)
    }

    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.pragma_update(None, "foreign_keys", true)?;
        let version: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        let steps = match version {
            0 => format!("{SCHEMA} {MIGRATE_2} {MIGRATE_3}"),
            1 => format!("{MIGRATE_1} {MIGRATE_2} {MIGRATE_3}"),
            2 => format!("{MIGRATE_2} {MIGRATE_3}"),
            3 => MIGRATE_3.to_owned(),
            SCHEMA_VERSION => String::new(),
            _ => return Err(Error::CacheTooNew),
        };
        if !steps.is_empty() {
            conn.execute_batch(&format!(
                "BEGIN; {steps} PRAGMA user_version = {SCHEMA_VERSION}; COMMIT;"
            ))?;
        }
        Ok(Self {
            conn: Mutex::new(conn),
            reader: None,
        })
    }

    pub fn folders(&self) -> Result<Vec<Folder>> {
        let conn = self.read();
        let mut stmt =
            conn.prepare("SELECT id, title FROM folders ORDER BY title COLLATE NOCASE")?;
        let rows = stmt.query_map([], |r| {
            Ok(Folder {
                id: r.get(0)?,
                title: r.get(1)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn feeds(&self) -> Result<Vec<Feed>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT id, title, folder_id, subscribed, icon, reader_view, url, site_url, fetch_error
             FROM feeds ORDER BY title COLLATE NOCASE",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(Feed {
                id: r.get(0)?,
                title: r.get(1)?,
                folder_id: r.get(2)?,
                subscribed: r.get(3)?,
                icon: r.get(4)?,
                reader_view: r.get(5)?,
                url: r.get(6)?,
                site_url: r.get(7)?,
                fetch_error: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Articles for `selection`, newest first.
    pub fn articles(&self, selection: &Selection) -> Result<Vec<ArticleSummary>> {
        let view = match selection.view {
            View::Unread => "NOT read",
            View::Starred => "starred",
            View::All => "1",
        };
        let (scope, param) = match &selection.scope {
            Scope::All => ("1", None),
            Scope::Feed { id } => ("feed_id = ?1", Some(id)),
            Scope::Folder { id } => (
                "feed_id IN (SELECT id FROM feeds WHERE folder_id = ?1)",
                Some(id),
            ),
        };
        let conn = self.read();
        let mut stmt = conn.prepare(&format!(
            "SELECT id, feed_id, title, author, url, published, summary, read, starred
             FROM articles WHERE {view} AND {scope} ORDER BY published DESC, id DESC"
        ))?;
        let params: Vec<&String> = param.into_iter().collect();
        let rows = stmt.query_map(rusqlite::params_from_iter(params), |r| {
            Ok(ArticleSummary {
                id: r.get::<_, i64>(0)?.to_string(),
                feed_id: r.get(1)?,
                title: r.get(2)?,
                author: r.get(3)?,
                url: r.get(4)?,
                published: r.get(5)?,
                summary: r.get(6)?,
                read: r.get(7)?,
                starred: r.get(8)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// The article's untrusted HTML, if it is cached.
    pub fn article_html(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .read()
            .query_row("SELECT html FROM articles WHERE id = ?1", [id], |r| {
                r.get(0)
            })
            .optional()?)
    }

    /// The article's web page URL, if the article is cached.
    pub(crate) fn article_url(&self, id: i64) -> Result<Option<String>> {
        Ok(self
            .read()
            .query_row("SELECT url FROM articles WHERE id = ?1", [id], |r| r.get(0))
            .optional()?)
    }

    /// The article's Reader View text, or how often fetching it failed. `None` if it
    /// was never tried.
    pub(crate) fn full_text(&self, id: i64) -> Result<Option<FullText>> {
        Ok(self
            .read()
            .query_row(
                "SELECT html, attempts FROM full_text WHERE article_id = ?1",
                [id],
                |r| {
                    Ok(match r.get::<_, Option<String>>(0)? {
                        Some(html) => FullText::Html(html),
                        None => FullText::Failed(r.get(1)?),
                    })
                },
            )
            .optional()?)
    }

    /// Does nothing if the article left the cache meanwhile.
    pub(crate) fn set_full_text(&self, id: i64, html: &str) -> Result<()> {
        self.lock().execute(
            "INSERT INTO full_text (article_id, html, attempts)
             SELECT id, ?2, 0 FROM articles WHERE id = ?1
             ON CONFLICT DO UPDATE SET html = excluded.html, attempts = 0",
            params![id, html],
        )?;
        Ok(())
    }

    /// Counts a failed fetch. A `permanent` failure, such as a page with nothing
    /// readable, gives up at once.
    pub(crate) fn full_text_failed(&self, id: i64, permanent: bool) -> Result<()> {
        let attempts = if permanent { MAX_ATTEMPTS } else { 1 };
        self.lock().execute(
            "INSERT INTO full_text (article_id, html, attempts)
             SELECT id, NULL, ?2 FROM articles WHERE id = ?1
             ON CONFLICT DO UPDATE SET
                 html = NULL, attempts = min(?3, attempts + excluded.attempts)",
            params![id, attempts, MAX_ATTEMPTS],
        )?;
        Ok(())
    }

    pub fn set_feed_reader_view(&self, feed_id: &str, on: bool) -> Result<()> {
        self.lock().execute(
            "UPDATE feeds SET reader_view = ?2 WHERE id = ?1",
            params![feed_id, on],
        )?;
        Ok(())
    }

    pub fn counts(&self) -> Result<Counts> {
        let conn = self.read();
        let by_feed = |filter: &str| -> Result<HashMap<String, i64>> {
            let mut stmt = conn.prepare(&format!(
                "SELECT feed_id, count(*) FROM articles WHERE {filter} GROUP BY feed_id"
            ))?;
            let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
            Ok(rows.collect::<rusqlite::Result<_>>()?)
        };
        Ok(Counts {
            unread_by_feed: by_feed("NOT read")?,
            starred_by_feed: by_feed("starred")?,
        })
    }

    /// Unread articles in the whole cache, for the account switcher.
    pub fn unread_total(&self) -> Result<i64> {
        Ok(self
            .read()
            .query_row("SELECT count(*) FROM articles WHERE NOT read", [], |r| {
                r.get(0)
            })?)
    }

    /// Empties the cache, for a sign-in to a different account. Queued changes go too.
    pub fn clear(&self) -> Result<()> {
        // A transaction that rolls back on error. A failed BEGIN…COMMIT batch would leave
        // the shared connection inside an open transaction.
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute_batch(
            "DELETE FROM pending; DELETE FROM articles; DELETE FROM feeds;
             DELETE FROM folders; DELETE FROM meta;",
        )?;
        tx.commit()?;
        Ok(())
    }

    /// When the last sync finished, in Unix milliseconds.
    pub fn last_sync(&self) -> Result<Option<i64>> {
        self.meta(LAST_SYNC)
    }

    /// Marks articles read or unread locally, and queues the changes for the server.
    pub fn set_read(&self, ids: &[i64], read: bool) -> Result<()> {
        self.set_flag(ids, Flag::Read, read)
    }

    /// Stars or unstars articles locally, and queues the changes for the server.
    pub fn set_starred(&self, ids: &[i64], starred: bool) -> Result<()> {
        self.set_flag(ids, Flag::Starred, starred)
    }

    fn set_flag(&self, ids: &[i64], flag: Flag, value: bool) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        {
            let col = flag.column();
            let mut update = tx.prepare(&format!(
                "UPDATE articles SET {col} = ?2 WHERE id = ?1 AND {col} != ?2"
            ))?;
            let mut queue = tx.prepare(
                "INSERT INTO pending (article_id, flag, value) VALUES (?1, ?2, ?3)
                 ON CONFLICT DO UPDATE SET value = excluded.value",
            )?;
            for &id in ids {
                if update.execute(params![id, value])? > 0 {
                    queue.execute(params![id, col, value])?;
                }
            }
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
                "INSERT INTO feeds (id, title, folder_id, subscribed, icon_url, url, site_url)
                 VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6)
                 ON CONFLICT DO UPDATE SET
                     title = excluded.title, folder_id = excluded.folder_id, subscribed = 1,
                     icon_url = excluded.icon_url, url = excluded.url,
                     site_url = excluded.site_url",
            )?;
            for f in feeds {
                if let Some((id, title)) = &f.folder {
                    folder.execute(params![id, title])?;
                }
                feed.execute(params![
                    f.id,
                    f.title,
                    f.folder.as_ref().map(|(id, _)| id),
                    f.icon_url,
                    f.url,
                    f.site_url
                ])?;
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

    /// Feeds whose icon was not fetched from their current icon URL, as `(id, URL)`.
    pub(crate) fn stale_icons(&self) -> Result<Vec<(String, String)>> {
        let conn = self.lock();
        let mut stmt = conn.prepare(
            "SELECT id, icon_url FROM feeds
             WHERE icon_url IS NOT NULL AND icon_from IS NOT icon_url",
        )?;
        let rows = stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Stores the icon fetched from `from`, or `None` if there was no usable image.
    pub(crate) fn set_icon(&self, feed_id: &str, from: &str, icon: Option<&str>) -> Result<()> {
        self.lock().execute(
            "UPDATE feeds SET icon_from = ?2, icon = ?3 WHERE id = ?1",
            params![feed_id, from, icon],
        )?;
        Ok(())
    }

    /// Unread articles of Reader View feeds whose full text is not cached and not given
    /// up on, newest first.
    pub(crate) fn full_text_wanted(&self, limit: usize) -> Result<Vec<i64>> {
        let conn = self.read();
        let mut stmt = conn.prepare(
            "SELECT a.id FROM articles a JOIN feeds f ON f.id = a.feed_id
             WHERE f.reader_view AND NOT a.read AND a.url != ''
             AND a.id NOT IN (
                 SELECT article_id FROM full_text WHERE html IS NOT NULL OR attempts >= ?2
             )
             ORDER BY a.published DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64, MAX_ATTEMPTS], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
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
                     (id, feed_id, title, author, url, published, read, starred, summary, html)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
                 ON CONFLICT (id) DO UPDATE SET
                     feed_id = excluded.feed_id,
                     title = excluded.title,
                     author = excluded.author,
                     url = excluded.url,
                     published = excluded.published,
                     summary = excluded.summary,
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
                    a.summary,
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

    /// Feeds with an address: in the local account, every one. Only the local account
    /// fetches them.
    pub(crate) fn local_feeds(&self) -> Result<Vec<LocalFeed>> {
        let conn = self.lock();
        let mut stmt =
            conn.prepare("SELECT id, url, etag, last_modified FROM feeds WHERE url IS NOT NULL")?;
        let rows = stmt.query_map([], |r| {
            Ok(LocalFeed {
                id: r.get(0)?,
                url: r.get(1)?,
                etag: r.get(2)?,
                last_modified: r.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Adds a local account's feed. False if it is there already.
    pub(crate) fn add_local_feed(&self, feed: &NewFeed) -> Result<bool> {
        let added = self.lock().execute(
            "INSERT INTO feeds (id, title, folder_id, subscribed, icon_url, url, site_url)
             VALUES (?1, ?2, ?3, 1, ?4, ?5, ?6) ON CONFLICT DO NOTHING",
            params![
                feed.id,
                feed.title,
                feed.folder_id,
                feed.icon_url,
                feed.url,
                feed.site_url
            ],
        )?;
        Ok(added > 0)
    }

    /// Stores what a local account's feed lists now: new entries as unread articles,
    /// changes to cached ones. Read and starred state stay as the user left them, and a
    /// renamed feed keeps its title. An entry without a date keeps the date it got when
    /// first seen (`dated` false). Returns how many rows changed.
    pub(crate) fn store_local_feed(
        &self,
        feed_id: &str,
        entries: &[(ArticleRow, bool)],
        etag: Option<&str>,
        last_modified: Option<&str>,
    ) -> Result<usize> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let mut stored = 0;
        {
            // Not one pruned or deleted already: it is in `seen` but not in `articles`.
            let mut article = tx.prepare(
                "INSERT INTO articles
                     (id, feed_id, title, author, url, published, read, starred, summary, html)
                 SELECT ?1, ?2, ?3, ?4, ?5, ?6, 0, 0, ?7, ?8
                 WHERE NOT EXISTS (SELECT 1 FROM seen WHERE article_id = ?1)
                    OR EXISTS (SELECT 1 FROM articles WHERE id = ?1)
                 ON CONFLICT (id) DO UPDATE SET
                     title = excluded.title,
                     author = excluded.author,
                     url = excluded.url,
                     published = IIF(?9, excluded.published, published),
                     summary = excluded.summary,
                     html = excluded.html
                 WHERE feed_id = excluded.feed_id AND (
                       title != excluded.title
                    OR author != excluded.author
                    OR url != excluded.url
                    OR (?9 AND published != excluded.published)
                    OR html != excluded.html)",
            )?;
            for (a, dated) in entries {
                stored += article.execute(params![
                    a.id,
                    feed_id,
                    a.title,
                    a.author,
                    a.url,
                    a.published,
                    a.summary,
                    a.html,
                    dated,
                ])?;
            }
        }
        let ids: Vec<i64> = entries.iter().map(|(a, _)| a.id).collect();
        tx.execute(
            "DELETE FROM seen WHERE feed_id = ?1 AND article_id NOT IN (SELECT value FROM json_each(?2))",
            params![feed_id, json(&ids)],
        )?;
        tx.execute(
            "INSERT INTO seen (article_id, feed_id) SELECT value, ?1 FROM json_each(?2)
             WHERE true ON CONFLICT DO NOTHING",
            params![feed_id, json(&ids)],
        )?;
        tx.execute(
            "UPDATE feeds SET etag = ?2, last_modified = ?3, fetch_error = NULL WHERE id = ?1",
            params![feed_id, etag, last_modified],
        )?;
        tx.commit()?;
        Ok(stored)
    }

    /// Records why fetching a local account's feed failed, or that it worked (`None`).
    pub(crate) fn set_fetch_error(&self, feed_id: &str, error: Option<&str>) -> Result<()> {
        self.lock().execute(
            "UPDATE feeds SET fetch_error = ?2 WHERE id = ?1",
            params![feed_id, error],
        )?;
        Ok(())
    }

    /// Drops every queued change. The local account has no server to send them to.
    pub(crate) fn clear_all_pending(&self) -> Result<()> {
        self.lock().execute("DELETE FROM pending", [])?;
        Ok(())
    }

    /// Removes a feed with its articles, starred ones too.
    pub fn unsubscribe(&self, feed_id: &str) -> Result<()> {
        self.lock()
            .execute("DELETE FROM feeds WHERE id = ?1", [feed_id])?;
        Ok(())
    }

    pub fn rename_feed(&self, feed_id: &str, title: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE feeds SET title = ?2 WHERE id = ?1",
            params![feed_id, title],
        )?;
        Ok(())
    }

    /// Moves a feed into a folder, or out of any with `None`.
    pub fn move_feed(&self, feed_id: &str, folder_id: Option<&str>) -> Result<()> {
        self.lock().execute(
            "UPDATE feeds SET folder_id = ?2 WHERE id = ?1",
            params![feed_id, folder_id],
        )?;
        Ok(())
    }

    /// The folder with this title, ignoring case, or else a new one. Returns its ID.
    /// IDs are `folder/<n>` and never change, so a rename keeps what refers to them.
    pub fn create_folder(&self, title: &str) -> Result<String> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        let existing: Option<String> = tx
            .query_row(
                "SELECT id FROM folders WHERE title = ?1 COLLATE NOCASE",
                [title],
                |r| r.get(0),
            )
            .optional()?;
        let id = match existing {
            Some(id) => id,
            None => {
                let n: i64 = tx.query_row(
                    "SELECT coalesce(max(CAST(substr(id, 8) AS INTEGER)), 0) + 1 FROM folders
                     WHERE id LIKE 'folder/%'",
                    [],
                    |r| r.get(0),
                )?;
                let id = format!("folder/{n}");
                tx.execute(
                    "INSERT INTO folders (id, title) VALUES (?1, ?2)",
                    params![id, title],
                )?;
                id
            }
        };
        tx.commit()?;
        Ok(id)
    }

    pub fn rename_folder(&self, folder_id: &str, title: &str) -> Result<()> {
        self.lock().execute(
            "UPDATE folders SET title = ?2 WHERE id = ?1",
            params![folder_id, title],
        )?;
        Ok(())
    }

    /// Removes a folder. Its feeds stay, in no folder.
    pub fn delete_folder(&self, folder_id: &str) -> Result<()> {
        let mut conn = self.lock();
        let tx = conn.transaction()?;
        tx.execute(
            "UPDATE feeds SET folder_id = NULL WHERE folder_id = ?1",
            [folder_id],
        )?;
        tx.execute("DELETE FROM folders WHERE id = ?1", [folder_id])?;
        tx.commit()?;
        Ok(())
    }

    pub(crate) fn newest_crawl(&self) -> Result<Option<i64>> {
        self.meta(NEWEST_CRAWL)
    }

    pub(crate) fn set_last_sync(&self, ms: i64) -> Result<()> {
        self.lock().execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)
             ON CONFLICT DO UPDATE SET value = excluded.value",
            params![LAST_SYNC, ms],
        )?;
        Ok(())
    }

    fn meta(&self, key: &str) -> Result<Option<i64>> {
        Ok(self
            .read()
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
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

    /// The connection for writes, and for reads that must see the sync's own writes.
    fn lock(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The connection for the UI's reads.
    fn read(&self) -> MutexGuard<'_, Connection> {
        match &self.reader {
            Some(reader) => reader.lock().unwrap_or_else(PoisonError::into_inner),
            None => self.lock(),
        }
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
            icon_url: None,
            url: None,
            site_url: None,
        }
    }

    #[test]
    fn version_1_cache_gets_icon_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "{SCHEMA}
             ALTER TABLE feeds DROP COLUMN icon_url;
             ALTER TABLE feeds DROP COLUMN icon_from;
             ALTER TABLE feeds DROP COLUMN icon;
             INSERT INTO feeds (id, title, subscribed) VALUES ('feed/1', 'A', 1);
             PRAGMA user_version = 1;"
        ))
        .unwrap();

        let db = Db::init(conn).unwrap();

        assert_eq!(db.feeds().unwrap()[0].icon, None);
        let version: i32 = db.with_conn(|c| {
            c.pragma_query_value(None, "user_version", |r| r.get(0))
                .unwrap()
        });
        assert_eq!(version, SCHEMA_VERSION);
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
            summary: String::new(),
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
        db.set_read(&[1], true).unwrap();
        db.set_starred(&[1], true).unwrap();
        assert_eq!(flags(&db, 1), (true, true));
        let mut pending = db.pending().unwrap();
        pending.sort_by_key(|p| p.1.column());
        assert_eq!(pending, [(1, Flag::Read, true), (1, Flag::Starred, true)]);
    }

    #[test]
    fn marking_many_queues_only_the_changed_ones() {
        let db = db_with_article();
        db.upsert_articles(&[
            article(2, "feed/1", true, false),
            article(3, "feed/1", false, false),
        ])
        .unwrap();
        db.set_read(&[1, 2, 3, 9], true).unwrap();
        let mut pending = db.pending().unwrap();
        pending.sort_by_key(|p| p.0);
        assert_eq!(pending, [(1, Flag::Read, true), (3, Flag::Read, true)]);
    }

    #[test]
    fn setting_the_current_value_queues_nothing() {
        let db = db_with_article();
        db.set_read(&[1], false).unwrap();
        assert!(db.pending().unwrap().is_empty());
    }

    #[test]
    fn server_update_does_not_overwrite_a_queued_change() {
        let db = db_with_article();
        db.set_read(&[1], true).unwrap();
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
        db.set_read(&[1], true).unwrap();
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
        db.set_read(&[1], true).unwrap();
        db.set_read(&[1], false).unwrap();
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
        db.set_read(&[2], true).unwrap();
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
        db.set_starred(&[1], true).unwrap();
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
        db.set_read(&[4], false).unwrap();
        db.set_read(&[4], true).unwrap();
        assert_eq!(db.prune(10_000).unwrap(), 1);
        assert_eq!(db.missing_ids(&[1, 2, 3, 4, 50]).unwrap(), [3]);
    }

    #[test]
    fn version_2_cache_gets_reader_view() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "{SCHEMA}
             INSERT INTO feeds (id, title, subscribed) VALUES ('feed/1', 'A', 1);
             PRAGMA user_version = 2;"
        ))
        .unwrap();

        let db = Db::init(conn).unwrap();

        assert!(!db.feeds().unwrap()[0].reader_view);
        db.upsert_articles(&[article(1, "feed/1", false, false)])
            .unwrap();
        db.set_full_text(1, "<p>x</p>").unwrap();
        assert_eq!(
            db.full_text(1).unwrap(),
            Some(FullText::Html("<p>x</p>".into()))
        );
    }

    #[test]
    fn full_text_goes_with_its_article_and_its_url() {
        let db = db_with_article(); // 1: unread, published at 1 s
        let at = |url: &str| ArticleRow {
            url: url.into(),
            ..article(3, "feed/1", true, false)
        };
        db.upsert_articles(&[at("https://example.com/a")]).unwrap();
        assert_eq!(db.full_text(3).unwrap(), None);
        db.full_text_failed(3, false).unwrap();
        db.full_text_failed(3, false).unwrap();
        assert_eq!(db.full_text(3).unwrap(), Some(FullText::Failed(2)));
        db.full_text_failed(3, true).unwrap();
        db.full_text_failed(3, false).unwrap();
        assert_eq!(
            db.full_text(3).unwrap(),
            Some(FullText::Failed(MAX_ATTEMPTS))
        );
        db.set_full_text(3, "<p>full</p>").unwrap();
        assert_eq!(
            db.full_text(3).unwrap(),
            Some(FullText::Html("<p>full</p>".into()))
        );

        // Another update keeps it; a new URL drops it.
        db.upsert_articles(&[ArticleRow {
            title: "New".into(),
            ..at("https://example.com/a")
        }])
        .unwrap();
        assert!(db.full_text(3).unwrap().is_some());
        db.upsert_articles(&[at("https://example.com/b")]).unwrap();
        assert_eq!(db.full_text(3).unwrap(), None);

        db.set_full_text(3, "<p>full</p>").unwrap();
        db.prune(10_000).unwrap();
        assert_eq!(db.full_text(3).unwrap(), None);
        // An article that left the cache gets no row.
        db.set_full_text(3, "<p>full</p>").unwrap();
        db.full_text_failed(3, true).unwrap();
        assert_eq!(db.full_text(3).unwrap(), None);
    }

    #[test]
    fn reader_view_feeds_want_their_unread_articles_fetched() {
        let db = Db::open_in_memory().unwrap();
        db.replace_subscriptions(&[feed("feed/1", None), feed("feed/2", None)])
            .unwrap();
        let with_url = |id, feed_id: &str, read| ArticleRow {
            url: format!("https://example.com/{id}"),
            ..article(id, feed_id, read, false)
        };
        db.upsert_articles(&[
            with_url(1, "feed/1", false),
            with_url(2, "feed/1", false),       // fetched
            with_url(3, "feed/1", true),        // read
            with_url(4, "feed/2", false),       // feed not in Reader View
            article(5, "feed/1", false, false), // no URL
            with_url(6, "feed/1", false),       // failed once: tried again
            with_url(7, "feed/1", false),       // given up on
        ])
        .unwrap();
        db.set_full_text(2, "<p>x</p>").unwrap();
        db.full_text_failed(6, false).unwrap();
        db.full_text_failed(7, true).unwrap();
        assert!(db.full_text_wanted(10).unwrap().is_empty());

        db.set_feed_reader_view("feed/1", true).unwrap();
        // A sync keeps the local setting.
        db.replace_subscriptions(&[feed("feed/1", None), feed("feed/2", None)])
            .unwrap();
        assert!(db.feeds().unwrap()[0].reader_view);

        assert_eq!(db.full_text_wanted(10).unwrap(), [6, 1]);
        assert_eq!(db.full_text_wanted(1).unwrap(), [6]);
    }

    fn ids(db: &Db, selection: Selection) -> Vec<String> {
        db.articles(&selection)
            .unwrap()
            .into_iter()
            .map(|a| a.id)
            .collect()
    }

    #[test]
    fn lists_articles_and_counts_through_the_read_connection() {
        let path = std::env::temp_dir().join(format!("skvaller-read-{}.db", std::process::id()));
        let db = Db::open(&path).unwrap();
        db.replace_subscriptions(&[feed("feed/1", Some("tech")), feed("feed/2", None)])
            .unwrap();
        db.upsert_articles(&[
            article(1, "feed/1", false, false),
            article(2, "feed/2", true, true),
            article(3, "feed/2", false, false),
        ])
        .unwrap();
        db.set_read(&[3], true).unwrap();

        let all = |view| Selection {
            view,
            scope: Scope::All,
        };
        let feed2 = |view| Selection {
            view,
            scope: Scope::Feed {
                id: "feed/2".into(),
            },
        };
        let tech = |view| Selection {
            view,
            scope: Scope::Folder { id: "tech".into() },
        };
        assert_eq!(ids(&db, all(View::Unread)), ["1"]);
        assert_eq!(ids(&db, all(View::Starred)), ["2"]);
        assert_eq!(ids(&db, all(View::All)), ["3", "2", "1"]);
        assert_eq!(ids(&db, feed2(View::All)), ["3", "2"]);
        assert_eq!(ids(&db, feed2(View::Starred)), ["2"]);
        assert!(ids(&db, feed2(View::Unread)).is_empty());
        assert_eq!(ids(&db, tech(View::All)), ["1"]);
        assert_eq!(ids(&db, tech(View::Unread)), ["1"]);
        assert!(ids(&db, tech(View::Starred)).is_empty());
        let counts = db.counts().unwrap();
        assert_eq!(counts.unread_by_feed.get("feed/1"), Some(&1));
        assert_eq!(counts.unread_by_feed.get("feed/2"), None);
        assert_eq!(counts.starred_by_feed.get("feed/1"), None);
        assert_eq!(counts.starred_by_feed.get("feed/2"), Some(&1));
        assert_eq!(db.article_html(1).unwrap(), Some(String::new()));
        assert_eq!(db.article_html(9).unwrap(), None);
        assert_eq!(db.folders().unwrap().len(), 1);
        assert_eq!(db.feeds().unwrap().len(), 2);

        drop(db);
        for suffix in ["", "-wal", "-shm"] {
            let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
        }
    }

    #[test]
    fn clear_empties_everything() {
        let db = db_with_article();
        db.set_read(&[1], true).unwrap();
        db.bump_newest_crawl(5).unwrap();
        db.clear().unwrap();
        assert_eq!(counts(&db), (0, 0, 0, 0));
        assert_eq!(db.newest_crawl().unwrap(), None);
    }

    #[test]
    fn selection_parses_the_ui_shape() {
        let sel: Selection =
            serde_json::from_str(r#"{"view":"starred","scope":{"kind":"all"}}"#).unwrap();
        assert!(matches!(
            sel,
            Selection {
                view: View::Starred,
                scope: Scope::All
            }
        ));
        let sel: Selection =
            serde_json::from_str(r#"{"view":"unread","scope":{"kind":"feed","id":"feed/1"}}"#)
                .unwrap();
        assert!(matches!(
            sel,
            Selection { view: View::Unread, scope: Scope::Feed { id } } if id == "feed/1"
        ));
    }

    #[test]
    fn newest_crawl_only_moves_forward() {
        let db = Db::open_in_memory().unwrap();
        assert_eq!(db.newest_crawl().unwrap(), None);
        db.bump_newest_crawl(20).unwrap();
        db.bump_newest_crawl(10).unwrap();
        assert_eq!(db.newest_crawl().unwrap(), Some(20));
    }

    #[test]
    fn version_3_cache_gets_local_feed_columns() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(&format!(
            "{SCHEMA} {MIGRATE_2}
             INSERT INTO feeds (id, title, subscribed) VALUES ('feed/1', 'A', 1);
             PRAGMA user_version = 3;"
        ))
        .unwrap();

        let db = Db::init(conn).unwrap();

        let feed = &db.feeds().unwrap()[0];
        assert_eq!(
            (feed.url.as_deref(), feed.fetch_error.as_deref()),
            (None, None)
        );
        assert!(db.local_feeds().unwrap().is_empty());
        let version: i32 = db.with_conn(|c| {
            c.pragma_query_value(None, "user_version", |r| r.get(0))
                .unwrap()
        });
        assert_eq!(version, SCHEMA_VERSION);
    }
}
