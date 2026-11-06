//! Article thumbnails: an article's lead image, fetched after a sync and shrunk to a
//! small JPEG in the cache. The UI shows only the cached copy, so image hosts never see
//! what the user scrolls past, and nothing full-size is decoded to be shown small.
//!
//! The image comes from what the feed says first (an enclosure, a media thumbnail),
//! else from the first images in the article's HTML. Only articles new to the cache get
//! one.

// Rust guideline compliant 2026-10-09

use std::io::Cursor;
use std::sync::Arc;
use std::time::Duration;

use feed_rs::model::MediaObject;
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageReader, Limits, RgbImage};
use reqwest::{StatusCode, Url};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::db::ThumbnailWanted;
use crate::full_text::USER_AGENT;
use crate::{Db, Result, html};

/// The longest side of a stored thumbnail, in pixels.
///
/// The Large size spans the list, at most 560 CSS pixels wide, and needs about twice
/// that on a HiDPI screen. 800 is a compromise with the space the cache takes: a
/// thumbnail is about 40–60 KB, so 5,000 cached articles take about 250 MB.
const MAX_SIDE: u32 = 800;
/// JPEG quality of a stored thumbnail. Artifacts don't show at thumbnail size.
const JPEG_QUALITY: u8 = 75;
/// Images with a side shorter than this are icons, emoji, buttons or tracking pixels.
const MIN_SIDE: u32 = 100;
/// Larger downloads are not read. A lead image is rarely over a megabyte or two.
const MAX_IMAGE_BYTES: usize = 5 * 1024 * 1024;
/// Decoding stops past these, so a small file can't claim a huge image (a decompression
/// bomb). Real photos are under 10,000 pixels a side.
const MAX_DECODED_SIDE: u32 = 12_000;
const MAX_DECODE_ALLOC: u64 = 256 * 1024 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);
/// Images fetched at the same time. As for feed icons: hosts can be slow.
const PARALLEL: usize = 6;
/// Articles looked at per run, newest first. The rest wait for the next sync.
const PER_RUN: usize = 300;
/// Articles stored per batch: the UI is told after each, so thumbnails show up while a
/// long run goes on.
const BATCH: usize = 30;
/// Images in an article's HTML that are tried, in order, when one isn't usable.
const MAX_CANDIDATES: usize = 3;
/// Runs in which an article's image couldn't be fetched (network or server errors),
/// before it is given up on.
pub(crate) const MAX_ATTEMPTS: u32 = 3;

/// Hosts that serve tracking pixels, counters, share buttons and emoji, never a lead
/// image. A host matches itself and its subdomains.
const SKIPPED_HOSTS: &[&str] = &[
    "doubleclick.net",
    "feedproxy.google.com",
    "google-analytics.com",
    "pixel.quantserve.com",
    "pixel.wp.com",
    "s.w.org",
    "scorecardresearch.com",
    "stats.wordpress.com",
];

/// Image file name endings, for a feed's image with no media type.
const IMAGE_EXTENSIONS: &[&str] = &[".jpg", ".jpeg", ".png", ".gif", ".webp"];

/// The first usable image among a Google Reader API item's enclosures (`href` and
/// media type), resolved against the article's `base` URL.
pub(crate) fn from_enclosures<'a>(
    enclosures: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
    base: &str,
) -> Option<String> {
    let base = Url::parse(base).ok();
    enclosures.into_iter().find_map(|(href, mime)| {
        let url = resolve(base.as_ref(), href)?;
        is_image(mime, &url).then(|| url.into())
    })
}

/// The first usable image among a feed entry's media: its thumbnails, then its media
/// content, which includes RSS enclosures. Resolved against the feed's `base` URL.
pub(crate) fn from_media(media: &[MediaObject], base: &Url) -> Option<String> {
    let thumbnails = media
        .iter()
        .flat_map(|m| &m.thumbnails)
        .filter(|t| !too_small(t.image.width, t.image.height))
        .find_map(|t| resolve(Some(base), &t.image.uri));
    let content = || {
        media
            .iter()
            .flat_map(|m| &m.content)
            .filter(|c| !too_small(c.width, c.height))
            .find_map(|c| {
                let url = resolve(Some(base), c.url.as_ref()?.as_str())?;
                let mime = c.content_type.as_ref().map(ToString::to_string);
                is_image(mime.as_deref(), &url).then_some(url)
            })
    };
    thumbnails.or_else(content).map(Into::into)
}

