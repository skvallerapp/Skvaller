//! The local account against a fake web server.

use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::{Scope, Selection, View};

const NOW: i64 = 1_800_000_000;

fn rss(items: &str) -> String {
    format!(
        r#"<?xml version="1.0"?><rss version="2.0"><channel>
           <title>Kernel Notes</title><link>https://kernel.example/</link>
           {items}</channel></rss>"#
    )
}

const TWO_ITEMS: &str = r#"
    <item><guid>a1</guid><title>First</title><link>/posts/1</link>
          <pubDate>Mon, 02 Nov 2026 10:00:00 GMT</pubDate>
          <description>&lt;p&gt;Hello &lt;b&gt;there&lt;/b&gt;&lt;/p&gt;</description></item>
    <item><guid>a2</guid><title>Second</title><link>https://kernel.example/posts/2</link>
          <pubDate>Tue, 03 Nov 2026 10:00:00 GMT</pubDate></item>"#;

fn xml(body: String) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "application/rss+xml")
        .set_body_string(body)
}

async fn serve(server: &MockServer, at: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(response)
        .mount(server)
        .await;
}

fn all(db: &Db) -> Vec<crate::ArticleSummary> {
    db.articles(&Selection {
        view: View::All,
        scope: Scope::All,
    })
    .unwrap()
}

#[tokio::test]
async fn subscribing_adds_the_feed_with_its_entries_unread() {
    let server = MockServer::start().await;
    serve(&server, "/feed.xml", xml(rss(TWO_ITEMS))).await;
    let db = Db::open_in_memory().unwrap();
    let folder = db.create_folder("Tech").unwrap();

    let url = format!("{}/feed.xml", server.uri());
    let id = subscribe(&db, &url, Some(&folder), NOW).await.unwrap();

    assert_eq!(id, format!("feed/{url}"));
    let feeds = db.feeds().unwrap();
    assert_eq!(feeds.len(), 1);
    assert_eq!(feeds[0].title, "Kernel Notes");
    assert_eq!(feeds[0].folder_id.as_deref(), Some(folder.as_str()));
    assert_eq!(feeds[0].url.as_deref(), Some(url.as_str()));
    assert_eq!(
        feeds[0].site_url.as_deref(),
        Some("https://kernel.example/")
    );
    let articles = all(&db);
    assert_eq!(articles.len(), 2);
    assert!(articles.iter().all(|a| !a.read && !a.starred));
    assert_eq!(articles[0].title, "Second");
    assert_eq!(articles[1].url, format!("{}/posts/1", server.uri()));
    assert_eq!(articles[1].summary, "Hello there");
    assert_eq!(articles[1].id, article_id(&id, "a1").to_string());

    // Again: the same feed, in the folder it has.
    assert_eq!(subscribe(&db, &url, None, NOW).await.unwrap(), id);
    assert_eq!(
        db.feeds().unwrap()[0].folder_id.as_deref(),
        Some(folder.as_str())
    );
}

#[tokio::test]
async fn a_web_page_leads_to_the_feed_it_links_to() {
    let server = MockServer::start().await;
    let page = r#"<html><head><title>Kernel</title>
        <link rel="stylesheet" href="/s.css">
        <link rel="alternate" type="application/atom+xml" href="/atom.xml">
        </head><body>Hi</body></html>"#;
    serve(
        &server,
        "/",
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/html; charset=utf-8")
            .set_body_string(page),
    )
    .await;
    let atom = r#"<?xml version="1.0" encoding="utf-8"?>
        <feed xmlns="http://www.w3.org/2005/Atom"><title type="html">Kernel &amp;amp; Co</title>
        <id>urn:k</id><updated>2026-11-03T10:00:00Z</updated>
        <link rel="alternate" href="https://kernel.example/"/>
        <entry><id>urn:k:1</id><title>Atom one</title><updated>2026-11-03T10:00:00Z</updated>
               <link href="https://kernel.example/1"/><content type="html">&lt;p&gt;Body&lt;/p&gt;</content>
               <author><name>Ada</name></author></entry>
        </feed>"#;
    serve(
        &server,
        "/atom.xml",
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/atom+xml")
            .set_body_string(atom),
    )
    .await;
    let db = Db::open_in_memory().unwrap();

    // No scheme: https:// is added, so give the full address of the fake server.
    let id = subscribe(&db, &server.uri(), None, NOW).await.unwrap();

    assert_eq!(id, format!("feed/{}/atom.xml", server.uri()));
    assert_eq!(db.feeds().unwrap()[0].title, "Kernel & Co");
    let articles = all(&db);
    assert_eq!(articles[0].title, "Atom one");
    assert_eq!(articles[0].author, "Ada");
    assert_eq!(
        db.article_html(&articles[0].id).unwrap().unwrap(),
        "<p>Body</p>"
    );
}

