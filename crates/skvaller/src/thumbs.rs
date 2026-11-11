//! The `thumb:` URLs the article list shows cached thumbnails by.
//!
//! Blitz loads an image only from a URL. These are answered from the account's cache,
//! never the network: reading and decoding run off the UI thread, and the URL stays
//! short, unlike a `data:` URL of the whole JPEG.
//!
//! A small thumbnail beside the headline is served cropped and shrunk to [`SQUARE`]:
//! Blitz decodes an image whole and keeps it decoded, and a stored one can take over a
//! megabyte that way.
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
use skvaller_core::square_thumbnail;

/// The URL scheme of cached thumbnails.
pub const SCHEME: &str = "thumb";

/// The side, in pixels, of the square a small thumbnail is served as.
///
/// The Medium size is a 64 px box, so 128 is sharp up to a 2× screen; the Small 40 px
/// box gets the same, to keep one variant. Decoded, a square takes 64 KB in Blitz's
/// image cache and as much on the GPU.
const SQUARE: u32 = 128;

/// How a thumbnail is served.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fit {
    /// Cropped and shrunk to [`SQUARE`], for the Small and Medium boxes.
    Square,
    /// As stored, for the Large size that spans the row.
    Full,
}

impl Fit {
    /// The fit's path segment in a [`url`].
    fn segment(self) -> &'static str {
        match self {
            Self::Square => "square",
            Self::Full => "full",
        }
    }

    /// The fit whose [`Fit::segment`] is `segment`. The URL test catches a variant
    /// missing here.
    fn from_segment(segment: &str) -> Option<Self> {
        [Self::Square, Self::Full]
            .into_iter()
            .find(|fit| fit.segment() == segment)
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
        // A database read, the squaring, and then the decode in `bytes`: none on the UI
        // thread.
        tokio::task::spawn_blocking(move || {
            let jpeg = parse(&url).and_then(|(fit, account, article)| {
                let jpeg = app.thumbnail(&account, &article).ok().flatten()?;
                match fit {
                    Fit::Square => square_thumbnail(&jpeg, SQUARE),
                    Fit::Full => Some(jpeg),
                }
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
        for fit in [Fit::Square, Fit::Full] {
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
        assert_eq!(parse(&at("full/31")), None);
        assert_eq!(parse(&at("full/3/31")), None);
        assert_eq!(parse(&at("full/31/32/33")), None);
        assert_eq!(parse(&at("full/+f/32")), None);
        // Another launch's URL, or one made up by an article.
        assert_eq!(parse(&Url::parse("thumb://t/full/31/32").unwrap()), None);
    }
}
