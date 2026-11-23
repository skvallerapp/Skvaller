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

const TWO_ITEMS: &str = r"
    <item><guid>a1</guid><title>First</title><link>/posts/1</link>
          <pubDate>Mon, 02 Nov 2026 10:00:00 GMT</pubDate>
          <description>&lt;p&gt;Hello &lt;b&gt;there&lt;/b&gt;&lt;/p&gt;</description></item>
    <item><guid>a2</guid><title>Second</title><link>https://kernel.example/posts/2</link>
          <pubDate>Tue, 03 Nov 2026 10:00:00 GMT</pubDate></item>";

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
    assert_eq!(db.feeds().unwrap(), []);
    assert!(matches!(
        subscribe(&db, "ftp://x", None, NOW).await.unwrap_err(),
        Error::BadUrl
    ));
}

#[tokio::test]
async fn a_json_feed_works_too() {
    let server = MockServer::start().await;
    let feed = r#"{ "version": "https://jsonfeed.org/version/1.1", "title": "Jay",
        "home_page_url": "/blog/",
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
    // feed-rs leaves JSON Feed links relative.
    assert_eq!(
        db.feeds().unwrap()[0].site_url,
        Some(format!("{}/blog/", server.uri()))
    );
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
    let report = refresh(&db, NOW, 3000, None).await.unwrap();

    assert_eq!(report.fetched, 1);
    let articles = all(&db);
    assert_eq!(articles[0].title, "Second, edited");
    assert!(articles.iter().all(|a| a.read));
    assert!(articles[0].starred);
    assert_eq!(db.feeds().unwrap()[0].title, "My Kernel");
    // Nothing to send to anyone.
    assert_eq!(db.pending().unwrap(), []);

    // Unchanged now.
    Mock::given(method("GET"))
        .and(path("/feed.xml"))
        .and(header("if-none-match", "\"v2\""))
        .respond_with(ResponseTemplate::new(304))
        .mount(&server)
        .await;
    assert_eq!(refresh(&db, NOW, 3000, None).await.unwrap().fetched, 0);
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

    refresh(&db, NOW, 30, None).await.unwrap();

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

    // Once the feed works again, its error is cleared.
    server.reset().await;
    serve(&server, "/bad.xml", xml(rss(""))).await;
    serve(&server, "/good.xml", xml(rss(TWO_ITEMS))).await;
    refresh(&db, NOW, 30, None).await.unwrap();
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
    refresh(&db, later, 30, None).await.unwrap();
    refresh(&db, later, 30, None).await.unwrap();
    let ids: Vec<String> = all(&db).into_iter().map(|a| a.id).collect();
    assert_eq!(ids, [article_id(&feed, "a2").to_string()]);
}

#[tokio::test]
async fn old_unread_entries_go_after_their_own_period_except_starred() {
    let server = MockServer::start().await;
    serve(&server, "/feed.xml", xml(rss(TWO_ITEMS))).await;
    let db = Db::open_in_memory().unwrap();
    let feed = subscribe(&db, &format!("{}/feed.xml", server.uri()), None, NOW)
        .await
        .unwrap();
    let starred = article_id(&feed, "a1").to_string();
    db.set_starred(&[starred.as_str()], true).unwrap();

    // Kept forever without a period of their own, and within one.
    let later = NOW + 400 * DAY;
    assert_eq!(refresh(&db, later, 30, None).await.unwrap().pruned, 0);
    assert_eq!(refresh(&db, later, 30, Some(500)).await.unwrap().pruned, 0);
    // Past it, the unstarred one goes, and stays gone.
    assert_eq!(refresh(&db, later, 30, Some(180)).await.unwrap().pruned, 1);
    assert_eq!(refresh(&db, later, 30, Some(180)).await.unwrap().fetched, 0);
    let ids: Vec<String> = all(&db).into_iter().map(|a| a.id).collect();
    assert_eq!(ids, [starred]);
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

    refresh(&db, NOW + DAY, 30, None).await.unwrap();
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
    assert_eq!(db.feeds().unwrap(), []);
    assert_eq!(all(&db), []);
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
    assert_eq!(all(&db), []);

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
    refresh(&db, NOW, 30, None).await.unwrap();
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
        refresh(&db, NOW + 60, 30, None).await,
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
    refresh(&db, NOW + 120, 30, None).await.unwrap();
}

