//! The `thumb:` URLs the article list shows cached thumbnails by.
//!
//! Blitz loads an image only from a URL. These are answered from the account's cache,
//! never the network: reading and decoding run off the UI thread, and the URL stays
//! short, unlike a `data:` URL of the whole JPEG.

use std::fmt::Write as _;
use std::sync::Arc;

use dioxus_native::net::{Bytes, NetHandler, NetProvider, Request, Url};
use skvaller_app::App;

/// The URL scheme of cached thumbnails.
pub const SCHEME: &str = "thumb";

/// The URL of the article's cached thumbnail.
pub fn url(account_id: &str, article_id: &str) -> String {
    format!("{SCHEME}://t/{}/{}", hex(account_id), hex(article_id))
}

/// Answers `thumb:` URLs from the accounts' caches.
pub struct Thumbnails(pub Arc<App>);

impl NetProvider for Thumbnails {
    fn fetch(&self, _doc_id: usize, request: Request, handler: Box<dyn NetHandler>) {
        let app = Arc::clone(&self.0);
        let url = request.url;
        // A database read, then the decode in `bytes`: neither on the UI thread.
        tokio::task::spawn_blocking(move || {
            let jpeg = ids(&url)
                .and_then(|(account, article)| app.thumbnail(&account, &article).ok().flatten());
            match jpeg {
                Some(jpeg) => handler.bytes(url.to_string(), Bytes::from(jpeg)),
                None => handler.error(url.to_string(), "no thumbnail".into()),
            }
        });
    }
}

/// The account and article IDs in a [`url`].
fn ids(url: &Url) -> Option<(String, String)> {
    let mut parts = url.path().strip_prefix('/')?.split('/');
    let (account, article) = (parts.next()?, parts.next()?);
    if parts.next().is_some() {
        return None;
    }
    Some((unhex(account)?, unhex(article)?))
}

/// `text`'s bytes as hex: any ID goes in a URL path as it is.
fn hex(text: &str) -> String {
    text.bytes().fold(String::new(), |mut out, b| {
        let _ = write!(out, "{b:02x}");
        out
    })
}

fn unhex(hex: &str) -> Option<String> {
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
    fn ids_go_through_the_url_unchanged() {
        let article = "tag:google.com,2005:reader/item/0000/ä?#";
        let url = Url::parse(&url("1", article)).unwrap();
        assert_eq!(url.scheme(), SCHEME);
        assert_eq!(ids(&url), Some(("1".into(), article.into())));
        assert_eq!(ids(&Url::parse("thumb://t/31").unwrap()), None);
        assert_eq!(ids(&Url::parse("thumb://t/3/31").unwrap()), None);
        assert_eq!(ids(&Url::parse("thumb://t/31/32/33").unwrap()), None);
    }
}
