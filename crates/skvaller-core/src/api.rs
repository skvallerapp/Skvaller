//! Google Reader API client. It uses only the calls that FreshRSS, Miniflux and The Old
//! Reader all have: Miniflux has no `stream/contents`, so articles come by ID.

use std::time::Duration;

use reqwest::header::{AUTHORIZATION, LOCATION};
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
/// Marks a cached ID longer than 64 bits. See [`parse_item_id`].
const HEX_ID: &str = "x";

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
}

/// Shows the server, never the token.
impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("base", &self.base)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Signs in with `ClientLogin`. `base` is the API URL: the server's address for
    /// Miniflux and The Old Reader, `https://rss.example.com/api/greader.php` for
    /// FreshRSS. An address without FreshRSS's path is tried as typed, then with it.
    /// [`Client::base`] tells which one answered.
    ///
    /// # Errors
    ///
    /// Returns `Error::Unauthorized` for a wrong username or password, `Error::BadUrl`
    /// for an address that isn't http or https or holds a username or password,
    /// `Error::Http` when the server can't be reached, and `Error::Protocol` for an
    /// unexpected answer.
    pub async fn login(base: &str, username: &str, password: &str) -> Result<Self> {
        let base = base.trim_end_matches('/');
        let redirect = match Self::login_at(base, username, password).await? {
            Ok(client) => return Ok(client),
            Err(redirect) => redirect,
        };
        if !base.ends_with(FRESHRSS_PATH) {
            let freshrss = format!("{base}{FRESHRSS_PATH}");
            if let Ok(client) = Self::login_at(&freshrss, username, password).await? {
                return Ok(client);
            }
        }
        // Redirects aren't followed, but where to is the likely fix, such as https.
        Err(Error::Protocol(match redirect {
            Some(to) => format!("no Google Reader API at this address; it redirects to {to}"),
            None => "no Google Reader API at this address".into(),
        }))
    }

    /// `Err` if there is no API at `base`: a redirect (with where to, if given), Not
    /// Found, or a page without a token, such as a web site's own. Other errors are the
    /// server's answer.
    async fn login_at(
        base: &str,
        username: &str,
        password: &str,
    ) -> Result<Result<Self, Option<String>>> {
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
        let status = res.status();
        if status.is_redirection() {
            let to = res.headers().get(LOCATION).and_then(|v| v.to_str().ok());
            return Ok(Err(to.map(|to| to.chars().take(200).collect())));
        }
        if matches!(
            status,
            StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
        ) {
            return Ok(Err(None));
        }
        // ClientLogin answers wrong credentials with 403. Elsewhere a 403 more likely
        // comes from a proxy or rate limit, so `check` doesn't treat it as signed out.
        if status == StatusCode::FORBIDDEN {
            return Err(Error::Unauthorized);
        }
        check(status)?;
        let body = res.text().await?;
        let Some(token) = body
            .lines()
            .find_map(|line| line.strip_prefix("Auth="))
            .filter(|token| !token.is_empty())
        else {
            return Ok(Err(None));
        };
        token.clone_into(&mut client.token);
        Ok(Ok(client))
    }

    /// Builds a client from a token saved by an earlier [`Client::login`].
    ///
    /// # Errors
    ///
    /// Returns `Error::BadUrl` for an address that isn't http or https or holds a
    /// username or password, and `Error::Http` if the HTTP client can't be built.
    pub fn new(base: &str, token: &str) -> Result<Self> {
        #[expect(
            clippy::map_err_ignore,
            reason = "BadUrl is what the user needs; the parser's error adds nothing"
        )]
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
    #[must_use]
    pub fn token(&self) -> &str {
        &self.token
    }

    /// The API URL, without a trailing slash.
    #[must_use]
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
            // An empty page ends the stream too, in case a server keeps handing out new
            // continuations with nothing behind them.
            if page.item_refs.is_empty() {
                return Ok(ids);
            }
            ids.extend(page.item_refs.iter().filter_map(|r| parse_item_id(&r.id)));
            match next_page(page.continuation, continuation.as_ref()) {
                Some(c) => continuation = Some(c),
                None => return Ok(ids),
            }
        }
    }

    /// Full articles for the given IDs. Items that don't parse are left out, so one
    /// malformed item can't fail every sync.
    pub(crate) async fn items_contents(&self, ids: &[String]) -> Result<Vec<Item>> {
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            items: Vec<serde_json::Value>,
        }
        let form: Vec<_> = ids
            .iter()
            .filter_map(|id| Some(("i", long_item_id(id)?)))
            .collect();
        if form.is_empty() {
            return Ok(Vec::new());
        }
        let req = self.post("stream/items/contents").form(&form);
        let page: Page = self.json(req).await?;
        Ok(page
            .items
            .into_iter()
            .filter_map(|item| serde_json::from_value(item).ok())
            .collect())
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
        let mut form: Vec<_> = ids
            .iter()
            .filter_map(|id| Some(("i", long_item_id(id)?)))
            .collect();
        if form.is_empty() {
            return Ok(());
        }
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

    /// The server's address, which the user chose.
    pub(crate) fn base_url(&self) -> Option<Url> {
        Url::parse(&self.base).ok()
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
        // A body that isn't the expected JSON, such as a proxy's sign-in page, is the
        // server's fault, not the network's.
        let body = res.bytes().await?;
        serde_json::from_slice(&body).map_err(|e| Error::Protocol(e.to_string()))
    }
}

