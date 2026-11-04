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

pub type Result<T, E = Error> = std::result::Result<T, E>;
