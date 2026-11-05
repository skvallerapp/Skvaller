//! Google Reader API client. It uses only the calls that FreshRSS, Miniflux and The Old
//! Reader all have: Miniflux has no `stream/contents`, so articles come by ID.

use std::time::Duration;

use reqwest::header::AUTHORIZATION;
use reqwest::redirect::Policy;
use reqwest::{RequestBuilder, StatusCode, Url};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::{Error, Result};

pub const READING_LIST: &str = "user/-/state/com.google/reading-list";
pub const READ: &str = "user/-/state/com.google/read";
pub const STARRED: &str = "user/-/state/com.google/starred";

/// IDs per `stream/items/ids` page, the most Miniflux and The Old Reader allow. IDs are
/// small, so fewer and larger pages. FreshRSS drops one item at each page boundary when
/// the boundary item changed between requests, so fewer boundaries also means fewer
/// wrong results.
const IDS_PAGE: &str = "10000";
/// FreshRSS's API path, tried when the address the user typed is not the API itself.
const FRESHRSS_PATH: &str = "/api/greader.php";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Time without any data arriving. A large page may take longer than this in total.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
/// A whole request. Generous for large pages, but a server that trickles data can't stall
/// a sync forever.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const LONG_ID_PREFIX: &str = "tag:google.com,2005:reader/item/";

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Client {
    /// Signs in with `ClientLogin`. `base` is the API URL: the server's address for
    /// Miniflux and The Old Reader, `https://rss.example.com/api/greader.php` for
    /// FreshRSS. An address without FreshRSS's path is tried as typed, then with it.
    /// [`Client::base`] tells which one answered.
    pub async fn login(base: &str, username: &str, password: &str) -> Result<Self> {
        let base = base.trim_end_matches('/');
        let mut result = Self::login_at(base, username, password).await;
        if !base.ends_with(FRESHRSS_PATH) {
            // Not the API, or a page that is not one: anything but an answer about the
            // password or the network.
            if let Err(Error::Protocol(_)) = result {
                result =
                    Self::login_at(&format!("{base}{FRESHRSS_PATH}"), username, password).await;
            }
        }
        result
    }

    async fn login_at(base: &str, username: &str, password: &str) -> Result<Self> {
        let mut client = Self::new(base, "")?;
        let res = client
            .http
            .post(format!("{}/accounts/ClientLogin", client.base))
            // The Old Reader documents the Google fields; the others ignore them.
            .form(&[
                ("client", "Skvaller"),
                ("accountType", "HOSTED_OR_GOOGLE"),
                ("service", "reader"),
                ("Email", username),
                ("Passwd", password),
            ])
            .send()
            .await?;
        check(res.status())?;
        let body = res.text().await?;
        client.token = body
            .lines()
            .find_map(|line| line.strip_prefix("Auth="))
            .filter(|token| !token.is_empty())
            .ok_or_else(|| Error::Protocol("no Auth token in the sign-in response".into()))?
            .to_owned();
        Ok(client)
    }

    /// Builds a client from a token saved by an earlier [`Client::login`].
    pub fn new(base: &str, token: &str) -> Result<Self> {
        let url = Url::parse(base).map_err(|_| Error::BadUrl)?;
        // A username or password in the URL would end up in error messages.
        let has_credentials = !url.username().is_empty() || url.password().is_some();
        if !matches!(url.scheme(), "http" | "https") || has_credentials {
            return Err(Error::BadUrl);
        }
        // No redirects: reqwest resends POST bodies (the password, the write token) on a
        // 307 or 308, even to another host.
        let http = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(Policy::none())
            .build()?;
        Ok(Self {
            http,
            base: base.trim_end_matches('/').to_owned(),
            token: token.to_owned(),
        })
    }

    /// The access token, to save in the keychain.
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The API URL, without a trailing slash.
    pub fn base(&self) -> &str {
        &self.base
    }

    pub(crate) async fn subscriptions(&self) -> Result<Vec<Subscription>> {
        #[derive(Deserialize)]
        struct List {
            subscriptions: Vec<Subscription>,
        }
        let list: List = self.json(self.get("subscription/list")).await?;
        Ok(list.subscriptions)
    }

    /// Every item ID in `stream`, minus those in `exclude`, and only those newer than
    /// `since` (Unix seconds) if given. Servers differ on what newer means: FreshRSS
    /// compares the crawl time, Miniflux the publish date. Malformed IDs are skipped.
    pub(crate) async fn item_ids(
        &self,
        stream: &str,
        exclude: Option<&str>,
        since: Option<i64>,
    ) -> Result<Vec<String>> {
        #[derive(Deserialize)]
        struct Page {
            #[serde(rename = "itemRefs", default)]
            item_refs: Vec<ItemRef>,
            continuation: Option<String>,
        }
        #[derive(Deserialize)]
        struct ItemRef {
            id: String,
        }

        let mut ids = Vec::new();
        let mut continuation: Option<String> = None;
        loop {
            let mut req = self
                .get("stream/items/ids")
                .query(&[("s", stream), ("n", IDS_PAGE)]);
            if let Some(x) = exclude {
                req = req.query(&[("xt", x)]);
            }
            if let Some(t) = since {
                req = req.query(&[("ot", t.to_string())]);
            }
            if let Some(c) = &continuation {
                req = req.query(&[("c", c)]);
            }
            let page: Page = self.json(req).await?;
            ids.extend(page.item_refs.iter().filter_map(|r| parse_item_id(&r.id)));
            match next_page(page.continuation, &continuation) {
                Some(c) => continuation = Some(c),
                None => return Ok(ids),
            }
        }
    }

    /// Full articles for the given IDs.
    pub(crate) async fn items_contents(&self, ids: &[String]) -> Result<Vec<Item>> {
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            items: Vec<Item>,
        }
        let form: Vec<_> = ids.iter().map(|id| ("i", long_item_id(id))).collect();
        let req = self.post("stream/items/contents").form(&form);
        let page: Page = self.json(req).await?;
        Ok(page.items)
    }

    /// The short-lived token that write calls need.
    pub(crate) async fn write_token(&self) -> Result<String> {
        let res = self.get("token").send().await?;
        check(res.status())?;
        Ok(res.text().await?.trim().to_owned())
    }

    /// Adds or removes a state tag, such as [`READ`], on the given items.
    pub(crate) async fn edit_tag(
        &self,
        write_token: &str,
        ids: &[String],
        tag: &str,
        add: bool,
    ) -> Result<()> {
        let mut form: Vec<_> = ids.iter().map(|id| ("i", long_item_id(id))).collect();
        form.push((if add { "a" } else { "r" }, tag.to_owned()));
        form.push(("T", write_token.to_owned()));
        let res = self.post("edit-tag").form(&form).send().await?;
        check(res.status())?;
        let body = res.text().await?;
        if body.trim() != "OK" {
            let body: String = body.chars().take(100).collect();
            return Err(Error::Protocol(format!("edit-tag answered {body:?}")));
        }
        Ok(())
    }

    /// Resolves a URL the server sent, which may be relative to the API URL.
    pub(crate) fn resolve(&self, url: &str) -> Option<Url> {
        Url::parse(&format!("{}/", self.base))
            .ok()?
            .join(url)
            .ok()
            .filter(|u| matches!(u.scheme(), "http" | "https"))
    }

    fn get(&self, path: &str) -> RequestBuilder {
        self.authed(self.http.get(self.url(path)))
            .query(&[("output", "json")])
    }

    fn post(&self, path: &str) -> RequestBuilder {
        self.authed(self.http.post(self.url(path)))
    }

    fn url(&self, path: &str) -> String {
        format!("{}/reader/api/0/{path}", self.base)
    }

    fn authed(&self, req: RequestBuilder) -> RequestBuilder {
        req.header(AUTHORIZATION, format!("GoogleLogin auth={}", self.token))
    }

    async fn json<T: DeserializeOwned>(&self, req: RequestBuilder) -> Result<T> {
        let res = req.send().await?;
        check(res.status())?;
        Ok(res.json().await?)
    }
}

