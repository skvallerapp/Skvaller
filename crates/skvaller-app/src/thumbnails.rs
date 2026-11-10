//! Article thumbnails: fetched into the cache in the background, shown from it.
//!
//! After each sync, and after a feed is added, the new articles' lead images are
//! fetched and stored small, unless thumbnails are off. The list is told as each batch
//! is stored, so they show up while a long run goes on.

use skvaller_core::prefetch_thumbnails;
use tracing::Level;

use crate::ThumbnailSize;
use crate::app::{App, Runtime, lock};
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
    /// they are off or a fetch still runs. Removing the account, or turning thumbnails
    /// off, aborts it.
    pub(crate) fn fetch_thumbnails(&self, rt: &Runtime) {
        // The setting is read under the lock that turning thumbnails off takes to stop
        // a run, after saving: either that stops this run or this sees them off.
        let mut running = lock(&rt.thumbnails);
        let off = self.settings().thumbnail_size == ThumbnailSize::None;
        if off || running.as_ref().is_some_and(|task| !task.is_finished()) {
            return;
        }
        let (Some(runtime), Ok(rt)) = (self.sync_runtime.get(), self.runtime(&rt.id)) else {
            return;
        };
        let events = self.events.clone();
        let task = runtime.spawn(async move {
            let stored = || {
                // The status as it is now, as a sync may have started meanwhile. No
                // listener yet is fine: a window that opens later loads the list.
                let _ = events.send(crate::SyncStatus {
                    changed: true,
                    ..rt.status()
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
            }
        });
        *running = Some(task.abort_handle());
    }
}
