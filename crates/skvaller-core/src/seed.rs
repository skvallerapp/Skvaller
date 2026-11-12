//! Synthetic articles, for testing the article list at a size no real feed set reaches.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::{Rgb, RgbImage};

use crate::db::{ArticleRow, Db, NewFeed};
use crate::error::{Error, Result};

const FEEDS: usize = 20;
/// Of every three test articles, two get a thumbnail, as many real feeds' do.
const WITH_THUMBNAIL: usize = 2;
/// Distinct test images, so neighbouring rows don't look the same.
const IMAGES: usize = 8;
/// A test image's size: that of a real thumbnail, so decoding one costs as much.
const IMAGE_SIZE: (u32, u32) = (800, 450);
/// Time between two test articles.
const SPACING_MS: i64 = 15 * 60 * 1000;

/// Adds 20 test feeds to a local account's cache, and `articles` articles to them.
///
/// The articles are spread over the feeds, the newest at `now_ms` and each next one 15
/// minutes older; two in three have a thumbnail. Returns how many were stored. Safe to run again: each run adds feeds and
/// articles of its own.
///
/// # Errors
///
/// Returns an error if writing the cache fails.
pub fn seed_articles(db: &Db, articles: usize, now_ms: i64) -> Result<usize> {
    let run = now_ms;
    let feed_ids: Vec<String> = (0..FEEDS).map(|n| format!("seed-{run}-{n}")).collect();
    for (n, id) in feed_ids.iter().enumerate() {
        let url = format!("https://seed-{run}-{n}.invalid/feed.xml");
        let title = format!("Test feed {}", n + 1);
        db.add_local_feed(&NewFeed {
            id,
            url: &url,
            title: &title,
            site_url: None,
            folder_id: None,
            icon_url: None,
        })?;
    }
    let mut stored = 0;
    for (n, id) in feed_ids.iter().enumerate() {
        let entries: Vec<(ArticleRow, bool)> = (0..articles)
            .filter(|i| i % FEEDS == n)
            .map(|i| {
                let id_n = run * 100_000 + i64::try_from(i).unwrap_or(0);
                let row = ArticleRow {
                    id: id_n.to_string(),
                    feed_id: id.clone(),
                    feed_title: String::new(),
                    title: format!("Test article {} with a headline of ordinary length", i + 1),
                    author: format!("Author {}", i % 7),
                    url: format!("https://seed-{run}-{n}.invalid/{i}"),
                    published: now_ms - i64::try_from(i).unwrap_or(0) * SPACING_MS,
                    read: false,
                    starred: false,
                    html: format!(
                        "<p>Body of test article {}.</p><p>Lorem ipsum dolor sit amet.</p>",
                        i + 1
                    ),
                    thumbnail: None,
                };
                (row, true)
            })
            .collect();
        stored += db.store_local_feed(id, &entries, None, None)?;
    }
    let images: Vec<Vec<u8>> = (0..IMAGES).map(test_image).collect::<Result<_>>()?;
    for i in (0..articles).filter(|i| i % 3 < WITH_THUMBNAIL) {
        let id = (run * 100_000 + i64::try_from(i).unwrap_or(0)).to_string();
        db.set_thumbnail(&id, &images[i % IMAGES])?;
    }
    Ok(stored)
}

/// A JPEG of a diagonal gradient, its colors by `n`.
fn test_image(n: usize) -> Result<Vec<u8>> {
    let (w, h) = IMAGE_SIZE;
    let hue = u8::try_from(n * 255 / IMAGES).unwrap_or(0);
    let image = RgbImage::from_fn(w, h, |x, y| {
        let t = u8::try_from((x + y) * 255 / (w + h)).unwrap_or(0);
        Rgb([hue, t, 255 - t / 2])
    });
    let mut jpeg = Cursor::new(Vec::new());
    JpegEncoder::new_with_quality(&mut jpeg, 75)
        .encode_image(&image)
        .map_err(|e| Error::Local(format!("encoding a test image failed: {e}")))?;
    Ok(jpeg.into_inner())
}