#[tokio::test]
async fn a_page_without_a_feed_is_an_error() {
    let server = MockServer::start().await;
    serve(
        &server,
        "/",
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/html")
            .set_body_string("<html><head></head><body>Nothing</body></html>"),
    )
    .await;
    let db = Db::open_in_memory().unwrap();

    let err = subscribe(&db, &server.uri(), None, NOW).await.unwrap_err();
    assert!(matches!(err, Error::NoFeed), "{err:?}");
    assert!(db.feeds().unwrap().is_empty());
    assert!(matches!(
        subscribe(&db, "ftp://x", None, NOW).await.unwrap_err(),
        Error::BadUrl
    ));
}

#[tokio::test]
async fn a_json_feed_works_too() {
    let server = MockServer::start().await;
    let feed = r#"{ "version": "https://jsonfeed.org/version/1.1", "title": "Jay",
        "home_page_url": "https://jay.example/",
        "items": [{ "id": "j1", "url": "https://jay.example/1", "title": "Jason",
                    "content_html": "<p>Hi</p>", "date_published": "2026-11-01T08:00:00Z" }] }"#;
    serve(
        &server,
        "/feed.json",
        ResponseTemplate::new(200)
            .insert_header("content-type", "application/feed+json")
            .set_body_string(feed),
    )
    .await;
    let db = Db::open_in_memory().unwrap();

    subscribe(&db, &format!("{}/feed.json", server.uri()), None, NOW)
        .await
        .unwrap();
    let articles = all(&db);
    assert_eq!(articles.len(), 1);
    assert_eq!(articles[0].title, "Jason");
    assert_eq!(articles[0].published, 1_793_520_000_000);
}

