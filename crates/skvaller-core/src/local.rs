//! The local account: feeds Skvaller fetches and parses itself, with no server.
//! Subscriptions, folders and read and starred state live only in the cache, so there
//! is nothing to send anywhere, and a fetch never changes what the user did.

use std::collections::HashSet;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use feed_rs::model;
use reqwest::header::{CONTENT_TYPE, ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED};
use reqwest::{StatusCode, Url};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::db::{ArticleRow, LocalFeed, NewFeed};
use crate::full_text::{USER_AGENT, decode};
use crate::html;
use crate::sync::{DAY, fetch_icons};
use crate::text::plain_text;
use crate::thumbnail;
use crate::{Db, Error, Result, SyncReport};

/// Larger feeds are not read. Big ones, with full articles, are a few megabytes.
const MAX_FEED_BYTES: usize = 10 * 1024 * 1024;
/// How much of a web page is looked through for a link to its feed.
const MAX_HEAD_BYTES: usize = 64 * 1024;
/// Feeds fetched at the same time.
const PARALLEL: usize = 6;
/// For the whole response. Generous: a big feed on a slow line takes a while.
const TIMEOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

/// One client for every feed, so connections are reused. It keeps no cookies.
fn http() -> Result<reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .user_agent(USER_AGENT)
        .build()?;
    Ok(CLIENT.get_or_init(|| client).clone())
}

/// Fetches every feed, stores what is new or changed, then removes unstarred articles
/// published more than `keep_read_days` ago if read, or `keep_unread_days` ago if
/// unread; `None` keeps unread ones. `now` is Unix seconds. A feed that fails keeps its
/// articles and records why, for the sidebar; the others go on.
pub async fn refresh(
    db: &Db,
    now: i64,
    keep_read_days: u32,
    keep_unread_days: Option<u32>,
) -> Result<SyncReport> {
    db.clear_all_pending()?;
    let http = http()?;
    let limit = Arc::new(Semaphore::new(PARALLEL));
    let mut fetches = JoinSet::new();
    for feed in db.local_feeds()? {
        let (http, limit) = (http.clone(), limit.clone());
        fetches.spawn(async move {
            // Held until the feed is stored, so only so many bodies wait in memory.
            let permit = limit.acquire_owned().await;
            let fetched = fetch(&http, &feed.url, Some(&feed)).await;
            (feed, fetched, permit)
        });
    }
    let (mut fetched, mut tried, mut reached) = (0, 0, 0);
    while let Some(done) = fetches.join_next().await {
        let Ok((feed, result, _permit)) = done else {
            continue;
        };
        tried += 1;
        if !matches!(result, Err(FetchError::Unreachable(_))) {
            reached += 1;
        }
        match result {
            Ok(Some(page)) => match parse(&page) {
                Some(parsed) => {
                    fetched += store(db, &feed.id, &parsed, &page, now)?;
                }
                None => db.set_fetch_error(&feed.id, Some("not a feed"))?,
            },
            Ok(None) => db.set_fetch_error(&feed.id, None)?,
            Err(e) => db.set_fetch_error(&feed.id, Some(&e.message()))?,
        }
    }
    // Pruning needs no network, so the retention period holds offline too.
    let before = |days: u32| (now - i64::from(days) * DAY) * 1000;
    // `seen` keeps their IDs, so the feed doesn't bring them back as new.
    let pruned = db.prune(
        before(keep_read_days),
        keep_unread_days.map_or(i64::MIN, before),
    )?;
    db.optimize()?;
    // Every feed out of reach is more likely the network than every site down: a
    // failure, so it is tried again, as a sync account's would be.
    if tried > 0 && reached == 0 {
        return Err(Error::Local("could not reach any feed".into()));
    }
    fetch_icons(db, |url| Url::parse(url).ok(), None).await?;
    db.set_last_sync(now * 1000)?;
    Ok(SyncReport {
        fetched,
        pruned,
        ..SyncReport::default()
    })
}