/// The first few usable images in an article's untrusted HTML, resolved against its
/// `base` URL. `<img>` with a stated width or height under [`MIN_SIDE`] are skipped.
fn from_html(html: &str, base: &str) -> Vec<String> {
    let Some(doc) = html::parse(html) else {
        return Vec::new();
    };
    let base = Url::parse(base).ok();
    let mut found = Vec::new();
    for img in doc.select("img").nodes() {
        let side = |name: &str| img.attr(name).and_then(|v| parse_side(&v));
        if too_small(side("width"), side("height")) {
            continue;
        }
        // Lazy-loading pages put a placeholder in `src` and the image in `data-src`.
        let url = ["src", "data-src"]
            .into_iter()
            .filter_map(|name| img.attr(name))
            .find_map(|src| resolve(base.as_ref(), &src))
            .map(String::from);
        if let Some(url) = url
            && !found.contains(&url)
        {
            found.push(url);
            if found.len() == MAX_CANDIDATES {
                break;
            }
        }
    }
    found
}

/// A stated side, as in `width="120"` or `width="120px"`. Percentages say nothing.
fn parse_side(value: &str) -> Option<u32> {
    let value = value.trim();
    value
        .strip_suffix("px")
        .unwrap_or(value)
        .trim()
        .parse()
        .ok()
}

fn too_small(width: Option<u32>, height: Option<u32>) -> bool {
    width.is_some_and(|w| w < MIN_SIDE) || height.is_some_and(|h| h < MIN_SIDE)
}

fn is_image(mime: Option<&str>, url: &Url) -> bool {
    match mime {
        Some(mime) => mime.trim().to_ascii_lowercase().starts_with("image/"),
        None => {
            let path = url.path().to_ascii_lowercase();
            IMAGE_EXTENSIONS.iter().any(|ext| path.ends_with(ext))
        }
    }
}

/// `url` made absolute, if it is http(s) and could be a lead image: not from a host or
/// path that serves only trackers, buttons and emoji.
fn resolve(base: Option<&Url>, url: &str) -> Option<Url> {
    let url = match base {
        Some(base) => base.join(url.trim()).ok()?,
        None => Url::parse(url.trim()).ok()?,
    };
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let skipped_host = SKIPPED_HOSTS
        .iter()
        .any(|h| host == *h || host.ends_with(&format!(".{h}")));
    let path = url.path().to_ascii_lowercase();
    // FeedBurner's tracking (~r) and share-button (~ff) images; emoji on any host.
    let skipped_path = path.contains("/~r/") || path.contains("/~ff/") || path.contains("/emoji/");
    (!skipped_host && !skipped_path).then_some(url)
}

/// How fetching one image went.
enum Fetched {
    Image(Vec<u8>),
    /// It will never work: gone, not an image, too large.
    Unusable,
    /// It may work later: network errors, server errors, rate limits.
    Later,
}

/// How getting an article's thumbnail went.
enum Outcome {
    Stored(Vec<u8>),
    /// None of its images is usable.
    None,
    /// Try again next run.
    Later,
}

/// Fetches and stores thumbnails for articles that want one, newest first, a few at a
/// time. `stored` is called after each batch that stored any, so the UI can show them.
///
/// Each article is tried once per run. One whose image fails for a reason that may pass
/// is tried again next run, [`MAX_ATTEMPTS`] runs at most; one with no usable image is
/// given up on at once.
///
/// # Errors
///
/// Only cache errors. A failed download is not an error.
pub async fn prefetch(db: &Db, mut stored: impl FnMut()) -> Result<()> {
    let wanted = db.thumbnails_wanted(PER_RUN)?;
    if wanted.is_empty() {
        return Ok(());
    }
    let http = reqwest::Client::builder()
        .timeout(TIMEOUT)
        .user_agent(USER_AGENT)
        .build()?;
    let limit = Arc::new(Semaphore::new(PARALLEL));
    for batch in wanted.chunks(BATCH) {
        let mut fetches = JoinSet::new();
        for article in batch {
            let candidates = candidates(db, article).await?;
            let (http, limit, id) = (http.clone(), Arc::clone(&limit), article.id.clone());
            fetches.spawn(async move {
                let _permit = limit.acquire_owned().await;
                (id, thumbnail(&http, candidates).await)
            });
        }
        let mut any = false;
        while let Some(done) = fetches.join_next().await {
            // A panicked fetch is tried again next run.
            let Ok((id, outcome)) = done else { continue };
            match outcome {
                Outcome::Stored(jpeg) => any |= db.set_thumbnail(&id, &jpeg)?,
                Outcome::None => db.give_up_thumbnail(&id)?,
                Outcome::Later => db.thumbnail_failed(&id)?,
            }
        }
        if any {
            stored();
        }
    }
    Ok(())
}

