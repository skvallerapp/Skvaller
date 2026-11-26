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
use crate::html;
use crate::net::{self, USER_AGENT, decode, web_url};
use crate::text::plain_text;
use crate::thumbnail;
use crate::{DAY, Db, Error, Result, SyncReport, icons};

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

/// Fetches every feed, stores what is new or changed, then prunes old articles.
///
/// Pruning removes unstarred articles published more than `keep_read_days` ago if
/// read, or `keep_unread_days` ago if unread; `None` keeps unread ones. `now` is Unix
/// seconds. A feed that fails keeps its articles and records why, for the sidebar; the
/// others go on.
///
/// # Errors
///
/// Returns `Error::Local` when no feed could be reached, which is more likely the
/// network than every site down, `Error::Http` if the HTTP client can't be built, and
/// `Error::Db` if the cache can't be written.
pub async fn refresh(
    db: &Db,
    now: i64,
    keep_read_days: u32,
    keep_unread_days: Option<u32>,
) -> Result<SyncReport> {
    db.clear_all_pending()?;
    let http = http()?;
    let limit = Arc::new(Semaphore::new(PARALLEL));
    let mut jobs = JoinSet::new();
    let feeds = db.local_feeds()?;
    // The user subscribed to these, LAN or not: their icons may come from them.
    let trusted: Vec<Url> = feeds
        .iter()
        .filter_map(|f| Url::parse(&f.url).ok())
        .collect();
    for feed in feeds {
        let (http, limit) = (http.clone(), Arc::clone(&limit));
        jobs.spawn(async move {
            // Held until the feed is stored, so only so many bodies wait in memory.
            let permit = limit.acquire_owned().await;
            let fetched = fetch(&http, &feed.url, Some(&feed)).await;
            (feed, fetched, permit)
        });
    }
    let (mut fetched, mut tried, mut reached) = (0, 0, 0);
    while let Some(done) = jobs.join_next().await {
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
    // Pruned articles' IDs stay in `seen`, so the feed doesn't bring them back as new.
    let pruned = db.prune(
        before(keep_read_days),
        keep_unread_days.map_or(i64::MIN, before),
    )?;
    db.optimize()?;
    // No feed reached is more likely the network than every site down: fail, so the
    // refresh is retried, as a sync account's would be.
    if tried > 0 && reached == 0 {
        return Err(Error::Local("could not reach any feed".into()));
    }
    icons::fetch(db, |url| Url::parse(url).ok(), &trusted, None).await?;
    db.set_last_sync(now * 1000)?;
    Ok(SyncReport {
        fetched,
        pruned,
        ..SyncReport::default()
    })
}

/// Subscribes to the feed at `url`, or to the one a web page there links to.
///
/// Stores its entries and returns the feed's ID; a feed subscribed to already keeps its
/// folder. `now` is Unix seconds, the date of entries that have none.
///
/// # Errors
///
/// Returns `Error::BadUrl` for an unusable address, `Error::NoFeed` when there is no
/// feed there or linked from there, `Error::Local` when it can't be fetched, said for
/// the user, `Error::Http` if the HTTP client can't be built, and `Error::Db` if the
/// cache can't be written.
pub async fn subscribe(db: &Db, url: &str, folder_id: Option<&str>, now: i64) -> Result<String> {
    let http = http()?;
    let url = with_scheme(url).ok_or(Error::BadUrl)?;
    let page = fetch_new(&http, &url).await?;
    let (page, parsed) = if let Some(parsed) = parse(&page) {
        (page, parsed)
    } else {
        let link = discover_off_runtime(&page).await.ok_or(Error::NoFeed)?;
        let page = fetch_new(&http, &link).await?;
        let parsed = parse(&page).ok_or(Error::NoFeed)?;
        (page, parsed)
    };

    // Subscribed already, maybe under the address before redirects.
    if let Some(id) = db.feed_by_url(&[url.as_str(), page.url.as_str()])? {
        return Ok(id);
    }
    let id = feed_id(&page.url);
    let site_url = site_url(&parsed, &page.url);
    let title = parsed
        .title
        .as_ref()
        .map(text)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| fallback_title(&page.url));
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
        let trusted = [page.url.clone()];
        icons::fetch(db, |url| Url::parse(url).ok(), &trusted, Some(&id)).await?;
    }
    Ok(id)
}

/// A feed's ID: its address, after redirects. Subscribing and importing must agree on
/// it, since it tells a feed subscribed to already.
pub(crate) fn feed_id(url: &Url) -> String {
    format!("feed/{url}")
}

