//! Feed icons, fetched into the cache as `data:` URLs, for synced and local accounts
//! alike.

use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::net::{self, Untrusted};
use crate::{Db, Result};

/// Larger icons are skipped. Favicons are a few kilobytes.
const MAX_ICON_BYTES: usize = 100 * 1024;
const TIMEOUT: Duration = Duration::from_secs(10);
/// Icons fetched at the same time.
const PARALLEL: usize = 6;

/// Fetches the icons of feeds that are new or whose icon URL changed, `resolve` making
/// each URL absolute; with `only`, just that feed's. A few at a time, since feeds can
/// point at slow hosts. An icon that can't be fetched now is tried again next sync; one
/// that isn't an image is not.
///
/// Only from public addresses, and from the hosts of `trusted`: the sync server, which
/// often serves the icons from the LAN, or the local account's feeds.
pub(crate) async fn fetch(
    db: &Db,
    resolve: impl Fn(&str) -> Option<reqwest::Url>,
    trusted: &[reqwest::Url],
    only: Option<&str>,
) -> Result<()> {
    let stale: Vec<_> = db
        .stale_icons()?
        .into_iter()
        .filter(|(id, _)| only.is_none_or(|only| only == id))
        .collect();
    if stale.is_empty() {
        return Ok(());
    }
    // Not the API client: icons may redirect, and they get no access token.
    let http = Untrusted::new(TIMEOUT, trusted)?;
    let limit = Arc::new(Semaphore::new(PARALLEL));
    let mut fetches = JoinSet::new();
    for (feed_id, url) in stale {
        let Some(resolved) = resolve(&url) else {
            db.set_icon(&feed_id, &url, None)?;
            continue;
        };
        let (http, limit) = (http.clone(), Arc::clone(&limit));
        fetches.spawn(async move {
            let _permit = limit.acquire_owned().await;
            (feed_id, url, fetch_one(&http, resolved).await)
        });
    }
    while let Some(done) = fetches.join_next().await {
        if let Ok((feed_id, url, Some(icon))) = done {
            db.set_icon(&feed_id, &url, icon.as_deref())?;
        }
    }
    Ok(())
}

/// The icon as a `data:` URL, `Some(None)` if there is no usable image there, or `None`
/// to try again next sync.
async fn fetch_one(http: &Untrusted, url: reqwest::Url) -> Option<Option<String>> {
    // On this device or a private network: no icon, for good.
    let Some(request) = http.get(url) else {
        return Some(None);
    };
    let res = match request.send().await {
        Ok(res) => res,
        // A name or a redirect that leads there: no icon, for good.
        Err(e) if net::is_blocked(&e) => return Some(None),
        Err(_) => return None,
    };
    let status = res.status();
    if status.is_server_error()
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
    {
        return None;
    }
    let mime = res
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(';').next().unwrap_or(v).trim().to_owned())
        .filter(|m| m.starts_with("image/"));
    let (true, Some(mime)) = (status.is_success(), mime) else {
        return Some(None);
    };
    let Some(body) = net::read_capped(res, MAX_ICON_BYTES).await.ok()? else {
        return Some(None);
    };
    if body.is_empty() {
        return Some(None);
    }
    Some(Some(format!("data:{mime};base64,{}", BASE64.encode(&body))))
}
