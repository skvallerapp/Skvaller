#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Wrong credentials, or the token was revoked. The user has to sign in again.
    #[error("the server rejected the sign-in")]
    Unauthorized,
    /// Not an `http` or `https` URL, or one with a username or password in it.
    #[error("the server address is not a valid http or https URL")]
    BadUrl,
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    /// The address has no feed, and no link to one.
    #[error("no feed found at this address")]
    NoFeed,
    /// A feed, page or file that can't be used, said for the user.
    #[error("{0}")]
    Local(String),
    #[error("unexpected response from the server: {0}")]
    Protocol(String),
    /// The cache was written by a newer version of Skvaller.
    #[error("the local cache is from a newer version of Skvaller")]
    CacheTooNew,
    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("keychain error: {0}")]
    Keychain(#[from] keyring::Error),
}

impl Error {
    /// The cache file is damaged, or not an SQLite database at all. Only starting it over
    /// helps.
    #[must_use]
    pub fn is_corrupt(&self) -> bool {
        matches!(
            self,
            Error::Db(rusqlite::Error::SqliteFailure(e, _))
                if matches!(
                    e.code,
                    rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
                )
        )
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