/// Subscribes to the feed at `url`, or to the one a web page there links to, and
/// stores its entries. Returns the feed's ID; a feed subscribed to already keeps its
/// folder. `now` is Unix seconds, the date of entries that have none.
pub async fn subscribe(db: &Db, url: &str, folder_id: Option<&str>, now: i64) -> Result<String> {
    let http = http()?;
    let url = with_scheme(url).ok_or(Error::BadUrl)?;
    let page = fetch(&http, url.as_str(), None)
        .await
        .map_err(|e| Error::Local(e.message()))?
        .ok_or(Error::NoFeed)?;
    let (page, parsed) = match parse(&page) {
        Some(parsed) => (page, parsed),
        None => {
            let link = discover_off_runtime(&page).await.ok_or(Error::NoFeed)?;
            let page = fetch(&http, link.as_str(), None)
                .await
                .map_err(|e| Error::Local(e.message()))?
                .ok_or(Error::NoFeed)?;
            let parsed = parse(&page).ok_or(Error::NoFeed)?;
            (page, parsed)
        }
    };

    // Subscribed already, maybe under the address before redirects.
    if let Some(id) = db.feed_by_url(&[url.as_str(), page.url.as_str()])? {
        return Ok(id);
    }
    let id = format!("feed/{}", page.url);
    let site_url = site_url(&parsed, &page.url);
    let title = parsed
        .title
        .as_ref()
        .map(text)
        .filter(|t| !t.is_empty())
        .or_else(|| page.url.host_str().map(str::to_owned))
        .unwrap_or_else(|| page.url.to_string());
    let icon_url = icon_url(&parsed, site_url.as_ref().unwrap_or(&page.url));
    let added = db.add_local_feed(&NewFeed {
        id: &id,
        url: page.url.as_str(),
        title: &title,
        site_url: site_url.as_ref().map(Url::as_str),
        folder_id,
        icon_url: icon_url.as_deref(),
    })?;
    if added {
        store(db, &id, &parsed, &page, now)?;
        fetch_icons(db, |url| Url::parse(url).ok(), Some(&id)).await?;
    }
    Ok(id)
}

/// A fetched feed or page.
struct Page {
    /// After redirects.
    url: Url,
    body: Vec<u8>,
    content_type: String,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// Fetches `url`. With `known`, the feed's last fetch, asks only for a newer one:
/// `Ok(None)` means it has not changed. `Err` says why it failed, for the user.
async fn fetch(
    http: &reqwest::Client,
    url: &str,
    known: Option<&LocalFeed>,
) -> Result<Option<Page>, FetchError> {
    let url = Url::parse(url).map_err(|_| FetchError::Refused("not a valid address".to_owned()))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(FetchError::Refused("not an http or https address".into()));
    }
    let mut req = http.get(url);
    if let Some(feed) = known {
        if let Some(etag) = &feed.etag {
            req = req.header(IF_NONE_MATCH, etag);
        }
        if let Some(modified) = &feed.last_modified {
            req = req.header(IF_MODIFIED_SINCE, modified);
        }
    }
    let mut res = req.send().await.map_err(network_error)?;
    let status = res.status();
    if status == StatusCode::NOT_MODIFIED && known.is_some() {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(FetchError::Refused(match status.canonical_reason() {
            Some(reason) => format!("the server answered {} {reason}", status.as_u16()),
            None => format!("the server answered {}", status.as_u16()),
        }));
    }
    if res
        .content_length()
        .is_some_and(|n| n > MAX_FEED_BYTES as u64)
    {
        return Err(FetchError::Refused("the feed is too large".into()));
    }
    let header = |name| {
        res.headers()
            .get(name)
            .and_then(|v: &reqwest::header::HeaderValue| v.to_str().ok())
            .map(str::to_owned)
    };
    let (content_type, etag, last_modified) = (
        header(CONTENT_TYPE).unwrap_or_default(),
        header(ETAG),
        header(LAST_MODIFIED),
    );
    let url = res.url().clone();
    let mut body = Vec::new();
    while let Some(chunk) = res.chunk().await.map_err(network_error)? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_FEED_BYTES {
            return Err(FetchError::Refused("the feed is too large".into()));
        }
    }
    Ok(Some(Page {
        url,
        body,
        content_type,
        etag,
        last_modified,
    }))
}

