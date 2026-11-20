//! The errors of starting the app and reading its accounts.

use std::path::PathBuf;

/// Why starting the app, or reading an account, failed.
///
/// The messages name what failed and where, as the user sees them in a dialog.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The system has no folder of this kind for apps, such as no home folder.
    #[error("Skvaller can't find its {0} folder")]
    NoFolder(&'static str),
    /// A file or folder operation failed.
    #[error("{action} {} failed: {source}", path.display())]
    Io {
        /// What was being done, such as "Creating" or "Saving".
        action: &'static str,
        /// The file or folder it was done to.
        path: PathBuf,
        /// What the system said.
        #[source]
        source: std::io::Error,
    },
    /// `config.json` can't be read, and no copy of it could be kept. Startup stops
    /// here, so a later save can't overwrite the only copy of the user's accounts.
    #[error("Skvaller can't read {}, and keeping a copy of it failed: {source}", path.display())]
    Backup {
        /// The settings file.
        path: PathBuf,
        /// Why the copy failed.
        #[source]
        source: std::io::Error,
    },
    /// No account has this ID.
    #[error("the account was removed")]
    NoAccount,
    /// Only the local account keeps its own subscriptions; a sync account's are
    /// managed on its server.
    #[error("this account's feeds are managed on its server")]
    NotLocal,
    /// A feed or folder needs a name.
    #[error("enter a name")]
    NoTitle,
    /// The account's cache didn't open at startup.
    #[error("this account's cache can't be opened: {0}")]
    Broken(String),
    /// An OPML file is larger than any real one.
    #[error("the file is too large for an OPML file")]
    TooLarge,
    /// A background task stopped before it was done, such as by a panic.
    #[error("the task stopped: {0}")]
    Task(String),
    /// The cache, the server or a feed failed.
    #[error(transparent)]
    Core(#[from] skvaller_core::Error),
}

/// A [`Result`](std::result::Result) with this crate's [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A background task that stopped, as an [`Error`], logged.
pub(crate) fn task_failed(e: &tokio::task::JoinError) -> Error {
    tracing::event!(
        name: "task.panic",
        tracing::Level::ERROR,
        error.message = %e,
        "a task stopped: {{error.message}}",
    );
    Error::Task(e.to_string())
}

/// Turns an I/O error into an [`Error`] that says what failed, on which path.
pub(crate) fn io(
    action: &'static str,
    path: &std::path::Path,
) -> impl FnOnce(std::io::Error) -> Error {
    let path = path.to_owned();
    move |source| Error::Io {
        action,
        path,
        source,
    }
}