fn check(status: StatusCode) -> Result<()> {
    match status {
        StatusCode::UNAUTHORIZED => Err(Error::Unauthorized),
        s if s.is_success() => Ok(()),
        s => Err(Error::Protocol(format!("HTTP {s}"))),
    }
}

/// The continuation for the next page, or `None` at the end. A server that repeats the
/// same continuation would otherwise loop forever.
pub(crate) fn next_page(next: Option<String>, current: Option<&String>) -> Option<String> {
    next.filter(|c| !c.is_empty() && Some(c) != current)
}

/// An item ID as the cache keeps it. Servers send IDs in a short and a long form, and a
/// sync needs both to name the same item. 64-bit IDs (FreshRSS, Miniflux) come as decimal
/// (`"1730000000000001"`) or as up to 16 hex digits in the long form
/// (`"tag:google.com,2005:reader/item/0006246b5f1e0001"`), and are kept as signed
/// decimal. Longer ones, such as The Old Reader's 24-digit ObjectIds, are hex in both
/// forms, and are kept as `x` and lowercase hex, so they can't read as a decimal ID.
pub(crate) fn parse_item_id(id: &str) -> Option<String> {
    let id = id.trim();
    match id.strip_prefix(LONG_ID_PREFIX) {
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
        .map(u64::cast_signed)
}

fn hex_id(hex: &str) -> Option<String> {
    if hex.is_empty() || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    if hex.len() <= 16 {
        let v = u64::from_str_radix(hex, 16).ok()?;
        return Some(v.cast_signed().to_string());
    }
    Some(format!("{HEX_ID}{}", hex.to_ascii_lowercase()))
}

/// The long form of an ID from [`parse_item_id`]. `None` for anything else, rather than
/// an ID that names another item.
fn long_item_id(id: &str) -> Option<String> {
    match id.strip_prefix(HEX_ID) {
        Some(hex) => hex_id(hex)
            .is_some()
            .then(|| format!("{LONG_ID_PREFIX}{hex}")),
        None => decimal_id(id).map(|v| format!("{LONG_ID_PREFIX}{:016x}", v.cast_unsigned())),
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
    /// Unix seconds. Some servers send `null` or leave it out.
    pub published: Option<i64>,
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
    /// Media attached to the item, such as its lead image.
    #[serde(default, deserialize_with = "lenient")]
    pub enclosure: Vec<Enclosure>,
}

#[derive(Deserialize)]
pub(crate) struct Enclosure {
    pub href: String,
    #[serde(rename = "type")]
    pub mime: Option<String>,
}

/// `T` if the value has its shape, else its default: an odd shape in an optional field
/// must not fail the whole page of items.
fn lenient<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: DeserializeOwned + Default,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
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
    #[test]
    fn debug_output_leaves_out_the_token() {
        let client = super::Client {
            http: reqwest::Client::new(),
            base: "https://rss.example.com".into(),
            token: "secret-token-123".into(),
        };
        let shown = format!("{client:?}");
        assert!(shown.contains("rss.example.com"));
        assert!(!shown.contains("secret-token-123"));
    }

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
            long_item_id("1730000000000001").as_deref(),
            Some("tag:google.com,2005:reader/item/0006256d03762001")
        );
        assert_eq!(
            parsed("tag:google.com,2005:reader/item/ffffffffffffffff"),
            Some("-1".into())
        );
        assert_eq!(parsed("18446744073709551615"), Some("-1".into()));
        assert_eq!(parsed("-1"), Some("-1".into()));
        assert_eq!(
            long_item_id("-1").as_deref(),
            Some("tag:google.com,2005:reader/item/ffffffffffffffff")
        );
        assert_eq!(parsed("nope"), None);
        // A bare hex ID, as caches from before the `x` have them, names no item.
        assert_eq!(long_item_id("5d0cfa30d1892a8d3f00002e"), None);
        assert_eq!(long_item_id("0012"), None);
        assert_eq!(parsed(""), None);
    }

    #[test]
    fn keeps_longer_item_ids_as_hex() {
        let long = "tag:google.com,2005:reader/item/5D0CFA30D1892A8D3F00002E";
        assert_eq!(parsed(long), Some("x5d0cfa30d1892a8d3f00002e".into()));
        assert_eq!(
            parsed("5d0cfa30d1892a8d3f00002e"),
            Some("x5d0cfa30d1892a8d3f00002e".into())
        );
        // All digits, but too long for 64 bits: still hex, as in the long form.
        assert_eq!(
            parsed("001234567890123456789012"),
            Some("x001234567890123456789012".into())
        );
        assert_eq!(
            long_item_id("x5d0cfa30d1892a8d3f00002e").as_deref(),
            Some("tag:google.com,2005:reader/item/5d0cfa30d1892a8d3f00002e")
        );
        // 18 hex digits that look decimal: not the same item as that decimal ID.
        assert_ne!(
            parsed("tag:google.com,2005:reader/item/123456789012345678"),
            parsed("123456789012345678")
        );
    }

    #[test]
    fn short_long_forms_and_odd_shapes_are_one_item() {
        // A long form without the padding, and a short form in hex.
        assert_eq!(
            parsed("tag:google.com,2005:reader/item/3039"),
            Some("12345".into())
        );
        assert_eq!(parsed("0012"), parsed("tag:google.com,2005:reader/item/12"));
        for long in [
            "tag:google.com,2005:reader/item/0006256d03762001",
            "tag:google.com,2005:reader/item/ffffffffffffffff",
            "tag:google.com,2005:reader/item/3039",
            "tag:google.com,2005:reader/item/00157a17b192950b65be3791",
            "tag:google.com,2005:reader/item/123456789012345678",
        ] {
            let id = parsed(long).unwrap();
            assert_eq!(parsed(&long_item_id(&id).unwrap()), Some(id), "{long}");
        }
    }

    #[test]
    fn rejects_credentials_in_the_url() {
        Client::new("https://carl:pw@rss.example.com/api/greader.php", "").unwrap_err();
        Client::new("ftp://rss.example.com", "").unwrap_err();
        Client::new("rss.example.com", "").unwrap_err();
        Client::new("http://192.168.1.2/api/greader.php", "").unwrap();
    }
}