/// Why a fetch failed, said for the user.
enum FetchError {
    /// No answer: likely the network, so a refresh that gets only these is offline.
    Unreachable(String),
    /// An answer that can't be used.
    Refused(String),
}

impl FetchError {
    fn message(self) -> String {
        match self {
            FetchError::Unreachable(m) | FetchError::Refused(m) => m,
        }
    }
}

/// Only no answer at all counts as unreachable; a broken one, such as a redirect loop, is
/// an answer. The message leaves out the URL, which can hold a username and password and
/// is stored and shown as the feed's error.
fn network_error(e: reqwest::Error) -> FetchError {
    if e.is_timeout() {
        FetchError::Unreachable("the server took too long to answer".into())
    } else if e.is_connect() {
        FetchError::Unreachable("could not reach the server".into())
    } else {
        FetchError::Refused(e.without_url().to_string())
    }
}

/// The page as an RSS, Atom or JSON feed, if it is one.
fn parse(page: &Page) -> Option<model::Feed> {
    feed_rs::parser::Builder::new()
        .base_uri(Some(page.url.as_str()))
        .id_generator(entry_key)
        .build()
        .parse(page.body.as_slice())
        .ok()
}

/// What tells an entry without an ID apart: its link and title. Empty if it has
/// neither; such entries are skipped.
fn entry_key(links: &[model::Link], title: &Option<model::Text>, _uri: Option<&str>) -> String {
    let link = links.first().map_or("", |l| l.href.as_str());
    let title = title.as_ref().map_or("", |t| t.content.as_str());
    if link.is_empty() && title.is_empty() {
        String::new()
    } else {
        format!("{link}\n{title}")
    }
}

/// `discover`, off the async runtime: parsing a large page takes a while.
async fn discover_off_runtime(page: &Page) -> Option<Url> {
    let (body, content_type, url) = (
        page.body.clone(),
        page.content_type.clone(),
        page.url.clone(),
    );
    tokio::task::spawn_blocking(move || discover(&body, &content_type, &url))
        .await
        .ok()
        .flatten()
}

