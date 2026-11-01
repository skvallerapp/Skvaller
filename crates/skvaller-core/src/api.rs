//! Google Reader API client. Tested against FreshRSS; the same calls work with Miniflux,
//! Inoreader, BazQux and The Old Reader.

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

/// Full articles per `stream/contents` page.
const CONTENTS_PAGE: &str = "1000";
/// IDs per `stream/items/ids` page. IDs are small, so fewer and larger pages. FreshRSS
/// drops one item at each page boundary when the boundary item changed between requests,
/// so fewer boundaries also means fewer wrong results.
const IDS_PAGE: &str = "10000";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Time without any data arriving. A large page may take longer than this in total.
const READ_TIMEOUT: Duration = Duration::from_secs(30);
const LONG_ID_PREFIX: &str = "tag:google.com,2005:reader/item/";

#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    base: String,
    token: String,
}

impl Client {
    /// Signs in with `ClientLogin`. `base` is the API URL, such as
    /// `https://rss.example.com/api/greader.php` for FreshRSS.
    pub async fn login(base: &str, username: &str, password: &str) -> Result<Self> {
        let mut client = Self::new(base, "")?;
        let res = client
            .http
            .post(format!("{}/accounts/ClientLogin", client.base))
            .form(&[("Email", username), ("Passwd", password)])
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

    pub(crate) async fn subscriptions(&self) -> Result<Vec<Subscription>> {
        #[derive(Deserialize)]
        struct List {
            subscriptions: Vec<Subscription>,
        }
        let list: List = self.json(self.get("subscription/list")).await?;
        Ok(list.subscriptions)
    }

    /// One page of full articles from `stream`, crawled at or after `since` (Unix seconds).
    pub(crate) async fn stream_contents(
        &self,
        stream: &str,
        since: i64,
        continuation: Option<&str>,
    ) -> Result<StreamPage> {
        let mut req = self
            .get(&format!("stream/contents/{stream}"))
            .query(&[("n", CONTENTS_PAGE), ("ot", &since.to_string())]);
        if let Some(c) = continuation {
            req = req.query(&[("c", c)]);
        }
        self.json(req).await
    }

    /// Every item ID in `stream`, minus those in `exclude`. Malformed IDs are skipped.
    pub(crate) async fn item_ids(&self, stream: &str, exclude: Option<&str>) -> Result<Vec<i64>> {
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
    pub(crate) async fn items_contents(&self, ids: &[i64]) -> Result<Vec<Item>> {
        let form: Vec<_> = ids.iter().map(|&id| ("i", long_item_id(id))).collect();
        let req = self.post("stream/items/contents").form(&form);
        let page: StreamPage = self.json(req).await?;
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
        ids: &[i64],
        tag: &str,
        add: bool,
    ) -> Result<()> {
        let mut form: Vec<_> = ids.iter().map(|&id| ("i", long_item_id(id))).collect();
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

/// Item IDs come as decimal (`"1730000000000001"`) or in the long hex form
/// (`"tag:google.com,2005:reader/item/0006246b5f1e0001"`).
pub(crate) fn parse_item_id(id: &str) -> Option<i64> {
    match id.strip_prefix(LONG_ID_PREFIX) {
        Some(hex) => u64::from_str_radix(hex, 16).map(|v| v as i64).ok(),
        None => id.parse().ok(),
    }
}

fn long_item_id(id: i64) -> String {
    format!("{LONG_ID_PREFIX}{:016x}", id as u64)
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
}

#[derive(Deserialize)]
pub(crate) struct Category {
    pub id: String,
    pub label: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct StreamPage {
    #[serde(default)]
    pub items: Vec<Item>,
    pub continuation: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct Item {
    pub id: String,
    #[serde(rename = "crawlTimeMsec")]
    pub crawl_time_msec: Option<String>,
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

    #[test]
    fn parses_both_item_id_forms() {
        assert_eq!(parse_item_id("1730000000000001"), Some(1730000000000001));
        assert_eq!(
            parse_item_id(&long_item_id(1730000000000001)),
            Some(1730000000000001)
        );
        assert_eq!(
            parse_item_id("tag:google.com,2005:reader/item/ffffffffffffffff"),
            Some(-1)
        );
        assert_eq!(parse_item_id("nope"), None);
    }

    #[test]
    fn rejects_credentials_in_the_url() {
        assert!(Client::new("https://carl:pw@rss.example.com/api/greader.php", "").is_err());
        assert!(Client::new("ftp://rss.example.com", "").is_err());
        assert!(Client::new("rss.example.com", "").is_err());
        assert!(Client::new("http://192.168.1.2/api/greader.php", "").is_ok());
    }
}
