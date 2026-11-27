//! Two-way sync between the server and the local cache.

use crate::api::{Item, READ, READING_LIST, STARRED, Subscription, parse_item_id};
use crate::db::{ArticleRow, FeedRow, Flag};
use crate::{Client, DAY, Db, Error, Result, icons, thumbnail};

/// Later syncs list articles from this much before the newest crawl time already seen,
/// so an article the server stored while the last sync ran is not missed.
const CRAWL_OVERLAP: i64 = 60 * 60;
/// How long read, unstarred articles stay cached unless the user changes the setting.
/// The first sync fetches read articles from the same period.
pub const DEFAULT_KEEP_READ_DAYS: u32 = 30;
/// Item IDs per `edit-tag` or `stream/items/contents` request.
const BATCH: usize = 250;
/// Articles stored per write transaction.
const WRITE_BATCH: usize = 50;

/// A chunk of queued changes the server took: the flag, its value and the article IDs.
type Sent = (Flag, bool, Vec<String>);

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
///
/// # Errors
///
/// Returns `Error::Unauthorized` when the token is no longer accepted, `Error::Http`
/// or `Error::Protocol` when the server fails or answers unexpectedly, and `Error::Db`
/// if the cache can't be written. Changes not sent stay queued for the next sync. Changes
/// the server took are cleared from the queue once the pull is done, even if it failed.
pub async fn sync(client: &Client, db: &Db, now: i64, keep_read_days: u32) -> Result<SyncReport> {
    let mut sent = Vec::new();
    // A push the server refuses doesn't hold back the pull, since queued changes are kept
    // and win anyway. Its error is returned once the pull is done.
    let pushed = push(client, db, &mut sent).await;
    let pulled = match pushed {
        Err(Error::Unauthorized) => Err(Error::Unauthorized),
        _ => pull(client, db, now, keep_read_days).await,
    };
    // Only after the pull, so a server that applies edits lazily and still lists the old
    // state can't turn a change back in this sync; a later sync takes the server's state.
    // But whatever the pull did, so an old change is never sent again over a newer one
    // made elsewhere.
    for (flag, value, ids) in &sent {
        db.clear_pending(*flag, *value, ids)?;
    }
    let report = pulled?;
    pushed?;
    db.set_last_sync(now * 1000)?;
    Ok(SyncReport {
        pushed: sent.iter().map(|(.., ids)| ids.len()).sum(),
        ..report
    })
}

/// Pulls feeds, folders, articles and flags, and prunes. Its report has nothing pushed.
async fn pull(client: &Client, db: &Db, now: i64, keep_read_days: u32) -> Result<SyncReport> {
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
        // Never past now: a crawl time in the future would skip what comes before it.
        db.bump_newest_crawl(ms.min(now * 1000))?;
    }
    // Never unread articles: the server lists them, so the next sync would fetch them
    // again. The server's own retention limits them.
    let pruned = db.prune(prune_before, i64::MIN)?;
    db.optimize()?;
    let server: Vec<_> = client.api_url().into_iter().collect();
    icons::fetch(db, |url| client.resolve(url), &server, None).await?;
    Ok(SyncReport {
        pushed: 0,
        fetched,
        changed,
        pruned,
        subscriptions,
    })
}

/// Sends queued changes, a chunk at a time, and adds the chunks the server took to
/// `sent`. They stay queued for the caller to clear. A chunk the server refuses stays
/// queued and the rest still go; the first such error is returned at the end.
async fn push(client: &Client, db: &Db, sent: &mut Vec<Sent>) -> Result<()> {
    let pending = db.pending()?;
    if pending.is_empty() {
        return Ok(());
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
                    Ok(()) => sent.push((flag, value, chunk.to_vec())),
                    Err(Error::Unauthorized) => return Err(Error::Unauthorized),
                    Err(e) => {
                        failed.get_or_insert(e);
                    }
                }
            }
        }
    }
    failed.map_or(Ok(()), Err)
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
                .map_or_default(|l| l.href);
            let thumbnail = thumbnail::from_enclosures(
                item.enclosure
                    .iter()
                    .map(|e| (e.href.as_str(), e.mime.as_deref())),
                &url,
            );
            let html = item.content.or(item.summary).map_or_default(|c| c.content);
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
    let mut stored = 0;
    // A few transactions per page, so a read or star change from the UI, which waits
    // for the write lock, waits for one of these rather than the whole page.
    for chunk in rows.chunks(WRITE_BATCH) {
        stored += db.upsert_articles(chunk)?;
    }
    Ok((stored, newest))
}

#[cfg(test)]
mod tests;
