//! Article thumbnails: fetched into the cache in the background, shown from it.
//!
//! After each sync, and after a feed is added, the new articles' lead images are
//! fetched and stored small, unless thumbnails are off. [`App::changes`] listeners are
//! told as each batch is stored, so they show up while a long run goes on.

use std::sync::atomic::Ordering;

use skvaller_core::prefetch_thumbnails;
use tracing::Level;

use crate::ThumbnailSize;
use crate::app::{App, Change, Changed, Runtime, lock};
use crate::error::Result;

impl App {
    /// The article's cached thumbnail, a JPEG, if it has one.
    ///
    /// # Errors
    ///
    /// [`crate::Error::NoAccount`] or [`crate::Error::Broken`] for an account without a
    /// cache, and [`crate::Error::Core`] when the cache can't be read.
    pub fn thumbnail(&self, account_id: &str, article_id: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.runtime(account_id)?.db.thumbnail(article_id)?)
    }

    /// Fetches thumbnails for the account's new articles on the sync runtime, unless
    /// they are off. While a fetch still runs, that one runs again before it ends, for
    /// the articles stored since it started. Removing the account, or turning
    /// thumbnails off, aborts it.
    pub(crate) fn fetch_thumbnails(&self, rt: &Runtime) {
        // Before the slot's lock, which the running fetch takes to see it: either it
        // sees this and runs again, or it has left the slot empty for a new one.
        rt.thumbnails_again.store(true, Ordering::SeqCst);
        // The setting is read under the lock that turning thumbnails off takes to stop
        // a run, after saving: either that stops this run or this sees them off.
        let off = || self.settings().thumbnail_size == ThumbnailSize::None;
        let changes = self.changes.clone();
        self.spawn_once(rt, &rt.thumbnails, off, |rt| async move {
            loop {
                rt.thumbnails_again.store(false, Ordering::SeqCst);
                let stored = || {
                    // No listener yet is fine: a window that opens later loads the list.
                    let _ = changes.send(Change {
                        account_id: rt.id.clone(),
                        what: Changed::Articles,
                    });
                };
                if let Err(e) = prefetch_thumbnails(&rt.db, stored).await {
                    tracing::event!(
                        name: "thumbnail.prefetch.failure",
                        Level::WARN,
                        account.id = %rt.id,
                        error.message = %e,
                        "fetching thumbnails for account {{account.id}} failed: {{error.message}}",
                    );
                    return;
                }
                let mut slot = lock(&rt.thumbnails);
                if !rt.thumbnails_again.load(Ordering::SeqCst) {
                    *slot = None;
                    return;
                }
            }
        });
    }
}