#[tokio::test]
async fn repeated_entry_id_keeps_the_first_and_its_reader_view_text() {
    let server = MockServer::start().await;
    let items = r"
        <item><guid>dup</guid><title>One</title><link>https://kernel.example/1</link>
              <pubDate>Mon, 02 Nov 2026 10:00:00 GMT</pubDate></item>
        <item><guid>dup</guid><title>Two</title><link>https://kernel.example/2</link>
              <pubDate>Mon, 02 Nov 2026 11:00:00 GMT</pubDate></item>";
    serve(&server, "/feed.xml", xml(rss(items))).await;
    let db = Db::open_in_memory().unwrap();
    subscribe(&db, &format!("{}/feed.xml", server.uri()), None, NOW)
        .await
        .unwrap();
    let articles = all(&db);
    assert_eq!(articles.len(), 1);
    assert_eq!(articles[0].title, "One");
    let id = articles[0].id.clone();
    db.set_full_text(&id, &articles[0].url, "<p>full</p>")
        .unwrap();

    let report = refresh(&db, NOW, 3000, None).await.unwrap();

    assert_eq!(report.fetched, 0);
    assert!(db.full_text(&id).unwrap().is_some());
}

#[tokio::test]
async fn a_youtube_entry_takes_its_media_description() {
    let server = MockServer::start().await;
    let feed = r#"<?xml version="1.0"?>
        <feed xmlns="http://www.w3.org/2005/Atom" xmlns:media="http://search.yahoo.com/mrss/">
          <title>Channel</title>
          <entry><id>yt:video:1</id><title>Video</title>
            <link rel="alternate" href="https://www.youtube.com/watch?v=1"/>
            <media:group>
              <media:content url="https://www.youtube.com/v/1" type="application/x-shockwave-flash"/>
              <media:description>Line one &amp; &lt;b&gt;
Line two: https://example.com/a?b=1&amp;c=2.</media:description></media:group></entry>
        </feed>"#;
    serve(&server, "/feed.xml", xml(feed.to_owned())).await;
    let db = Db::open_in_memory().unwrap();
    let feed_id = subscribe(&db, &format!("{}/feed.xml", server.uri()), None, NOW)
        .await
        .unwrap();

    let id = article_id(&feed_id, "yt:video:1").to_string();
    assert_eq!(
        db.article_html(&id).unwrap().as_deref(),
        Some("<p>Line one &amp; &lt;b&gt;<br>Line two: https://example.com/a?b=1&amp;c=2.</p>")
    );
}

