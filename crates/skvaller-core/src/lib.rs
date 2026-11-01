//! Sync, storage and the Google Reader API client. Contains no UI code.

mod api;
mod db;
mod error;
mod keychain;
mod sync;

pub use api::Client;
pub use db::Db;
pub use error::{Error, Result};
pub use keychain::{delete_token, load_token, save_token};
pub use sync::{DEFAULT_KEEP_READ_DAYS, SyncReport, sync};