/// The images to try for `article`: the one the feed named, else those in its HTML.
async fn candidates(db: &Db, article: &ThumbnailWanted) -> Result<Vec<String>> {
    if let Some(url) = &article.source {
        return Ok(vec![url.clone()]);
    }
    let Some(html) = db.article_html(&article.id)? else {
        return Ok(Vec::new());
    };
    let base = article.url.clone();
    // Off the async threads: parsing a large page takes a while. A panic finds nothing.
    Ok(tokio::task::spawn_blocking(move || from_html(&html, &base))
        .await
        .unwrap_or_default())
}

/// The first of `candidates` that makes a thumbnail. Stops at the first that may work
/// later, so the rest are not tried in its place.
async fn thumbnail(http: &reqwest::Client, candidates: Vec<String>) -> Outcome {
    for url in candidates {
        match fetch(http, &url).await {
            Fetched::Image(bytes) => {
                let jpeg = tokio::task::spawn_blocking(move || shrink(&bytes))
                    .await
                    .ok()
                    .flatten();
                if let Some(jpeg) = jpeg {
                    return Outcome::Stored(jpeg);
                }
            }
            Fetched::Unusable => {}
            Fetched::Later => return Outcome::Later,
        }
    }
    Outcome::None
}

async fn fetch(http: &reqwest::Client, url: &str) -> Fetched {
    let Ok(url) = Url::parse(url) else {
        return Fetched::Unusable;
    };
    let Ok(mut res) = http.get(url).send().await else {
        return Fetched::Later;
    };
    let status = res.status();
    if status.is_server_error()
        || status == StatusCode::TOO_MANY_REQUESTS
        || status == StatusCode::REQUEST_TIMEOUT
    {
        return Fetched::Later;
    }
    let is_image = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.trim().to_ascii_lowercase().starts_with("image/"));
    let too_large = res
        .content_length()
        .is_some_and(|n| n > MAX_IMAGE_BYTES as u64);
    if !status.is_success() || !is_image || too_large {
        return Fetched::Unusable;
    }
    // Read up to the limit only: a server may send no length, or the wrong one.
    let mut body = Vec::new();
    loop {
        match res.chunk().await {
            Ok(Some(chunk)) => {
                body.extend_from_slice(&chunk);
                if body.len() > MAX_IMAGE_BYTES {
                    return Fetched::Unusable;
                }
            }
            Ok(None) => return Fetched::Image(body),
            Err(_) => return Fetched::Later,
        }
    }
}

/// The image as a thumbnail JPEG, scaled down to fit [`MAX_SIDE`], or `None` if it
/// can't be decoded or is too small to be a lead image. Transparency becomes white.
fn shrink(bytes: &[u8]) -> Option<Vec<u8>> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODED_SIDE);
    limits.max_image_height = Some(MAX_DECODED_SIDE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    reader.limits(limits);
    let image = reader.decode().ok()?;
    if image.width() < MIN_SIDE || image.height() < MIN_SIDE {
        return None;
    }
    let image = if image.width() > MAX_SIDE || image.height() > MAX_SIDE {
        image.resize(MAX_SIDE, MAX_SIDE, FilterType::Triangle)
    } else {
        image
    };
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, JPEG_QUALITY)
        .encode_image(&on_white(&image))
        .ok()?;
    Some(jpeg)
}

/// `image` without transparency: see-through pixels blend into white.
fn on_white(image: &DynamicImage) -> RgbImage {
    if !image.color().has_alpha() {
        return image.to_rgb8();
    }
    let rgba = image.to_rgba8();
    RgbImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let blend = |c: u8| {
            let (c, a) = (u16::from(c), u16::from(a));
            // c·a + 255·(255 − a) is at most 255·255, so this fits in a u8.
            u8::try_from((c * a + 255 * (255 - a)) / 255).unwrap_or(u8::MAX)
        };
        image::Rgb([blend(r), blend(g), blend(b)])
    })
}

#[cfg(test)]
mod tests;
