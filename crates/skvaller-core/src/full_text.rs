//! Reader View: the full text of an article, extracted from its web page with
//! dom_smoothie, a port of Mozilla's Readability.js. It is as untrusted as the page.

use std::sync::OnceLock;
use std::time::Duration;

use encoding_rs::Encoding;
use reqwest::header::CONTENT_TYPE;
use reqwest::{StatusCode, Url};

use crate::db::FullText;
use crate::html;
use crate::{Db, Result};

/// Failed fetches of a page before it is given up on, until the user asks again.
pub(crate) const MAX_ATTEMPTS: u32 = 3;
/// Pages fetched ahead per sync, at most. The rest wait for the next sync.
const PREFETCH_PAGES: usize = 30;
/// Larger pages are not read. A long article page is a few hundred kilobytes.
const MAX_PAGE_BYTES: usize = 2 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(15);
/// Browser-like, since some sites turn away anything else, but says what it is.
pub(crate) const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (compatible; Skvaller/",
    env!("CARGO_PKG_VERSION"),
    "; +https://github.com/skvallerapp/Skvaller)"
);

/// The article's full text: from the cache, or else fetched from its web page and
/// cached. `Ok(None)`: the page has nothing readable, or the article is gone. A page
/// given up on is fetched again only with `retry`. `Err`: try again later.
pub async fn load(db: &Db, id: &str, retry: bool) -> Result<Option<String>> {
    match db.full_text(id)? {
        Some(FullText::Html(html)) => return Ok(Some(html)),
        Some(FullText::Failed(attempts)) if attempts >= MAX_ATTEMPTS && !retry => {
            return Ok(None);
        }
        _ => {}
    }
    let Some(url) = db.article_url(id)? else {
        return Ok(None);
    };
    let page = match fetch_page(&http()?, &url).await {
        Ok(page) => page,
        Err(e) => {
            db.full_text_failed(id, false)?;
            return Err(e);
        }
    };
    let html = match page {
        // A panic in the extractor counts as nothing readable.
        Some((html, url)) => tokio::task::spawn_blocking(move || extract(&html, &url))
            .await
            .ok()
            .flatten(),
        None => None,
    };
    match &html {
        Some(html) => db.set_full_text(id, html)?,
        None => db.full_text_failed(id, true)?,
    }
    Ok(html)
}

/// Fetches the full text of unread articles in Reader View feeds, so it is there
/// offline. Pages that fail are skipped; only cache errors stop it.
pub async fn prefetch(db: &Db) -> Result<()> {
    for id in db.full_text_wanted(PREFETCH_PAGES)? {
        if let Err(e @ crate::Error::Db(_)) = load(db, &id, false).await {
            return Err(e);
        }
    }
    Ok(())
}

/// One client for every page, so connections are reused. Not the API client: pages
/// redirect, and they get no access token. It keeps no cookies.
fn http() -> Result<reqwest::Client> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let client = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(USER_AGENT)
        .build()?;
    Ok(CLIENT.get_or_init(|| client).clone())
}

/// The HTML of the page at `url`, and its URL after redirects. `Ok(None)`: it will
/// never be readable (not an http(s) URL, gone, not HTML, too large). `Err`: it may
/// work later (network errors, server errors, rate limits and bot walls).
async fn fetch_page(http: &reqwest::Client, url: &str) -> Result<Option<(String, String)>> {
    let Ok(url) = Url::parse(url) else {
        return Ok(None);
    };
    if !matches!(url.scheme(), "http" | "https") {
        return Ok(None);
    }
    let mut res = http.get(url).send().await?;
    let status = res.status();
    let transient = [
        StatusCode::FORBIDDEN,
        StatusCode::REQUEST_TIMEOUT,
        StatusCode::TOO_EARLY,
        StatusCode::TOO_MANY_REQUESTS,
    ];
    if status.is_server_error() || transient.contains(&status) {
        return Err(res.error_for_status().expect_err("an error status").into());
    }
    let content_type = res
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = content_type.split(';').next().unwrap_or("").trim();
    let is_html = matches!(mime, "text/html" | "application/xhtml+xml");
    let too_large = res
        .content_length()
        .is_some_and(|n| n > MAX_PAGE_BYTES as u64);
    if !status.is_success() || !is_html || too_large {
        return Ok(None);
    }
    let final_url = res.url().to_string();
    let mut body = Vec::new();
    while let Some(chunk) = res.chunk().await? {
        body.extend_from_slice(&chunk);
        if body.len() > MAX_PAGE_BYTES {
            return Ok(None);
        }
    }
    Ok(Some((decode(&body, &content_type), final_url)))
}