fn check(status: StatusCode) -> Result<()> {
    match status {
        StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN => Err(Error::Unauthorized),
        s if s.is_success() => Ok(()),
        s => Err(Error::Protocol(format!("HTTP {s}"))),
    }
}

/// The continuation for the next page, or `None` at the end. A server that repeats the
/// same continuation would otherwise loop forever.
pub(crate) fn next_page(next: Option<String>, current: &Option<String>) -> Option<String> {
    next.filter(|c| !c.is_empty() && Some(c) != current.as_ref())
}

/// An item ID as the cache keeps it. Servers send IDs in a short and a long form, and a
/// sync needs both to name the same item. 64-bit IDs (FreshRSS, Miniflux) come as decimal
/// (`"1730000000000001"`) or as 16 hex digits in the long form
/// (`"tag:google.com,2005:reader/item/0006246b5f1e0001"`), and are kept as signed
/// decimal. Longer ones, such as The Old Reader's 24-digit ObjectIds, are hex in both
/// forms, and are kept as lowercase hex. None of those is a valid decimal `i64`, so the
/// two kinds can't be mixed up.
pub(crate) fn parse_item_id(id: &str) -> Option<String> {
    let id = id.trim();
    match id.strip_prefix(LONG_ID_PREFIX) {
        Some(hex) if hex.len() == 16 => u64::from_str_radix(hex, 16)
            .ok()
            .map(|v| (v as i64).to_string()),
        Some(hex) => hex_id(hex),
        None => decimal_id(id).map(|v| v.to_string()).or_else(|| hex_id(id)),
    }
}

/// Decimal only if it reads back the same, so `"0012"` is hex. Unsigned 64-bit IDs
/// wrap, as in the long form.
fn decimal_id(id: &str) -> Option<i64> {
    if let Ok(v) = id.parse::<i64>()
        && v.to_string() == id
    {
        return Some(v);
    }
    id.parse::<u64>()
        .ok()
        .filter(|v| v.to_string() == id)
        .map(|v| v as i64)
}

fn hex_id(hex: &str) -> Option<String> {
    (!hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()))
        .then(|| hex.to_ascii_lowercase())
}

