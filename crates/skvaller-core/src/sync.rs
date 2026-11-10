//! Two-way sync between the server and the local cache.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::api::{Item, READ, READING_LIST, STARRED, Subscription, parse_item_id};
use crate::db::{ArticleRow, FeedRow, Flag};
use crate::net::{self, Untrusted};
use crate::{Client, Db, Error, Result, thumbnail};

pub(crate) const DAY: i64 = 24 * 60 * 60;
/// Later syncs list articles from this much before the newest crawl time already seen,
/// so an article the server stored while the last sync ran is not missed.
const CRAWL_OVERLAP: i64 = 60 * 60;
/// How long read, unstarred articles stay cached unless the user changes the setting.
/// The first sync fetches read articles from the same period (FR-5b, FR-5c).
pub const DEFAULT_KEEP_READ_DAYS: u32 = 30;
/// Item IDs per `edit-tag` or `stream/items/contents` request.
const BATCH: usize = 250;
/// Larger icons are skipped. Favicons are a few kilobytes.
const MAX_ICON_BYTES: usize = 100 * 1024;
const ICON_TIMEOUT: Duration = Duration::from_secs(10);
/// Icons fetched at the same time.
const PARALLEL_ICONS: usize = 6;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Queued local changes sent to the server.
    pub pushed: usize,
    /// Articles added.
    pub fetched: usize,
    /// Cached articles whose read or starred state changed on the server.
    pub changed: usize,
    /// Old read articles removed from the cache.
    pub pruned: usize,
    /// Feeds and folders added, changed or removed, and articles removed with their feed.
    pub subscriptions: usize,
}

impl SyncReport {
    /// Whether the cached articles, or the feeds they are in, changed.
    #[must_use]
    pub fn changed_articles(&self) -> bool {
        self.fetched + self.changed + self.pruned + self.subscriptions > 0
    }
}

/// Sends queued local changes, then pulls feeds, folders, articles and read and starred
/// state, then removes read, unstarred articles published more than `keep_read_days`
/// ago. `now` is Unix seconds.
///
/// The first sync fetches every unread and starred article, plus read articles from the
/// last `keep_read_days`. Later syncs fetch what the server got since then, plus any
/// unread or starred article not cached yet. Articles come by ID, and only those not
/// cached yet: Miniflux has no `stream/contents`. A flag with a queued local change is
/// never overwritten.
pub async fn sync(client: &Client, db: &Db, now: i64, keep_read_days: u32) -> Result<SyncReport> {
    // A push the server refuses doesn't hold back the pull, since queued changes are kept
    // and win anyway. Its error is returned once the pull is done.
    let pushed = match push(client, db).await {
        Err(Error::Unauthorized) => return Err(Error::Unauthorized),
        pushed => pushed,
    };
    let keep_read_since = now - i64::from(keep_read_days) * DAY;
    let prune_before = keep_read_since * 1000;

    let subs = client.subscriptions().await?;
    // FreshRSS leaves feeds set to "show in its feed only" out of the reading list, so
    // those are listed one by one.
    let streams: Vec<String> = std::iter::once(READING_LIST.to_owned())
        .chain(
            subs.iter()
                .filter(|s| s.priority.as_deref() == Some("feed"))
                .map(|s| s.id.clone()),
        )
        .collect();
    let subscriptions =
        db.replace_subscriptions(&subs.into_iter().map(feed_row).collect::<Vec<_>>())?;

    let since = match db.newest_crawl()? {
        Some(ms) => ms / 1000 - CRAWL_OVERLAP,
        None => keep_read_since,
    };
    let mut recent = Vec::new();
    let mut unread = Vec::new();
    for stream in &streams {
        recent.extend(client.item_ids(stream, None, Some(since)).await?);
        unread.extend(client.item_ids(stream, Some(READ), None).await?);
    }
    let starred = client.item_ids(STARRED, None, None).await?;
    let wanted: Vec<&String> = recent.iter().chain(&unread).chain(&starred).collect();
    let mut fetched = 0;
    let mut newest_crawl = None;
    for chunk in db.missing_ids(&wanted)?.chunks(BATCH) {
        let (stored, newest) = store(db, client.items_contents(chunk).await?, prune_before)?;
        fetched += stored;
        newest_crawl = newest_crawl.max(newest);
    }
    let changed = db.reconcile(Flag::Read, &unread)? + db.reconcile(Flag::Starred, &starred)?;

    // Only once every page is stored, so an interrupted first sync starts over.
    if let Some(ms) = newest_crawl {
        db.bump_newest_crawl(ms)?;
    }
    // Never unread articles: the server lists them, so the next sync would fetch them
    // again. The server's own retention limits them.
    let pruned = db.prune(prune_before, i64::MIN)?;
    db.optimize()?;
    let server: Vec<_> = client.base_url().into_iter().collect();
    fetch_icons(db, |url| client.resolve(url), &server, None).await?;
    let pushed = pushed?;
    db.set_last_sync(now * 1000)?;
    Ok(SyncReport {
        pushed,
        fetched,
        changed,
        pruned,
        subscriptions,
    })
}

