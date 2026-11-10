//! Skvaller's app layer, without a UI: accounts, settings and background sync.
//!
//! [`App::load`] reads `config.json` and opens each account's cache from the folders in
//! [`Dirs`]. The UI then reads folders, feeds and articles from the local caches only,
//! and [`App::start_sync`] keeps them up to date in the background, reporting through
//! [`App::subscribe`]. Nothing the UI calls waits for the network.
//!
//! The file formats and folders are those of the Tauri version of Skvaller, so its
//! accounts carry over.

mod accounts;
mod app;
mod config;
mod error;
mod gate;
mod sync;

#[doc(inline)]
pub use app::{AccountChange, App, Dirs, Library, Notice, import_local, seed_local};
#[doc(inline)]
pub use config::{
    Account, AccountKind, Clock, Config, DateFormat, Font, MarkReadOn, Settings, SortSubscriptions,
    SyncSchedule, Theme, ThumbnailSize,
};
#[doc(inline)]
pub use error::{Error, Result};
#[doc(inline)]
pub use sync::SyncStatus;