/// Decodes like a browser, mostly: a byte order mark, else the Content-Type charset,
/// else a `<meta>` charset in the first 1024 bytes, else UTF-8.
pub(crate) fn decode(body: &[u8], content_type: &str) -> String {
    let encoding = content_type
        .split(';')
        .skip(1)
        .find_map(|param| {
            let (name, value) = param.split_once('=')?;
            (name.trim() == "charset").then(|| label(value.trim().trim_matches(['"', '\''])))?
        })
        .or_else(|| meta_charset(&body[..body.len().min(1024)]))
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(body).0.into_owned()
}

/// The charset of a `<meta charset>` or `<meta http-equiv="Content-Type">` tag. A rough
/// search for `charset=`, not the HTML spec's full prescan.
fn meta_charset(head: &[u8]) -> Option<&'static Encoding> {
    let head = String::from_utf8_lossy(head).to_ascii_lowercase();
    let mut rest = head.as_str();
    while let Some(i) = rest.find("charset") {
        rest = &rest[i + "charset".len()..];
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start().trim_start_matches(['"', '\'']);
        let end = value
            .find(|c: char| c.is_whitespace() || "\"';>/".contains(c))
            .unwrap_or(value.len());
        if let Some(encoding) = label(&value[..end]) {
            // UTF-16 becomes UTF-8: a page whose meta tag reads as ASCII isn't UTF-16.
            return Some(encoding.output_encoding());
        }
    }
    None
}

fn label(name: &str) -> Option<&'static Encoding> {
    Encoding::for_label(name.as_bytes())
}