/// Sends queued changes, a chunk at a time. A chunk the server refuses stays queued and
/// the rest still go; the first such error is returned at the end.
async fn push(client: &Client, db: &Db) -> Result<usize> {
    let pending = db.pending()?;
    if pending.is_empty() {
        return Ok(0);
    }
    let token = client.write_token().await?;
    let mut failed = None;
    for flag in [Flag::Read, Flag::Starred] {
        for value in [true, false] {
            let ids: Vec<String> = pending
                .iter()
                .filter(|&&(_, f, v)| f == flag && v == value)
                .map(|(id, ..)| id.clone())
                .collect();
            for chunk in ids.chunks(BATCH) {
                match client.edit_tag(&token, chunk, flag.tag(), value).await {
                    Ok(()) => db.clear_pending(flag, value, chunk)?,
                    Err(Error::Unauthorized) => return Err(Error::Unauthorized),
                    Err(e) => {
                        failed.get_or_insert(e);
                    }
                }
            }
        }
    }
    failed.map_or(Ok(pending.len()), Err)
}

/// Fetches the icons of feeds that are new or whose icon URL changed, `resolve` making
/// each URL absolute; with `only`, just that feed's. A few at a time, since feeds can
/// point at slow hosts. An icon that can't be fetched now is tried again next sync; one
/// that isn't an image is not.
///
/// Only from public addresses, and from the hosts of `trusted`: the sync server, which
/// often serves the icons from the LAN, or the local account's feeds.
pub(crate) async fn fetch_icons(
    db: &Db,
    resolve: impl Fn(&str) -> Option<reqwest::Url>,
    trusted: &[reqwest::Url],
    only: Option<&str>,
) -> Result<()> {
    let stale: Vec<_> = db
        .stale_icons()?
        .into_iter()
        .filter(|(id, _)| only.is_none_or(|only| only == id))
        .collect();
    if stale.is_empty() {
        return Ok(());
    }
    // Not the API client: icons may redirect, and they get no access token.
    let http = Untrusted::new(ICON_TIMEOUT, trusted)?;
    let limit = Arc::new(Semaphore::new(PARALLEL_ICONS));
    let mut fetches = JoinSet::new();
    for (feed_id, url) in stale {
        let Some(resolved) = resolve(&url) else {
            db.set_icon(&feed_id, &url, None)?;
            continue;
        };
        let (http, limit) = (http.clone(), limit.clone());
        fetches.spawn(async move {
            let _permit = limit.acquire_owned().await;
            (feed_id, url, fetch_icon(&http, resolved).await)
        });
    }
    while let Some(done) = fetches.join_next().await {
        if let Ok((feed_id, url, Some(icon))) = done {
            db.set_icon(&feed_id, &url, icon.as_deref())?;
        }
    }
    Ok(())
}

