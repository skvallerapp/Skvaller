//! Picking, shrinking and fetching thumbnails, against a fake image host.

use image::{ImageFormat, Rgba, RgbaImage};
use rusqlite::OptionalExtension;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::db::ArticleRow;
use crate::{Scope, Selection, View};

const PAGE: &str = "https://blog.example/posts/1";

/// A PNG of `width` × `height`, one color.
fn png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    RgbaImage::from_pixel(width, height, Rgba(color))
        .write_to(&mut out, ImageFormat::Png)
        .unwrap();
    out.into_inner()
}

fn decoded(jpeg: &[u8]) -> DynamicImage {
    image::load_from_memory_with_format(jpeg, ImageFormat::Jpeg).unwrap()
}

#[test]
fn an_items_image_enclosure_is_picked_over_other_media() {
    let pick =
        |enclosures: &[(&str, Option<&str>)]| from_enclosures(enclosures.iter().copied(), PAGE);
    assert_eq!(
        pick(&[
            ("https://cdn.example/a.mp3", Some("audio/mpeg")),
            ("https://cdn.example/a.jpg", Some("image/jpeg")),
        ]),
        Some("https://cdn.example/a.jpg".to_owned())
    );
    // No type: the file name says it.
    assert_eq!(
        pick(&[("/media/b.PNG", None)]),
        Some("https://blog.example/media/b.PNG".to_owned())
    );
    assert_eq!(pick(&[("https://cdn.example/b.mp4", None)]), None);
    assert_eq!(
        pick(&[("https://pixel.wp.com/g.gif", Some("image/gif"))]),
        None
    );
    assert_eq!(pick(&[]), None);
}

