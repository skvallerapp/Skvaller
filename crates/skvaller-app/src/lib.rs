//! Skvaller's app layer, without a UI: accounts, settings and background sync.
//!
//! [`App::load`] reads `config.json` and opens each account's cache from the folders in
//! [`Dirs`]. The UI then reads folders, feeds and articles from the local caches only,
//! and [`App::start_sync`] keeps them up to date in the background, reporting its
//! progress through [`App::subscribe`]. Every change to a cache, a sync's or the
//! user's, is told through [`App::changes`]. Nothing the UI calls waits for the network.

mod accounts;
mod app;
mod config;
mod error;
mod feeds;
mod gate;
mod sync;
mod thumbnails;

#[doc(inline)]
pub use app::{
    AccountChange, App, Change, Changed, Dirs, Library, Notice, import_local, seed_local,
};
#[doc(inline)]
pub use config::{
    Account, AccountKind, Clock, Config, DateFormat, Font, FontSize, LIST_WIDTH, Layout, LineWidth,
    MarkReadOn, PaneWidth, SIDEBAR_WIDTH, Settings, SortSubscriptions, SyncSchedule, Theme,
    ThumbnailSize,
};
#[doc(inline)]
pub use error::{Error, Result};
#[doc(inline)]
pub use sync::SyncStatus;