#[tokio::test]
async fn media_descriptions_come_after_content_and_an_empty_one_is_no_error() {
    let server = MockServer::start().await;
    let media = |description: &str| {
        format!(
            r#"<media:group><media:content url="https://www.youtube.com/v/1"/>{description}</media:group>"#
        )
    };
    let feed = format!(
        r#"<?xml version="1.0"?>
        <feed xmlns="http://www.w3.org/2005/Atom" xmlns:media="http://search.yahoo.com/mrss/">
          <title>Channel</title>
          <entry><id>empty</id><title>A</title>{}</entry>
          <entry><id>blank</id><title>B</title>{}</entry>
          <entry><id>summary</id><title>C</title><summary>Own text</summary>{}</entry>
          <entry><id>no-summary</id><title>D</title><summary> </summary>{}</entry>
          <entry><id>html</id><title>E</title>{}</entry>
          <entry><id>title</id><title>F</title>{}</entry>
          <entry><id>typed</id><title>G</title>{}</entry>
          <entry><id>cdata</id><title>H</title>{}</entry>
          <entry><id>blank-first</id><title>I</title>{}{}</entry>
          <entry><id>text</id><title>J</title><summary type="text">a &lt;b&gt; c
d</summary></entry>
        </feed>"#,
        media("<media:description/>"),
        media("<media:description> </media:description>"),
        media("<media:description>Media text</media:description>"),
        media("<media:description>Media text</media:description>"),
        media(r#"<media:description type="html">&lt;b&gt;Bold&lt;/b&gt;</media:description>"#),
        media("<media:title></media:title><media:description>Kept</media:description>"),
        media(r#"<media:description type="text/html">Odd</media:description>"#),
        media("<media:description><![CDATA[]]><!-- none --></media:description>"),
        media("<media:description> </media:description>"),
        media("<media:description>Second</media:description>"),
    );
    serve(&server, "/feed.xml", xml(feed)).await;
    let db = Db::open_in_memory().unwrap();
    let feed_id = subscribe(&db, &format!("{}/feed.xml", server.uri()), None, NOW)
        .await
        .unwrap();

    let html = |guid: &str| {
        db.article_html(&article_id(&feed_id, guid).to_string())
            .unwrap()
            .unwrap_or_default()
    };
    assert_eq!(html("empty"), "");
    assert_eq!(html("summary"), "<p>Own text</p>");
    assert_eq!(html("no-summary"), "<p>Media text</p>");
    assert_eq!(html("html"), "<b>Bold</b>");
    assert_eq!(html("title"), "<p>Kept</p>");
    assert_eq!(html("typed"), "");
    assert_eq!(html("cdata"), "");
    assert_eq!(html("blank-first"), "<p>Second</p>");
    assert_eq!(html("text"), "<p>a &lt;b&gt; c<br>d</p>");
}

#[tokio::test]
async fn new_entries_want_a_thumbnail_from_their_media_and_only_once() {
    let server = MockServer::start().await;
    let items = r#"
        <item><guid>m1</guid><title>Thumb</title><link>/1</link>
              <media:content url="https://cdn.example/full.jpg" type="image/jpeg"/>
              <media:thumbnail url="/thumb.jpg" width="640" height="360"/></item>
        <item><guid>m2</guid><title>Enclosure</title><link>/2</link>
              <enclosure url="https://cdn.example/e.png" type="image/png" length="1"/></item>
        <item><guid>m3</guid><title>Text only</title><link>/3</link>
              <media:thumbnail url="https://cdn.example/icon.png" width="24" height="24"/></item>"#;
    let feed = rss(items).replace(
        r#"<rss version="2.0">"#,
        r#"<rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/">"#,
    );
    serve(&server, "/feed.xml", xml(feed)).await;
    let db = Db::open_in_memory().unwrap();
    let url = format!("{}/feed.xml", server.uri());
    let feed_id = subscribe(&db, &url, None, NOW).await.unwrap();

    let source = |guid: &str| -> Option<Option<String>> {
        db.with_conn(|c| {
            c.query_row(
                "SELECT url FROM thumbnail_sources WHERE article_id = ?1",
                [article_id(&feed_id, guid).to_string()],
                |r| r.get(0),
            )
            .ok()
        })
    };
    assert_eq!(
        source("m1"),
        Some(Some(format!("{}/thumb.jpg", server.uri())))
    );
    assert_eq!(
        source("m2"),
        Some(Some("https://cdn.example/e.png".to_owned()))
    );
    // Its only image is an icon: the HTML is looked at instead.
    assert_eq!(source("m3"), Some(None));

    // One done with is not wanted again when the feed lists it next time.
    db.give_up_thumbnail(&article_id(&feed_id, "m1").to_string())
        .unwrap();
    refresh(&db, NOW, 30, None).await.unwrap();
    assert_eq!(source("m1"), None);
    assert_eq!(
        source("m2"),
        Some(Some("https://cdn.example/e.png".to_owned()))
    );
}

#[test]
fn a_typed_address_gets_https_unless_it_names_a_scheme() {
    let url = |s| with_scheme(s).map(String::from);
    assert_eq!(
        url("example.com/feed").as_deref(),
        Some("https://example.com/feed")
    );
    assert_eq!(
        url("example.com/?next=https://x").as_deref(),
        Some("https://example.com/?next=https://x")
    );
    assert_eq!(
        url("localhost:8080").as_deref(),
        Some("https://localhost:8080/")
    );
    assert_eq!(
        url("http://example.com/").as_deref(),
        Some("http://example.com/")
    );
    assert_eq!(url("ftp://example.com/"), None);
}