#[test]
fn an_entrys_media_thumbnail_comes_before_its_media_content_and_enclosure() {
    let entry = |items: &str| {
        let feed = format!(
            r#"<?xml version="1.0"?>
            <rss version="2.0" xmlns:media="http://search.yahoo.com/mrss/"><channel>
            <title>T</title><item><guid>1</guid>{items}</item></channel></rss>"#
        );
        let feed = feed_rs::parser::parse(feed.as_bytes()).unwrap();
        let base = Url::parse(PAGE).unwrap();
        from_media(&feed.entries[0].media, &base)
    };
    assert_eq!(
        entry(
            r#"<media:content url="https://cdn.example/full.jpg" type="image/jpeg"/>
               <media:thumbnail url="https://cdn.example/thumb.jpg" width="480" height="360"/>"#
        ),
        Some("https://cdn.example/thumb.jpg".to_owned())
    );
    assert_eq!(
        entry(r#"<media:content url="https://cdn.example/full.jpg" type="image/jpeg"/>"#),
        Some("https://cdn.example/full.jpg".to_owned())
    );
    assert_eq!(
        entry(r#"<enclosure url="https://cdn.example/e.webp" type="image/webp" length="1"/>"#),
        Some("https://cdn.example/e.webp".to_owned())
    );
    // Too small to be the lead image, and not an image at all.
    assert_eq!(
        entry(
            r#"<media:thumbnail url="https://cdn.example/icon.png" width="32" height="32"/>
               <enclosure url="https://cdn.example/ep.mp3" type="audio/mpeg" length="1"/>"#
        ),
        None
    );
}

#[test]
fn the_first_images_in_the_html_are_picked_without_tiny_ones_or_trackers() {
    let html = r#"
        <p><img src="https://feeds.feedburner.com/~r/blog/~4/abc" width="1" height="1">
        <img src="/spacer.gif" width="1" height="1">
        <img src="https://s.w.org/images/core/emoji/15.0/72x72/1f600.png" class="emoji">
        <img src="https://cdn.example/smiley.png" width="16px">
        <img src="data:image/gif;base64,R0lGOD" data-src="/lazy.jpg">
        <img src="photo.jpg" width="100%">
        <img src="https://cdn.example/photo.jpg">
        <img src="https://cdn.example/photo.jpg">
        <img src="https://cdn.example/fourth.jpg"></p>"#;
    assert_eq!(
        from_html(html, PAGE),
        [
            "https://blog.example/lazy.jpg",
            "https://blog.example/posts/photo.jpg",
            "https://cdn.example/photo.jpg",
        ]
    );
    assert!(from_html("<p>No images</p>", PAGE).is_empty());
}

#[test]
fn a_large_image_shrinks_to_fit_and_a_small_one_stays_its_size() {
    let big = decoded(&shrink(&png(2000, 1000, [10, 120, 200, 255])).unwrap());
    assert_eq!((big.width(), big.height()), (MAX_SIDE, MAX_SIDE / 2));
    let tall = decoded(&shrink(&png(300, 1600, [10, 120, 200, 255])).unwrap());
    assert_eq!((tall.width(), tall.height()), (150, MAX_SIDE));
    let small = decoded(&shrink(&png(320, 200, [10, 120, 200, 255])).unwrap());
    assert_eq!((small.width(), small.height()), (320, 200));
}

#[test]
fn tiny_broken_and_see_through_images() {
    assert_eq!(shrink(&png(1, 1, [0, 0, 0, 0])), None);
    assert_eq!(shrink(&png(400, 60, [0, 0, 0, 255])), None);
    assert_eq!(shrink(b"<html>not an image</html>"), None);
    // Transparent becomes white, not black.
    let clear = decoded(&shrink(&png(200, 200, [0, 0, 0, 0])).unwrap()).to_rgb8();
    assert!(clear.get_pixel(100, 100).0.iter().all(|&c| c > 250));
}

/// A cache with one article per `(id, feed's image, HTML)`, each new, so each wants a
/// thumbnail.
fn db_with(articles: &[(i64, Option<String>, &str)]) -> Db {
    let db = Db::open_in_memory().unwrap();
    let rows: Vec<ArticleRow> = articles
        .iter()
        .map(|(id, source, html)| ArticleRow {
            id: id.to_string(),
            feed_id: "feed/1".into(),
            feed_title: "Feed".into(),
            title: format!("Article {id}"),
            author: String::new(),
            url: PAGE.into(),
            published: *id,
            read: false,
            starred: false,
            html: (*html).to_owned(),
            thumbnail: source.clone(),
        })
        .collect();
    db.upsert_articles(&rows).unwrap();
    db
}

/// Each article's `(has a thumbnail, still wants one, failed runs)`.
fn states(db: &Db) -> Vec<(String, bool, bool, i64)> {
    let listed = db
        .articles(&Selection {
            view: View::All,
            scope: Scope::All,
        })
        .unwrap();
    listed
        .into_iter()
        .map(|a| {
            let attempts: Option<i64> = db.with_conn(|c| {
                c.query_row(
                    "SELECT attempts FROM thumbnail_sources WHERE article_id = ?1",
                    [&a.id],
                    |r| r.get(0),
                )
                .optional()
                .unwrap()
            });
            (a.id, a.thumbnail, attempts.is_some(), attempts.unwrap_or(0))
        })
        .collect()
}

async fn serve(server: &MockServer, at: &str, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(at))
        .respond_with(response)
        .mount(server)
        .await;
}

fn image(bytes: Vec<u8>) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "image/png")
        .set_body_bytes(bytes)
}

