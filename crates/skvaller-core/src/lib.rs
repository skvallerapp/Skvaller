//! Sync, storage and the Google Reader API client. Contains no UI code.

mod api;
mod db;
mod error;
mod full_text;
mod html;
mod keychain;
mod local;
mod net;
mod opml;
mod reader;
mod seed;
mod sync;
mod text;
mod thumbnail;

pub use api::Client;
pub use db::{ArticleSummary, Counts, Db, Feed, Folder, Scope, Selection, View};
pub use error::{Error, Result};
pub use full_text::{load as load_full_text, prefetch as prefetch_full_text};
pub use keychain::{KEYCHAIN_SERVICE, delete_token, load_token, save_token};
pub use local::{refresh as refresh_local, subscribe};
pub use net::ArticleNet;
pub use opml::{ImportReport, export as export_opml, import as import_opml};
pub use reader::ready_article;
pub use seed::seed_articles;
pub use sync::{DEFAULT_KEEP_READ_DAYS, SyncReport, sync};
pub use thumbnail::{fill as fill_thumbnail, prefetch as prefetch_thumbnails};
