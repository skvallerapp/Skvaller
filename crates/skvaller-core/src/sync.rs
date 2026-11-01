//! Two-way sync between the server and the local cache.

use crate::api::{Item, READ, READING_LIST, STARRED, Subscription, next_page, parse_item_id};
use crate::db::{ArticleRow, FeedRow, Flag};
use crate::{Client, Db, Result};

const DAY: i64 = 24 * 60 * 60;
/// Later syncs re-fetch this much before the newest crawl time already seen, so an
/// article the server stored while the last sync ran is not missed.
const CRAWL_OVERLAP: i64 = 60 * 60;
/// How long read, unstarred articles stay cached unless the user changes the setting.
/// The first sync fetches read articles from the same period (FR-5b, FR-5c).
pub const DEFAULT_KEEP_READ_DAYS: u32 = 30;
/// Item IDs per `edit-tag` or `stream/items/contents` request.
const BATCH: usize = 250;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Queued local changes sent to the server.
    pub pushed: usize,
    /// Articles added, or updated because their content changed.
    pub fetched: usize,
    /// Cached articles whose read or starred state changed on the server.
    pub changed: usize,
    /// Old read articles removed from the cache.
    pub pruned: usize,
}

/// Sends queued local changes, then pulls feeds, folders, articles and read and starred
/// state, then removes read, unstarred articles published more than `keep_read_days`
/// ago. `now` is Unix seconds.
///
/// The first sync fetches every unread and starred article, plus read articles from the
/// last `keep_read_days`. Later syncs fetch what the server crawled since then, plus any unread or
/// starred article not cached yet. A flag with a queued local change is never
/// overwritten.
pub async fn sync(client: &Client, db: &Db, now: i64, keep_read_days: u32) -> Result<SyncReport> {
    let pushed = push(client, db).await?;
    let keep_read_since = now - i64::from(keep_read_days) * DAY;

    let subs = client.subscriptions().await?;
    // FreshRSS leaves feeds set to "show in its feed only" out of the reading list, so
    // those are fetched one by one.
    let streams: Vec<String> = std::iter::once(READING_LIST.to_owned())
        .chain(
            subs.iter()
                .filter(|s| s.priority.as_deref() == Some("feed"))
                .map(|s| s.id.clone()),
        )
        .collect();
    db.replace_subscriptions(&subs.into_iter().map(feed_row).collect::<Vec<_>>())?;

    let since = match db.newest_crawl()? {
        Some(ms) => ms / 1000 - CRAWL_OVERLAP,
        None => keep_read_since,
    };
    let mut fetched = 0;
    let mut newest_crawl = None;
    for stream in &streams {
        let mut continuation: Option<String> = None;
        loop {
            let page = client
                .stream_contents(stream, since, continuation.as_deref())
                .await?;
            let (stored, newest) = store(db, page.items)?;
            fetched += stored;
            newest_crawl = newest_crawl.max(newest);
            match next_page(page.continuation, &continuation) {
                Some(c) => continuation = Some(c),
                None => break,
            }
        }
    }

    let mut unread = Vec::new();
    for stream in &streams {
        unread.extend(client.item_ids(stream, Some(READ)).await?);
    }
    let starred = client.item_ids(STARRED, None).await?;
    let wanted: Vec<i64> = unread.iter().chain(&starred).copied().collect();
    for chunk in db.missing_ids(&wanted)?.chunks(BATCH) {
        fetched += store(db, client.items_contents(chunk).await?)?.0;
    }
    let changed = db.reconcile(Flag::Read, &unread)? + db.reconcile(Flag::Starred, &starred)?;

    // Only once every page is stored, so an interrupted first sync starts over.
    if let Some(ms) = newest_crawl {
        db.bump_newest_crawl(ms)?;
    }
    let pruned = db.prune(keep_read_since * 1000)?;
    Ok(SyncReport {
        pushed,
        fetched,
        changed,
        pruned,
    })
}

async fn push(client: &Client, db: &Db) -> Result<usize> {
    let pending = db.pending()?;
    if pending.is_empty() {
        return Ok(0);
    }
    let token = client.write_token().await?;
    for flag in [Flag::Read, Flag::Starred] {
        for value in [true, false] {
            let ids: Vec<i64> = pending
                .iter()
                .filter(|&&(_, f, v)| f == flag && v == value)
                .map(|&(id, ..)| id)
                .collect();
            for chunk in ids.chunks(BATCH) {
                client.edit_tag(&token, chunk, flag.tag(), value).await?;
                db.clear_pending(flag, value, chunk)?;
            }
        }
    }
    Ok(pending.len())
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
    }
}

/// Stores articles from the server, skipping any with a malformed ID. Returns how many
/// rows changed, and the newest server crawl time among the items in Unix milliseconds.
/// Only the crawl time counts: a feed's publish dates can be in the future.
fn store(db: &Db, items: Vec<Item>) -> Result<(usize, Option<i64>)> {
    let newest = items
        .iter()
        .filter_map(|item| item.crawl_time_msec.as_deref()?.parse().ok())
        .max();
    let rows: Vec<ArticleRow> = items
        .into_iter()
        .filter_map(|item| {
            Some(ArticleRow {
                id: parse_item_id(&item.id)?,
                read: item.has_state(READ),
                starred: item.has_state(STARRED),
                url: item
                    .canonical
                    .into_iter()
                    .chain(item.alternate)
                    .next()
                    .map(|l| l.href)
                    .unwrap_or_default(),
                html: item
                    .content
                    .or(item.summary)
                    .map(|c| c.content)
                    .unwrap_or_default(),
                feed_title: item
                    .origin
                    .title
                    .unwrap_or_else(|| item.origin.stream_id.clone()),
                feed_id: item.origin.stream_id,
                title: item.title.unwrap_or_default(),
                author: item.author.unwrap_or_default(),
                published: item.published * 1000,
            })
        })
        .collect();
    Ok((db.upsert_articles(&rows)?, newest))
}

#[cfg(test)]
mod tests;