/// The feed a web page links to with `<link rel="alternate">`, the first one. `None`
/// for a page that nests too deeply to parse in good time, like Reader View's guard.
fn discover(body: &[u8], content_type: &str, base: &Url) -> Option<Url> {
    // The links are in the head, so only its start is parsed.
    let start = &body[..body.len().min(MAX_HEAD_BYTES)];
    let html = decode(start, content_type);
    let end = html
        .to_ascii_lowercase()
        .find("<body")
        .unwrap_or(html.len());
    let document = html::parse(&html[..end])?;
    let types = [
        "application/rss+xml",
        "application/atom+xml",
        "application/feed+json",
        "application/json",
    ];
    document
        .select(r#"link[rel~="alternate"][href]"#)
        .iter()
        .filter(|link| {
            link.attr("type")
                .is_some_and(|t| types.contains(&t.trim().to_ascii_lowercase().as_str()))
        })
        .find_map(|link| web_url(base.join(link.attr("href")?.trim()).ok()?))
}

/// Stores the feed's entries, and returns how many rows changed. Of entries that share
/// an ID, as in some broken feeds, the first counts: storing each would rewrite the
/// article on every refresh, and a new URL drops its Reader View text.
fn store(db: &Db, feed_id: &str, feed: &model::Feed, page: &Page, now: i64) -> Result<usize> {
    let mut ids = HashSet::new();
    let entries: Vec<(ArticleRow, bool)> = feed
        .entries
        .iter()
        .filter(|entry| !entry.id.is_empty() && ids.insert(entry.id.as_str()))
        .map(|entry| {
            let html = entry
                .content
                .as_ref()
                .and_then(|c| c.body.clone())
                .or_else(|| entry.summary.as_ref().map(|s| s.content.clone()))
                .unwrap_or_default();
            let date = entry.published.or(entry.updated);
            let row = ArticleRow {
                id: article_id(feed_id, &entry.id).to_string(),
                feed_id: feed_id.to_owned(),
                feed_title: String::new(),
                title: entry.title.as_ref().map(text).unwrap_or_default(),
                author: entry
                    .authors
                    .iter()
                    .find_map(|a| a.name.clone())
                    .unwrap_or_default(),
                url: entry_url(entry, &page.url),
                published: date.map_or(now * 1000, |d| d.timestamp_millis()),
                read: false,
                starred: false,
                html,
                thumbnail: thumbnail::from_media(&entry.media, &page.url),
            };
            (row, date.is_some())
        })
        .collect();
    db.store_local_feed(
        feed_id,
        &entries,
        page.etag.as_deref(),
        page.last_modified.as_deref(),
    )
}

/// The entry's web page: its alternate link, else its first one.
fn entry_url(entry: &model::Entry, base: &Url) -> String {
    let link = entry
        .links
        .iter()
        .find(|l| l.rel.as_deref().is_none_or(|rel| rel == "alternate"))
        .or(entry.links.first());
    link.and_then(|l| web_url(base.join(&l.href).ok()?))
        .map(String::from)
        .unwrap_or_default()
}

/// A title as plain text. Atom titles can be HTML.
fn text(t: &model::Text) -> String {
    let content = t.content.trim();
    if t.content_type.to_string().contains("html") {
        plain_text(content, usize::MAX)
    } else {
        content.to_owned()
    }
}

/// The web site the feed is for: its alternate link, else its first one that is not
/// itself. Resolved against `base`, the feed's URL: feed-rs leaves JSON Feed links as
/// they are.
fn site_url(feed: &model::Feed, base: &Url) -> Option<Url> {
    let links = || {
        feed.links
            .iter()
            .filter(|l| l.rel.as_deref() != Some("self"))
    };
    links()
        .find(|l| l.rel.as_deref() == Some("alternate"))
        .or_else(|| links().next())
        .and_then(|l| web_url(base.join(&l.href).ok()?))
}

/// Only http(s) addresses from a feed: it is untrusted.
fn web_url(url: Url) -> Option<Url> {
    matches!(url.scheme(), "http" | "https").then_some(url)
}

/// The feed's icon, else its logo, else the site's favicon.
fn icon_url(feed: &model::Feed, site: &Url) -> Option<String> {
    feed.icon
        .as_ref()
        .or(feed.logo.as_ref())
        .and_then(|image| web_url(site.join(&image.uri).ok()?))
        .or_else(|| web_url(site.join("/favicon.ico").ok()?))
        .map(String::from)
}

/// What the user typed, with `https://` when no scheme is given.
fn with_scheme(input: &str) -> Option<Url> {
    let input = input.trim();
    let url = if input.contains("://") {
        Url::parse(input)
    } else {
        Url::parse(&format!("https://{input}"))
    };
    url.ok().filter(|u| matches!(u.scheme(), "http" | "https"))
}

/// A stable ID for an entry: 63 bits of an FNV-1a hash of the feed and the entry's own
/// ID. The same entry always gets the same one, so a fetch updates it instead of
/// adding it again. Stable across Rust versions, unlike the standard library's hasher.
fn article_id(feed_id: &str, entry_id: &str) -> i64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in feed_id.bytes().chain([0]).chain(entry_id.bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    (hash & i64::MAX as u64) as i64
}

#[cfg(test)]
mod tests;
