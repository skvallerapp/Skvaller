//! The access token lives in the OS keychain. The password is never stored.
//!
//! These calls block, and can wait for the user to unlock the keychain. Call them from a
//! blocking thread (`spawn_blocking` or a non-async Tauri command), never on the async
//! runtime. On Linux without a running Secret Service they return [`Error::Keychain`].
//!
//! [`Error::Keychain`]: crate::Error::Keychain

use keyring::{Entry, Error as KeyringError};

use crate::Result;

const SERVICE: &str = "io.github.sinderc.skvaller";

/// `account` identifies the sign-in, such as `"carl@https://rss.example.com/api/greader.php"`.
pub fn save_token(account: &str, token: &str) -> Result<()> {
    Ok(Entry::new(SERVICE, account)?.set_password(token)?)
}

pub fn load_token(account: &str) -> Result<Option<String>> {
    match Entry::new(SERVICE, account)?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

pub fn delete_token(account: &str) -> Result<()> {
    match Entry::new(SERVICE, account)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}