/// The long form of an ID from [`parse_item_id`].
fn long_item_id(id: &str) -> String {
    match id.parse::<i64>() {
        Ok(v) => format!("{LONG_ID_PREFIX}{:016x}", v as u64),
        Err(_) => format!("{LONG_ID_PREFIX}{id}"),
    }
}

#[derive(Deserialize)]
pub(crate) struct Subscription {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub categories: Vec<Category>,
    /// FreshRSS only. `"feed"` means the feed is left out of the reading list and shows
    /// only on its own.
    #[serde(rename = "frss:priority")]
    pub priority: Option<String>,
    /// The feed's favicon. FreshRSS and Miniflux send it.
    #[serde(rename = "iconUrl")]
    pub icon_url: Option<String>,
    /// The feed's own address, and its web site's, for an OPML export.
    pub url: Option<String>,
    #[serde(rename = "htmlUrl")]
    pub html_url: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct Category {
    pub id: String,
    pub label: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct Item {
    pub id: String,
    #[serde(rename = "crawlTimeMsec")]
    pub crawl_time_msec: Option<String>,
    /// When the server got the item, in microseconds. For servers that don't send
    /// `crawlTimeMsec`.
    #[serde(rename = "timestampUsec")]
    pub timestamp_usec: Option<String>,
    /// Unix seconds.
    #[serde(default)]
    pub published: i64,
    pub title: Option<String>,
    pub author: Option<String>,
    #[serde(default)]
    pub canonical: Vec<Link>,
    #[serde(default)]
    pub alternate: Vec<Link>,
    pub summary: Option<Content>,
    pub content: Option<Content>,
    pub origin: Origin,
    #[serde(default)]
    pub categories: Vec<String>,
}

#[derive(Deserialize)]
pub(crate) struct Link {
    pub href: String,
}

#[derive(Deserialize)]
pub(crate) struct Content {
    pub content: String,
}

#[derive(Deserialize)]
pub(crate) struct Origin {
    #[serde(rename = "streamId")]
    pub stream_id: String,
    pub title: Option<String>,
}

impl Item {
    /// When the server got the item, in Unix milliseconds.
    pub fn crawl_time(&self) -> Option<i64> {
        let ms = self.crawl_time_msec.as_deref().and_then(|t| t.parse().ok());
        ms.or_else(|| {
            let us: i64 = self.timestamp_usec.as_deref()?.parse().ok()?;
            Some(us / 1000)
        })
    }

    /// Servers write states as `user/-/state/...` or `user/<id>/state/...`.
    pub fn has_state(&self, state: &str) -> bool {
        let suffix = state.trim_start_matches("user/-");
        self.categories
            .iter()
            .any(|c| c.starts_with("user/") && c.ends_with(suffix))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(id: &str) -> Option<String> {
        parse_item_id(id)
    }

    #[test]
    fn parses_both_64_bit_item_id_forms() {
        assert_eq!(parsed("1730000000000001"), Some("1730000000000001".into()));
        assert_eq!(
            parsed("tag:google.com,2005:reader/item/0006256d03762001"),
            Some("1730000000000001".into())
        );
        assert_eq!(
            long_item_id("1730000000000001"),
            "tag:google.com,2005:reader/item/0006256d03762001"
        );
        assert_eq!(
            parsed("tag:google.com,2005:reader/item/ffffffffffffffff"),
            Some("-1".into())
        );
        assert_eq!(parsed("18446744073709551615"), Some("-1".into()));
        assert_eq!(parsed("-1"), Some("-1".into()));
        assert_eq!(
            long_item_id("-1"),
            "tag:google.com,2005:reader/item/ffffffffffffffff"
        );
        assert_eq!(parsed("nope"), None);
        assert_eq!(parsed(""), None);
    }

    #[test]
    fn keeps_longer_item_ids_as_hex() {
        let long = "tag:google.com,2005:reader/item/5D0CFA30D1892A8D3F00002E";
        assert_eq!(parsed(long), Some("5d0cfa30d1892a8d3f00002e".into()));
        assert_eq!(
            parsed("5d0cfa30d1892a8d3f00002e"),
            Some("5d0cfa30d1892a8d3f00002e".into())
        );
        // All digits, but too long for 64 bits: still hex, as in the long form.
        assert_eq!(
            parsed("001234567890123456789012"),
            Some("001234567890123456789012".into())
        );
        assert_eq!(
            long_item_id("001234567890123456789012"),
            "tag:google.com,2005:reader/item/001234567890123456789012"
        );
        assert_eq!(
            long_item_id("5d0cfa30d1892a8d3f00002e"),
            "tag:google.com,2005:reader/item/5d0cfa30d1892a8d3f00002e"
        );
    }

    #[test]
    fn rejects_credentials_in_the_url() {
        assert!(Client::new("https://carl:pw@rss.example.com/api/greader.php", "").is_err());
        assert!(Client::new("ftp://rss.example.com", "").is_err());
        assert!(Client::new("rss.example.com", "").is_err());
        assert!(Client::new("http://192.168.1.2/api/greader.php", "").is_ok());
    }
}
