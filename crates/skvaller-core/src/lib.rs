//! Sync, storage and the Google Reader API client. Contains no UI code.

mod api;
mod db;
mod error;
mod keychain;
mod sync;
mod text;

pub use api::Client;
pub use db::{ArticleSummary, Counts, Db, Feed, Folder, Selection, SmartView};
pub use error::{Error, Result};
pub use keychain::{KEYCHAIN_SERVICE, delete_token, load_token, save_token};
pub use sync::{DEFAULT_KEEP_READ_DAYS, SyncReport, sync};