/// The icon as a `data:` URL, `Some(None)` if there is no usable image there, or `None`
/// to try again next sync.
async fn fetch_icon(http: &Untrusted, url: reqwest::Url) -> Option<Option<String>> {
    // On this device or a private network: no icon, for good.
    let Some(request) = http.get(url) else {
        return Some(None);
    };
    let mut res = match request.send().await {
        Ok(res) => res,
        // A name or a redirect that leads there: no icon, for good.
        Err(e) if net::is_blocked(&e) => return Some(None),
        Err(_) => return None,
    };
    let status = res.status();
    if status.is_server_error()
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
    {
        return None;
    }
    let mime = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).trim().to_owned())
        .filter(|m| m.starts_with("image/"));
    let too_big = res
        .content_length()
        .is_some_and(|n| n > MAX_ICON_BYTES as u64);
    let (true, Some(mime)) = (status.is_success() && !too_big, mime) else {
        return Some(None);
    };
    // Read up to the limit only: a server may send no length, or the wrong one.
    let mut body = Vec::new();
    while let Some(chunk) = res.chunk().await.ok()? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_ICON_BYTES {
            return Some(None);
        }
    }
    if body.is_empty() {
        return Some(None);
    }
    Some(Some(format!("data:{mime};base64,{}", BASE64.encode(&body))))
}

/// Skvaller shows each feed in one folder: the first one the server lists.
fn feed_row(sub: Subscription) -> FeedRow {
    let folder = sub.categories.into_iter().next().map(|c| {
        let title = c
            .label
            .unwrap_or_else(|| c.id.rsplit('/').next().unwrap_or(&c.id).to_owned());
        (c.id, title)
    });
    FeedRow {
        id: sub.id,
        title: sub.title,
        folder,
        icon_url: sub.icon_url,
        url: sub.url,
        site_url: sub.html_url,
    }
}

/// Stores articles from the server, skipping any with a malformed ID and any the prune
/// would remove: read, unstarred and published before `prune_before` (Unix
/// milliseconds). Returns how many rows changed, and the newest server crawl time among
/// the items in Unix milliseconds. Only the crawl time counts: a feed's publish dates can
/// be in the future.
///
/// A recent item with an old publish date, read already, would otherwise be stored and
/// pruned again, and flash in the list, on every sync until its crawl time leaves
/// the overlap.
fn store(db: &Db, items: Vec<Item>, prune_before: i64) -> Result<(usize, Option<i64>)> {
    let newest = items.iter().filter_map(Item::crawl_time).max();
    let rows: Vec<ArticleRow> = items
        .into_iter()
        .filter_map(|item| {
            let id = parse_item_id(&item.id)?;
            let (read, starred) = (item.has_state(READ), item.has_state(STARRED));
            let crawl_time = item.crawl_time();
            let url = item
                .canonical
                .into_iter()
                .chain(item.alternate)
                .next()
                .map(|l| l.href)
                .unwrap_or_default();
            let thumbnail = thumbnail::from_enclosures(
                item.enclosure
                    .iter()
                    .map(|e| (e.href.as_str(), e.mime.as_deref())),
                &url,
            );
            let html = item
                .content
                .or(item.summary)
                .map(|c| c.content)
                .unwrap_or_default();
            Some(ArticleRow {
                id,
                read,
                starred,
                url,
                html,
                thumbnail,
                feed_title: item
                    .origin
                    .title
                    .unwrap_or_else(|| item.origin.stream_id.clone()),
                feed_id: item.origin.stream_id,
                title: item.title.unwrap_or_default(),
                author: item.author.unwrap_or_default(),
                // Without a publish date, when the server got it beats 1970, which
                // would sort it last and get it pruned and fetched again.
                published: item
                    .published
                    .filter(|&s| s > 0)
                    .map(|s| s.saturating_mul(1000))
                    .or(crawl_time)
                    .unwrap_or_default(),
            })
        })
        .filter(|row| !(row.read && !row.starred && row.published < prune_before))
        .collect();
    Ok((db.upsert_articles(&rows)?, newest))
}

#[cfg(test)]
mod tests;