/// The title of a feed that names none: its host, else its address.
pub(crate) fn fallback_title(url: &Url) -> String {
    url.host_str()
        .map_or_else(|| url.to_string(), str::to_owned)
}

/// `url` fetched unconditionally, its failure said for the user.
async fn fetch_new(http: &reqwest::Client, url: &Url) -> Result<Page> {
    match fetch(http, url.as_str(), None).await {
        Ok(page) => Ok(page.expect("only a conditional fetch finds nothing new")),
        Err(e) => Err(Error::Local(e.message())),
    }
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
    #[expect(
        clippy::map_err_ignore,
        reason = "the parser's error is not worded for the user"
    )]
    let url = Url::parse(url).map_err(|_| FetchError::Refused("not a valid address".to_owned()))?;
    let url =
        web_url(url).ok_or_else(|| FetchError::Refused("not an http or https address".into()))?;
    let mut req = http.get(url);
    if let Some(feed) = known {
        if let Some(etag) = &feed.etag {
            req = req.header(IF_NONE_MATCH, etag);
        }
        if let Some(modified) = &feed.last_modified {
            req = req.header(IF_MODIFIED_SINCE, modified);
        }
    }
    let res = req.send().await.map_err(network_error)?;
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
    let body = net::read_capped(res, MAX_FEED_BYTES)
        .await
        .map_err(network_error)?
        .ok_or_else(|| FetchError::Refused("the feed is too large".into()))?;
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
    // A parser that failed is left mid-entry, and panics if used again.
    let parse = |body: &[u8]| {
        feed_rs::parser::Builder::new()
            .base_uri(Some(page.url.as_str()))
            .id_generator(entry_key)
            .build()
            .parse(body)
            .ok()
    };
    parse(&page.body).or_else(|| {
        // feed-rs fails the whole feed on one media text it can't read, such as the
        // empty `media:description` YouTube writes for a video without one.
        parse(&without_bad_media_texts(&page.body)?)
    })
}

/// `body` without the `<media:title>` and `<media:description>` elements feed-rs
/// can't read, or `None` if it has none: those with no text, only blanks, comments or
/// an empty CDATA, and those of a `type` other than plain or html. Only the `media`
/// prefix is looked for, as feeds use; a misread one costs no more than the feed's
/// failing.
fn without_bad_media_texts(body: &[u8]) -> Option<Vec<u8>> {
    const OPEN: &[u8] = b"<media:";
    const NAMES: [&[u8]; 2] = [b"title", b"description"];
    let mut out = Vec::with_capacity(body.len());
    let mut rest = body;
    let mut found = false;
    // A closing tag not found after one element isn't after a later one either: each
    // search is done once, so a feed of unclosed ones takes no longer than a good one.
    let mut unclosed = [false; NAMES.len()];
    while let Some(at) = rest.windows(OPEN.len()).position(|w| w == OPEN) {
        let tag = &rest[at..];
        let name = NAMES.iter().position(|name| {
            let end = OPEN.len() + name.len();
            tag[OPEN.len()..].starts_with(name)
                && tag
                    .get(end)
                    .is_some_and(|&b| b == b'>' || b == b'/' || b.is_ascii_whitespace())
        });
        let text = name.map(|n| (n, media_text(tag, OPEN.len() + NAMES[n].len(), unclosed[n])));
        let kept = match text {
            // No `>` after it: none after a later one either.
            Some((_, MediaText::Unended)) => break,
            Some((n, MediaText::Unclosed)) => {
                unclosed[n] = true;
                OPEN.len()
            }
            Some((_, MediaText::Unreadable(len))) => {
                out.extend_from_slice(&rest[..at]);
                rest = &rest[at + len..];
                found = true;
                continue;
            }
            // Kept whole: what it holds is its text, not more elements.
            Some((_, MediaText::Readable(len))) => len,
            None => OPEN.len(),
        };
        out.extend_from_slice(&rest[..at + kept]);
        rest = &rest[at + kept..];
    }
    out.extend_from_slice(rest);
    found.then_some(out)
}

/// A media text element, with its length up to the end of its closing tag.
enum MediaText {
    /// Its start tag has no `>`.
    Unended,
    /// It has no closing tag.
    Unclosed,
    Readable(usize),
    /// One feed-rs can't read.
    Unreadable(usize),
}