#[tokio::test]
async fn fetched_images_are_stored_and_failures_given_up_on() {
    let server = MockServer::start().await;
    let at = |p: &str| Some(format!("{}{p}", server.uri()));
    serve(
        &server,
        "/ok.png",
        image(png(1200, 900, [200, 30, 30, 255])),
    )
    .await;
    serve(&server, "/gone.png", ResponseTemplate::new(404)).await;
    serve(&server, "/busy.png", ResponseTemplate::new(503)).await;
    serve(
        &server,
        "/page.html",
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/html")
            .set_body_string("<p>Not an image</p>"),
    )
    .await;
    serve(
        &server,
        "/second.png",
        image(png(400, 300, [30, 200, 30, 255])),
    )
    .await;
    let html = format!(
        r#"<img src="{0}/gone.png"><img src="{0}/page.html"><img src="{0}/second.png">"#,
        server.uri()
    );
    let db = db_with(&[
        (6, at("/ok.png"), ""),
        (5, at("/gone.png"), ""),
        (4, at("/busy.png"), ""),
        (3, None, &html),
        (2, None, "<p>No images</p>"),
    ]);

    let mut batches = 0;
    prefetch(&db, || batches += 1).await.unwrap();

    assert_eq!(batches, 1);
    assert_eq!(
        states(&db),
        [
            ("6".to_owned(), true, false, 0),
            ("5".to_owned(), false, false, 0),
            ("4".to_owned(), false, true, 1),
            ("3".to_owned(), true, false, 0),
            ("2".to_owned(), false, false, 0),
        ]
    );
    let stored = decoded(&db.thumbnail("6").unwrap().unwrap());
    assert_eq!((stored.width(), stored.height()), (MAX_SIDE, 600));
    assert_eq!(db.thumbnail("5").unwrap(), None);

    // The busy host is tried once per run, then given up on.
    for _ in 1..MAX_ATTEMPTS {
        prefetch(&db, || batches += 1).await.unwrap();
    }
    assert_eq!(states(&db)[2], ("4".to_owned(), false, false, 0));
    assert_eq!(batches, 1);
    let busy = server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .filter(|r| r.url.path() == "/busy.png")
        .count();
    assert_eq!(busy, MAX_ATTEMPTS as usize);
}

#[tokio::test]
async fn a_large_download_is_not_read() {
    let server = MockServer::start().await;
    serve(&server, "/huge.png", image(vec![0; MAX_IMAGE_BYTES + 1])).await;
    let db = db_with(&[(1, Some(format!("{}/huge.png", server.uri())), "")]);
    prefetch(&db, || {}).await.unwrap();
    assert_eq!(states(&db), [("1".to_owned(), false, false, 0)]);
}

#[test]
fn a_thumbnail_goes_with_its_article() {
    let db = db_with(&[(1, None, ""), (2, None, "")]);
    assert!(db.set_thumbnail("1", b"jpeg").unwrap());
    // Pruned: read, unstarred and old.
    db.set_read(&["1", "2"], true).unwrap();
    db.with_conn(|c| c.execute("DELETE FROM pending", []).unwrap());
    assert_eq!(db.prune(i64::MAX, i64::MIN).unwrap(), 2);
    let left: i64 = db.with_conn(|c| {
        c.query_row(
            "SELECT (SELECT count(*) FROM thumbnails) + (SELECT count(*) FROM thumbnail_sources)",
            [],
            |r| r.get(0),
        )
        .unwrap()
    });
    assert_eq!(left, 0);
    // An article gone while its image was fetched gets nothing.
    assert!(!db.set_thumbnail("1", b"jpeg").unwrap());
}

#[test]
fn only_articles_new_to_the_cache_want_a_thumbnail() {
    let db = db_with(&[(1, None, "")]);
    // As an article cached before thumbnails were: it wants none.
    db.give_up_thumbnail("1").unwrap();
    let again = ArticleRow {
        id: "1".into(),
        feed_id: "feed/1".into(),
        feed_title: "Feed".into(),
        title: "Article 1, edited".into(),
        author: String::new(),
        url: PAGE.into(),
        published: 1,
        read: false,
        starred: false,
        html: String::new(),
        thumbnail: Some("https://cdn.example/a.jpg".into()),
    };
    assert_eq!(db.upsert_articles(&[again]).unwrap(), 1);
    assert_eq!(states(&db), [("1".to_owned(), false, false, 0)]);
}