/// The main content of `html`, the page at `url`, as HTML with absolute links. `None`
/// if nothing readable was found, or the page nests too deeply to read in good time.
/// Takes milliseconds to tens of milliseconds: keep it off the async runtime.
fn extract(html: &str, url: &str) -> Option<String> {
    let document = html::parse(html)?;
    // Errors are a bad URL or no content: nothing readable either way.
    let mut readability =
        dom_smoothie::Readability::with_document(document, Some(url), None).ok()?;
    let article = readability.parse().ok()?;
    if article.text_content.trim().is_empty() {
        return None;
    }
    Some(article.content.to_string())
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;
    use crate::db::ArticleRow;

    const ARTICLE: &str = include_str!("../testdata/article.html");

    #[test]
    fn extracts_the_article_without_the_page_around_it() {
        let html = extract(ARTICLE, "https://example.com/posts/kernel/").unwrap();

        assert!(html.contains("The scheduler picks the next task"));
        assert!(html.contains("lock-free run queue"));
        assert!(!html.contains("Subscribe to our newsletter"));
        assert!(!html.contains("<script"));
        assert!(!html.contains("Related posts"));
        // Relative URLs become absolute: the reader frame has no base URL.
        assert!(html.contains("https://example.com/posts/kernel/diagram.png"));
        assert!(html.contains("https://example.com/about"));
    }

    #[test]
    fn a_page_without_text_has_nothing_to_extract() {
        let html = "<html><head><title>Empty</title></head><body><nav></nav></body></html>";
        assert_eq!(extract(html, "https://example.com/"), None);
    }

    #[test]
    fn deeply_nested_pages_are_given_up_on_quickly() {
        let nested = |n| {
            format!(
                "<html><body>{}<p>{}</p>{}</body></html>",
                "<div>".repeat(n),
                "Some text. ".repeat(100),
                "</div>".repeat(n)
            )
        };
        let start = Instant::now();
        assert_eq!(extract(&nested(400_000), "https://example.com/"), None);
        assert_eq!(
            extract(&nested(html::MAX_DEPTH + 10), "https://example.com/"),
            None
        );
        // Rebuilt formatting elements nest as deep as divs do.
        assert_eq!(
            extract(&"<b><div></b>".repeat(100_000), "https://example.com/"),
            None
        );
        assert!(
            start.elapsed() < Duration::from_secs(3),
            "{:?}",
            start.elapsed()
        );
        // Real pages nest far less.
        let icons = "<svg><path d=\"M0\"/><circle r=\"1\"/></svg>".repeat(3000);
        let text = "Some text. ".repeat(100);
        let page = format!("<html><body><article>{icons}<p>{text}</p></article></body></html>");
        assert!(extract(&page, "https://example.com/").is_some());
        assert!(extract(&nested(50), "https://example.com/").is_some());
    }

    #[test]
    fn decodes_with_the_header_charset_then_the_meta_charset() {
        assert_eq!(decode(b"caf\xe9", "text/html; charset=ISO-8859-1"), "café");
        assert_eq!(
            decode(b"caf\xe9", "text/html; charset = \"windows-1252\""),
            "café"
        );
        assert_eq!(decode("café".as_bytes(), "text/html"), "café");
        let meta = b"<html><head><meta charset=\"windows-1252\"></head><body>r\xe4ksm\xf6rg\xe5s";
        assert!(decode(meta, "text/html").ends_with("räksmörgås"));
        let http_equiv =
            b"<meta http-equiv=\"Content-Type\" content=\"text/html; charset=iso-8859-1\">\xe5";
        assert!(decode(http_equiv, "text/html").ends_with('å'));
        // The header wins over the meta tag.
        let both = "<meta charset=\"windows-1252\">å".as_bytes();
        assert!(decode(both, "text/html; charset=utf-8").ends_with('å'));
        // A meta tag can't make a page UTF-16.
        assert_eq!(
            meta_charset(b"<meta charset=utf-16>"),
            Some(encoding_rs::UTF_8)
        );
    }

    async fn serve(server: &MockServer, at: &str, response: ResponseTemplate) -> String {
        Mock::given(method("GET"))
            .and(path(at))
            .respond_with(response)
            .mount(server)
            .await;
        format!("{}{at}", server.uri())
    }

    fn html_page(body: &str) -> ResponseTemplate {
        // Not set_body_string: it would replace the content type.
        ResponseTemplate::new(200).set_body_raw(body.to_owned(), "text/html; charset=utf-8")
    }

    #[tokio::test]
    async fn relative_urls_resolve_against_the_page_after_redirects() {
        let server = MockServer::start().await;
        let target = serve(&server, "/posts/kernel/", html_page(ARTICLE)).await;
        let link = serve(
            &server,
            "/r/123",
            ResponseTemplate::new(301).insert_header("location", target.as_str()),
        )
        .await;

        let (html, url) = fetch_page(&http().unwrap(), &link).await.unwrap().unwrap();
        assert_eq!(url, target);
        let extracted = extract(&html, &url).unwrap();
        assert!(extracted.contains(&format!("{target}diagram.png")));
    }

    #[tokio::test]
    async fn pages_that_are_not_html_gone_or_too_large_are_never_readable() {
        let server = MockServer::start().await;
        let http = http().unwrap();
        let big = "x".repeat(MAX_PAGE_BYTES + 1);
        let pages = [
            ResponseTemplate::new(200).set_body_raw("x", "application/pdf"),
            ResponseTemplate::new(404).set_body_raw("x", "text/html"),
            html_page(&big),
        ];
        for (i, response) in pages.into_iter().enumerate() {
            let url = serve(&server, &format!("/{i}"), response).await;
            assert!(
                matches!(fetch_page(&http, &url).await, Ok(None)),
                "page {i}"
            );
        }
        assert!(matches!(
            fetch_page(&http, "ftp://example.com/").await,
            Ok(None)
        ));
        assert!(matches!(fetch_page(&http, "not a url").await, Ok(None)));
    }

    #[tokio::test]
    async fn server_errors_rate_limits_and_bot_walls_may_work_later() {
        let server = MockServer::start().await;
        let http = http().unwrap();
        for status in [503, 429, 403] {
            let url = serve(
                &server,
                &format!("/{status}"),
                ResponseTemplate::new(status),
            )
            .await;
            let result = fetch_page(&http, &url).await;
            assert!(matches!(result, Err(crate::Error::Http(_))), "{status}");
        }
    }

    fn article(id: i64) -> ArticleRow {
        ArticleRow {
            id: id.to_string(),
            feed_id: "feed/1".into(),
            feed_title: String::new(),
            title: String::new(),
            author: String::new(),
            url: String::new(),
            published: id,
            read: false,
            starred: false,
            summary: String::new(),
            html: String::new(),
        }
    }

    fn db_with_page(url: &str) -> Db {
        let db = Db::open_in_memory().unwrap();
        db.upsert_articles(&[ArticleRow {
            url: url.into(),
            ..article(1)
        }])
        .unwrap();
        db
    }

    #[tokio::test]
    async fn loads_once_then_from_the_cache() {
        let server = MockServer::start().await;
        Mock::given(path("/post"))
            .respond_with(html_page(ARTICLE))
            .expect(1)
            .mount(&server)
            .await;
        let db = db_with_page(&format!("{}/post", server.uri()));

        for _ in 0..2 {
            let html = load(&db, "1", false).await.unwrap().unwrap();
            assert!(html.contains("The scheduler picks the next task"));
        }
        assert_eq!(load(&db, "2", false).await.unwrap(), None);
    }

    #[tokio::test]
    async fn gives_up_after_failing_until_asked_again() {
        let server = MockServer::start().await;
        Mock::given(path("/post"))
            .respond_with(ResponseTemplate::new(503))
            .expect(MAX_ATTEMPTS as u64 + 1)
            .mount(&server)
            .await;
        let db = db_with_page(&format!("{}/post", server.uri()));

        for _ in 0..MAX_ATTEMPTS {
            assert!(load(&db, "1", false).await.is_err());
        }
        assert_eq!(load(&db, "1", false).await.unwrap(), None);
        assert!(load(&db, "1", true).await.is_err());
    }

    #[tokio::test]
    async fn a_page_with_nothing_readable_is_not_fetched_again() {
        let server = MockServer::start().await;
        Mock::given(path("/post"))
            .respond_with(ResponseTemplate::new(404))
            .expect(2)
            .mount(&server)
            .await;
        let db = db_with_page(&format!("{}/post", server.uri()));

        assert_eq!(load(&db, "1", false).await.unwrap(), None);
        assert_eq!(load(&db, "1", false).await.unwrap(), None);
        assert_eq!(load(&db, "1", true).await.unwrap(), None);
    }

    #[tokio::test]
    async fn prefetches_unread_articles_of_reader_view_feeds() {
        let server = MockServer::start().await;
        let pages = [
            (1, "feed/1", html_page(ARTICLE), 1),
            (2, "feed/1", ResponseTemplate::new(503), 2), // tried again
            (3, "feed/1", ResponseTemplate::new(404), 1), // never readable
            (4, "feed/2", html_page(ARTICLE), 0),         // not in Reader View
        ];
        let db = Db::open_in_memory().unwrap();
        for (id, feed_id, response, times) in pages {
            let at = format!("/page/{id}");
            Mock::given(path(&at))
                .respond_with(response)
                .expect(times)
                .mount(&server)
                .await;
            db.upsert_articles(&[ArticleRow {
                feed_id: feed_id.into(),
                url: format!("{}{at}", server.uri()),
                ..article(id)
            }])
            .unwrap();
        }
        db.set_feed_reader_view("feed/1", true).unwrap();

        prefetch(&db).await.unwrap();
        prefetch(&db).await.unwrap();

        assert!(matches!(
            db.full_text("1").unwrap(),
            Some(FullText::Html(_))
        ));
        assert_eq!(db.full_text("2").unwrap(), Some(FullText::Failed(2)));
        assert_eq!(db.full_text("4").unwrap(), None);
    }
}