/// The element `tag` starts with, its name `tag[1..name_end]`, and whether feed-rs
/// can read its text. `unclosed` when an earlier one found no closing tag.
fn media_text(tag: &[u8], name_end: usize, unclosed: bool) -> MediaText {
    let Some(gt) = tag[name_end..].iter().position(|&b| b == b'>') else {
        return MediaText::Unended;
    };
    let gt = name_end + gt;
    let attrs = &tag[name_end..gt];
    if attrs.ends_with(b"/") {
        return MediaText::Unreadable(gt + 1);
    }
    if unclosed {
        return MediaText::Unclosed;
    }
    let close = [b"</", &tag[1..name_end], b">"].concat();
    let inner = &tag[gt + 1..];
    let Some(close_at) = inner
        .windows(close.len())
        .position(|w| w == close.as_slice())
    else {
        return MediaText::Unclosed;
    };
    let len = gt + 1 + close_at + close.len();
    let typed = ["type=\"", "type='"].iter().find_map(|open| {
        let start = attrs
            .windows(open.len())
            .position(|w| w == open.as_bytes())?
            + open.len();
        let quote = open.as_bytes()[open.len() - 1];
        let value = &attrs[start..];
        Some(&value[..value.iter().position(|&b| b == quote)?])
    });
    let bad_type = typed.is_some_and(|t| t != b"plain" && t != b"html");
    if bad_type || no_text(&inner[..close_at]) {
        MediaText::Unreadable(len)
    } else {
        MediaText::Readable(len)
    }
}

/// Whether XML `content` holds no text but blanks: only comments and CDATA sections
/// of blanks besides.
fn no_text(mut content: &[u8]) -> bool {
    loop {
        content = content.trim_ascii_start();
        let (open, close): (&[u8], &[u8]) = if content.starts_with(b"<!--") {
            (b"<!--", b"-->")
        } else if content.starts_with(b"<![CDATA[") {
            (b"<![CDATA[", b"]]>")
        } else {
            return content.is_empty();
        };
        let Some(end) = content.windows(close.len()).position(|w| w == close) else {
            return false;
        };
        if open == b"<![CDATA[" && !content[open.len()..end].trim_ascii().is_empty() {
            return false;
        }
        content = &content[end + close.len()..];
    }
}

/// What tells an entry without an ID apart: its link and title. Empty if it has
/// neither; such entries are skipped.
#[expect(
    clippy::ref_option,
    reason = "the signature feed-rs's id_generator takes"
)]
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
    // The links are in the head, so only its start is parsed.
    let (head, content_type, url) = (
        page.body[..page.body.len().min(MAX_HEAD_BYTES)].to_vec(),
        page.content_type.clone(),
        page.url.clone(),
    );
    tokio::task::spawn_blocking(move || discover(&head, &content_type, &url))
        .await
        .ok()
        .flatten()
}

/// The feed the start of a web page, `head`, links to with `<link rel="alternate">`, the
/// first one. `None` for a page that nests too deeply to parse in good time, like Reader
/// View's guard.
fn discover(head: &[u8], content_type: &str, base: &Url) -> Option<Url> {
    let html = decode(head, content_type);
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
            // The first body that isn't blank: YouTube's only one is its media's.
            let content = entry.content.as_ref().and_then(|c| {
                Some((
                    c.body.as_deref()?,
                    c.content_type.to_string().contains("html"),
                ))
            });
            let texts = entry
                .summary
                .iter()
                .chain(entry.media.iter().filter_map(|m| m.description.as_ref()))
                .map(|t| {
                    (
                        t.content.as_str(),
                        t.content_type.to_string().contains("html"),
                    )
                });
            let html = content
                .into_iter()
                .chain(texts)
                .find(|(body, _)| !body.trim().is_empty())
                .map_or_default(|(body, is_html)| as_html(body, is_html));
            let date = entry.published.or(entry.updated);
            let row = ArticleRow {
                id: article_id(feed_id, &entry.id).to_string(),
                feed_id: feed_id.to_owned(),
                feed_title: String::new(),
                title: entry.title.as_ref().map_or_default(text),
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
        .map_or_default(String::from)
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

/// A body as HTML: plain text escaped, with its line breaks kept. The reader links
/// its web addresses.
fn as_html(body: &str, is_html: bool) -> String {
    if is_html {
        return body.to_owned();
    }
    format!("<p>{}</p>", html::escape(body.trim()).replace('\n', "<br>"))
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
    // A scheme is only letters, digits, `+`, `-` and `.` before the `://`: in
    // `example.com/?next=https://x`, the `://` is in the query.
    let has_scheme = input.split_once("://").is_some_and(|(scheme, _)| {
        !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
    });
    let url = if has_scheme {
        Url::parse(input)
    } else {
        Url::parse(&format!("https://{input}"))
    };
    url.ok().and_then(web_url)
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
    (hash & i64::MAX.cast_unsigned()).cast_signed()
}

#[cfg(test)]
mod tests;