#[tokio::test]
async fn a_refresh_keeps_what_the_user_did_and_asks_only_for_changes() {
    let server = MockServer::start().await;
    let first = xml(rss(TWO_ITEMS)).insert_header("etag", "\"v1\"");
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .respond_with(first)
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let db = Db::open_in_memory().unwrap();
    let feed = subscribe(&db, &format!("{}/feed.xml", server.uri()), None, NOW)
        .await
        .unwrap();
    let ids: Vec<String> = all(&db).into_iter().map(|a| a.id).collect();
    db.set_read(&ids, true).unwrap();
    db.set_starred(&ids[..1], true).unwrap();
    db.rename_feed(&feed, "My Kernel").unwrap();

    // The feed changed one title; the server sees the ETag.
    let changed = TWO_ITEMS.replace("<title>Second</title>", "<title>Second, edited</title>");
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .and(header("if-none-match", "\"v1\""))
        .respond_with(xml(rss(&changed)).insert_header("etag", "\"v2\""))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // Kept long enough that nothing is pruned.
    let report = refresh(&db, NOW, 3000).await.unwrap();

    assert_eq!(report.fetched, 1);
    let articles = all(&db);
    assert_eq!(articles[0].title, "Second, edited");
    assert!(articles.iter().all(|a| a.read));
    assert!(articles[0].starred);
    assert_eq!(db.feeds().unwrap()[0].title, "My Kernel");
    // Nothing to send to anyone.
    assert!(db.pending().unwrap().is_empty());

    // Unchanged now.
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .and(header("if-none-match", "\"v2\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    assert_eq!(refresh(&db, NOW, 3000).await.unwrap().fetched, 0);
    assert_eq!(all(&db).len(), 2);
}

#[tokio::test]
async fn a_failing_feed_says_why_and_the_others_go_on() {
    let server = MockServer::start().await;
    serve(&server, "/good.xml", xml(rss(TWO_ITEMS))).await;
    serve(&server, "/bad.xml", xml(rss(""))).await;
    let db = Db::open_in_memory().unwrap();
    let good = subscribe(&db, &format!("{}/good.xml", server.uri()), None, NOW)
        .await
        .unwrap();
    let bad = subscribe(&db, &format!("{}/bad.xml", server.uri()), None, NOW)
        .await
        .unwrap();
    server.reset().await;
    serve(&server, "/good.xml", xml(rss(TWO_ITEMS))).await;
    serve(&server, "/bad.xml", ResponseTemplate::new(500)).await;

    refresh(&db, NOW, 30).await.unwrap();

    let feeds = db.feeds().unwrap();
    let error = |id: &str| {
        feeds
            .iter()
            .find(|f| f.id == id)
            .unwrap()
            .fetch_error
            .clone()
    };
    assert_eq!(
        error(&bad).as_deref(),
        Some("the server answered 500 Internal Server Error")
    );
    assert_eq!(error(&good), None);
    assert_eq!(all(&db).len(), 2);

    // Fixed: the error goes.
    server.reset().await;
    serve(&server, "/bad.xml", xml(rss(""))).await;
    serve(&server, "/good.xml", xml(rss(TWO_ITEMS))).await;
    refresh(&db, NOW, 30).await.unwrap();
    assert!(db.feeds().unwrap().iter().all(|f| f.fetch_error.is_none()));
}

#[tokio::test]
async fn a_pruned_entry_does_not_come_back_while_the_feed_lists_it() {
    let server = MockServer::start().await;
    serve(&server, "/feed.xml", xml(rss(TWO_ITEMS))).await;
    let db = Db::open_in_memory().unwrap();
    let feed = subscribe(&db, &format!("{}/feed.xml", server.uri()), None, NOW)
        .await
        .unwrap();
    let old = article_id(&feed, "a1").to_string();
    db.set_read(&[old], true).unwrap();

    // Long after: the read one is pruned, and stays gone.
    let later = NOW + 400 * DAY;
    refresh(&db, later, 30).await.unwrap();
    refresh(&db, later, 30).await.unwrap();
    let ids: Vec<String> = all(&db).into_iter().map(|a| a.id).collect();
    assert_eq!(ids, [article_id(&feed, "a2").to_string()]);
}

#[tokio::test]
async fn an_entry_without_a_date_keeps_the_one_it_was_first_seen_with() {
    let server = MockServer::start().await;
    serve(
        &server,
        "/feed.xml",
        xml(rss("<item><guid>n</guid><title>Undated</title></item>")),
    )
    .await;
    let db = Db::open_in_memory().unwrap();
    subscribe(&db, &format!("{}/feed.xml", server.uri()), None, NOW)
        .await
        .unwrap();

    refresh(&db, NOW + DAY, 30).await.unwrap();
    assert_eq!(all(&db)[0].published, NOW * 1000);
}

#[tokio::test]
async fn folders_keep_their_id_and_a_deleted_ones_feeds_stay() {
    let server = MockServer::start().await;
    serve(&server, "/feed.xml", xml(rss(TWO_ITEMS))).await;
    let db = Db::open_in_memory().unwrap();
    let tech = db.create_folder("Tech").unwrap();
    assert_eq!(db.create_folder("tech").unwrap(), tech);
    let news = db.create_folder("News").unwrap();
    assert_ne!(news, tech);
    let feed = subscribe(&db, &format!("{}/feed.xml", server.uri()), Some(&tech), NOW)
        .await
        .unwrap();

    db.rename_folder(&tech, "Technology").unwrap();
    assert_eq!(db.folders().unwrap()[1].title, "Technology");
    db.move_feed(&feed, Some(&news)).unwrap();
    assert_eq!(
        db.feeds().unwrap()[0].folder_id.as_deref(),
        Some(news.as_str())
    );
    db.delete_folder(&news).unwrap();
    assert_eq!(db.feeds().unwrap()[0].folder_id, None);
    assert_eq!(db.folders().unwrap().len(), 1);

    db.unsubscribe(&feed).unwrap();
    assert!(db.feeds().unwrap().is_empty());
    assert!(all(&db).is_empty());
}

#[test]
fn article_ids_are_stable_and_positive() {
    assert_eq!(
        article_id("feed/https://a.example/f", "1"),
        article_id("feed/https://a.example/f", "1")
    );
    assert_ne!(article_id("feed/a", "1"), article_id("feed/b", "1"));
    assert_ne!(article_id("feed/a1", ""), article_id("feed/a", "1"));
    assert!(article_id("feed/x", "y") >= 0);
    // Pinned: a change would make every cached entry new again.
    assert_eq!(article_id("feed/x", "y"), 8_695_524_808_960_597_949);
}

#[tokio::test]
async fn a_feed_unsubscribed_while_fetched_is_skipped_and_one_redirected_to_is_found() {
    let server = MockServer::start().await;
    serve(&server, "/feed.xml", xml(rss(TWO_ITEMS))).await;
    let db = Db::open_in_memory().unwrap();
    let url = format!("{}/feed.xml", server.uri());
    let feed = subscribe(&db, &url, None, NOW).await.unwrap();

    db.unsubscribe(&feed).unwrap();
    let page = Page {
        url: Url::parse(&url).unwrap(),
        body: rss(TWO_ITEMS).into_bytes(),
        content_type: String::new(),
        etag: None,
        last_modified: None,
    };
    let parsed = parse(&page).unwrap();
    assert_eq!(store(&db, &feed, &parsed, &page, NOW).unwrap(), 0);
    assert!(all(&db).is_empty());

    // Subscribed by its address after redirects; asked for by the one before.
    let feed = subscribe(&db, &url, None, NOW).await.unwrap();
    db.with_conn(|c| {
        c.execute(
            "UPDATE feeds SET url = 'https://moved.example/feed' WHERE id = ?1",
            [&feed],
        )
        .unwrap()
    });
    assert_eq!(
        db.feed_by_url(&["https://moved.example/feed"]).unwrap(),
        Some(feed)
    );
}

#[tokio::test]
async fn a_refresh_that_reaches_no_feed_fails_and_one_without_feeds_works() {
    let db = Db::open_in_memory().unwrap();
    // No feeds: nothing to reach, and the refresh is done.
    refresh(&db, NOW, 30).await.unwrap();
    assert_eq!(db.last_sync().unwrap(), Some(NOW * 1000));

    // A port nothing listens on: likely offline, so the refresh fails, keeping the feed.
    let closed = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let url = format!("http://127.0.0.1:{closed}/feed.xml");
    let added = db
        .add_local_feed(&NewFeed {
            id: &format!("feed/{url}"),
            url: &url,
            title: "Gone",
            site_url: None,
            folder_id: None,
            icon_url: None,
        })
        .unwrap();
    assert!(added);
    assert!(matches!(
        refresh(&db, NOW + 60, 30).await,
        Err(Error::Local(_))
    ));
    assert_eq!(
        db.feeds().unwrap()[0].fetch_error.as_deref(),
        Some("could not reach the server")
    );
    assert_eq!(db.last_sync().unwrap(), Some(NOW * 1000));

    // One that answers, even with an error, means the network is there.
    let server = MockServer::start().await;
    serve(&server, "/bad.xml", ResponseTemplate::new(404)).await;
    let bad = format!("{}/bad.xml", server.uri());
    db.add_local_feed(&NewFeed {
        id: &format!("feed/{bad}"),
        url: &bad,
        title: "Bad",
        site_url: None,
        folder_id: None,
        icon_url: None,
    })
    .unwrap();
    refresh(&db, NOW + 120, 30).await.unwrap();
}
