//! The access token lives in the OS keychain. The password is never stored.
//!
//! These calls block, and can wait for the user to unlock the keychain. Call them from a
//! blocking thread (`spawn_blocking`), never on the async runtime. On Linux without a
//! running Secret Service they return [`Error::Keychain`].
//!
//! [`Error::Keychain`]: crate::Error::Keychain

use keyring::{Entry, Error as KeyringError};

use crate::Result;

/// The app's bundle identifier. Keep it unchanged: saved tokens are stored under it. The
/// folders are named `Skvaller` instead (`Dirs::system` in `skvaller-app`).
pub const KEYCHAIN_SERVICE: &str = "io.github.skvallerapp.Skvaller";

/// Saves the access token in the OS keychain.
///
/// `account` identifies the sign-in, such as
/// `"carl@https://rss.example.com/api/greader.php"`.
///
/// # Errors
///
/// Returns `Error::Keychain` if the OS keychain refuses or can't be reached.
pub fn save_token(account: &str, token: &str) -> Result<()> {
    Ok(Entry::new(KEYCHAIN_SERVICE, account)?.set_password(token)?)
}

/// The access token saved for `account`, if any.
///
/// # Errors
///
/// Returns `Error::Keychain` if the OS keychain refuses or can't be reached. A missing
/// token is `Ok(None)`.
pub fn load_token(account: &str) -> Result<Option<String>> {
    match Entry::new(KEYCHAIN_SERVICE, account)?.get_password() {
        Ok(token) => Ok(Some(token)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Deletes the access token saved for `account`.
///
/// # Errors
///
/// Returns `Error::Keychain` if the OS keychain refuses or can't be reached. A missing
/// token is not an error.
pub fn delete_token(account: &str) -> Result<()> {
    match Entry::new(KEYCHAIN_SERVICE, account)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keychain_service_keeps_its_name() {
        // Changing it loses existing accounts: their tokens are saved under this name.
        assert_eq!(KEYCHAIN_SERVICE, "io.github.skvallerapp.Skvaller");
    }
}
