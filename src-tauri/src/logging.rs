//! Logging. Events go to a daily file in the app's log folder, which release builds on
//! Windows and macOS need, having no console, and to stderr.

// Rust guideline compliant 2026-10-09

use std::path::Path;

use tracing::Level;
use tracing::level_filters::LevelFilter;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Layer, fmt};

/// Log files kept, one a day, so about a week's worth. Dates are UTC.
const FILES_KEPT: usize = 7;

/// Sends events at INFO and above to a file in `dir`, and to stderr, and logs panics.
///
/// Without `dir`, or when the file can't be made, events go to stderr only.
pub fn init(dir: Option<&Path>) {
    let file = dir.map(|dir| {
        RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix("skvaller")
            .filename_suffix("log")
            .max_log_files(FILES_KEPT)
            .build(dir)
    });
    let (file, file_error) = match file {
        Some(Ok(appender)) => (Some(appender), None),
        Some(Err(e)) => (None, Some(e)),
        None => (None, None),
    };
    tracing_subscriber::registry()
        .with(
            file.map(|w| fmt::layer().with_ansi(false).with_writer(w))
                .with_filter(LevelFilter::INFO),
        )
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(LevelFilter::INFO),
        )
        // Only fails when already set, as in a test.
        .try_init()
        .ok();
    if let Some(e) = file_error {
        tracing::event!(
            name: "log.file.failure",
            Level::WARN,
            file.directory = %dir.map_or_else(String::new, |d| d.display().to_string()),
            error.message = %e,
            "logging to a file in {{file.directory}} failed: {{error.message}}",
        );
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::event!(
            name: "app.run.panic",
            Level::ERROR,
            panic.message = %info,
            "panicked: {{panic.message}}",
        );
        default_hook(info);
    }));
}
