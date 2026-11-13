//! The `thumb:` URLs the article list shows cached thumbnails by.
//!
//! Blitz loads an image only from a URL. These are answered from the account's cache,
//! never the network: reading and decoding run off the UI thread, and the URL stays
//! short, unlike a `data:` URL of the whole JPEG.
//!
//! A thumbnail is served cropped and shrunk to its box, by its [`Fit`]: Blitz decodes
//! an image whole and keeps it decoded, and a stored one can take over a megabyte that
//! way.
//!
//! The article's frame shares the window's providers, so its untrusted HTML could ask
//! for these URLs too. Each URL carries a token made at launch, which that HTML can't
//! know, as it runs no scripts: a URL without it is refused.

use std::collections::hash_map::RandomState;
use std::fmt::Write as _;
use std::hash::{BuildHasher, Hasher};
use std::sync::{Arc, LazyLock};

use dioxus_native::net::{Bytes, NetHandler, NetProvider, Request, Url};
use skvaller_app::App;
use skvaller_core::fill_thumbnail;

/// The URL scheme of cached thumbnails.
pub const SCHEME: &str = "thumb";

/// The box a thumbnail is served for, in pixels: its CSS box on a 2× screen.
///
/// Decoded, an image takes four bytes a pixel in Blitz's image cache and as much on the
/// GPU: 64 KB for a square, 0.9 MB for the wide box of the default list. Stored
/// thumbnails are at most 800 px and never enlarged, so a wide one stays under 1.5 MB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// 128 × 128, for the Small and Medium boxes beside the headline. Medium is 64 px,
    /// Small 40 px; both get the same, to keep one variant.
    Square,
    /// This many px wide and 16:9, for the Large box below the headline: as wide as the
    /// list lets it, and as high as that makes it (`.thumb.large`). See [`Fit::wide`].
    Wide(u32),
}

/// The widest [`Fit::Wide`] served: the widest list's box on a 2× screen, with room.
const MAX_WIDE: u32 = 1200;

impl Fit {
    /// The wide fit for a Large box `width` CSS px wide.
    ///
    /// The width is rounded up to a step of 40 px, so resizing the list loads new
    /// thumbnails only now and then, and never shows one stretched.
    #[must_use]
    pub fn wide(width: f64) -> Self {
        const STEP: f64 = 40.0;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "the box is a few hundred px wide, at least STEP"
        )]
        let px = ((width.max(STEP) / STEP).ceil() * STEP * 2.0) as u32;
        Self::Wide(px.min(MAX_WIDE))
    }

    /// The box's width and height in pixels.
    fn size(self) -> (u32, u32) {
        match self {
            Self::Square => (128, 128),
            Self::Wide(width) => (width, width * 9 / 16),
        }
    }

    /// The fit's path segment in a [`url`].
    fn segment(self) -> String {
        match self {
            Self::Square => "square".into(),
            Self::Wide(width) => format!("wide{width}"),
        }
    }

    /// The fit whose [`Fit::segment`] is `segment`.
    fn from_segment(segment: &str) -> Option<Self> {
        if segment == "square" {
            return Some(Self::Square);
        }
        let digits = segment.strip_prefix("wide")?;
        // Digits only: `parse` would take a sign.
        if !digits.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        let width: u32 = digits.parse().ok()?;
        (1..=MAX_WIDE).contains(&width).then_some(Self::Wide(width))
    }
}

/// This launch's token, the host of every [`url`]: random, from the standard
/// library's hash seed.
static TOKEN: LazyLock<String> = LazyLock::new(|| {
    let seed = RandomState::new().build_hasher().finish();
    format!("t{seed:016x}")
});

/// The URL of the article's cached thumbnail, served as `fit` says.
pub fn url(account_id: &str, article_id: &str, fit: Fit) -> String {
    format!(
        "{SCHEME}://{}/{}/{}/{}",
        *TOKEN,
        fit.segment(),
        hex(account_id),
        hex(article_id)
    )
}

/// Answers `thumb:` URLs from the accounts' caches.
pub struct Thumbnails(pub Arc<App>);

impl NetProvider for Thumbnails {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let app = Arc::clone(&self.0);
        let url = request.url;
        // A database read, the crop, and then the decode in `bytes`: none on the UI
        // thread.
        tokio::task::spawn_blocking(move || {
            let jpeg = parse(&url).and_then(|(fit, account, article)| {
                let jpeg = app.thumbnail(&account, &article).ok().flatten()?;
                let (width, height) = fit.size();
                fill_thumbnail(&jpeg, width, height)
            });
            match jpeg {
                Some(jpeg) => handler.bytes(url.to_string(), Bytes::from(jpeg)),
                None => handler.error(url.to_string(), "no thumbnail".into()),
            }
        });
    }
}

/// The fit, account and article IDs in a [`url`] of this launch.
fn parse(url: &Url) -> Option<(Fit, String, String)> {
    if url.host_str() != Some(TOKEN.as_str()) {
        return None;
    }
    let mut parts = url.path().strip_prefix('/')?.split('/');
    let fit = Fit::from_segment(parts.next()?)?;
    let (account, article) = (parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    Some((fit, unhex(account)?, unhex(article)?))
}

/// `text`'s bytes as hex: any ID goes in a URL path as it is.
fn hex(text: &str) -> String {
    text.bytes().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

fn unhex(hex: &str) -> Option<String> {
    // `from_str_radix` would take a sign, as in "+f".
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..hex.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok())
        .collect();
    String::from_utf8(bytes?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_fit_go_through_the_url_unchanged() {
        let article = "tag:google.com,2005:reader/item/0000/ä?#";
        for fit in [Fit::Square, Fit::Wide(640), Fit::wide(299.0)] {
            let url = Url::parse(&url("1", article, fit)).unwrap();
            assert_eq!(url.scheme(), SCHEME);
            assert_eq!(parse(&url), Some((fit, "1".into(), article.into())));
        }
        let at = |path: &str| Url::parse(&format!("thumb://{}/{path}", *TOKEN)).unwrap();
        assert_eq!(
            parse(&at("square/31/32")),
            Some((Fit::Square, "1".into(), "2".into()))
        );
        assert_eq!(parse(&at("31/32")), None);
        assert_eq!(parse(&at("huge/31/32")), None);
        assert_eq!(parse(&at("wide640/31")), None);
        assert_eq!(parse(&at("wide640/3/31")), None);
        assert_eq!(parse(&at("wide640/31/32/33")), None);
        assert_eq!(parse(&at("wide640/+f/32")), None);
        for fit in ["wide", "wide0", "wide+640", "wide9999", "wide640x"] {
            assert_eq!(parse(&at(&format!("{fit}/31/32"))), None, "{fit}");
        }
        // Another launch's URL, or one made up by an article.
        assert_eq!(parse(&Url::parse("thumb://t/wide640/31/32").unwrap()), None);
    }

    #[test]
    fn a_wide_fit_rounds_the_box_up_to_steps_of_40_px() {
        // The default list's box, 300 px, on a 2× screen.
        assert_eq!(Fit::wide(300.0), Fit::Wide(640));
        assert_eq!(Fit::Wide(640).size(), (640, 360));
        assert_eq!(Fit::wide(280.0), Fit::Wide(560));
        assert_eq!(Fit::wide(281.0), Fit::Wide(640));
        // The widest list's box.
        assert_eq!(
            Fit::wide(skvaller_app::LIST_WIDTH.max - 40.0),
            Fit::Wide(1040)
        );
        assert_eq!(Fit::wide(5000.0), Fit::Wide(MAX_WIDE));
        assert_eq!(Fit::wide(0.0), Fit::Wide(80));
    }
}
