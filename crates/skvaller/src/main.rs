//! Skvaller, the desktop RSS reader: the native window.
//!
//! Usage: `skvaller [--import-opml <file>]`. The flag adds the file's feeds to the local
//! account, adding that account if there is none, then starts as usual: a way to get
//! articles onto a machine without accounts.

// Rust guideline compliant 2026-10-09

#![cfg_attr(all(not(test), target_os = "windows"), windows_subsystem = "windows")]

mod article;
mod i18n;
mod ui;

use std::process::ExitCode;
use std::sync::Arc;

use dioxus_native::{Config, LogicalSize, WindowAttributes};
use mimalloc::MiMalloc;
use skvaller_app::{App, Dirs, import_local};

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            // Until the app shows a native dialog, as the Tauri version did.
            eprintln!("Skvaller couldn't start: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let dirs = Dirs::system().map_err(|e| e.to_string())?;
    let mut args = std::env::args().skip(1);
    if let Some(flag) = args.next() {
        if flag != "--import-opml" {
            return Err(format!(
                "unknown argument {flag}; the only one is --import-opml <file>"
            ));
        }
        let path = args.next().ok_or("--import-opml needs a file")?;
        let opml =
            std::fs::read_to_string(&path).map_err(|e| format!("reading {path} failed: {e}"))?;
        let report = import_local(&dirs, &opml).map_err(|e| e.to_string())?;
        eprintln!("imported from {path}: {report:?}");
    }

    let app = Arc::new(App::load(&dirs).map_err(|e| e.to_string())?);
    if let Some(notice) = app.notice() {
        eprintln!("{}: {}", notice.title, notice.message);
    }

    // The syncs' own runtime, apart from the one Dioxus Native runs for loading images.
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| format!("starting the sync runtime failed: {e}"))?;
    let entered = runtime.enter();
    app.start_sync();
    drop(entered);

    let window = WindowAttributes::default()
        .with_title("Skvaller")
        .with_surface_size(LogicalSize::new(1200.0, 800.0))
        .with_min_surface_size(LogicalSize::new(640.0, 400.0));
    let context = Arc::clone(&app);
    dioxus_native::launch_cfg(
        ui::Root,
        vec![Box::new(move || Box::new(Arc::clone(&context)))],
        vec![Box::new(Config::default()), Box::new(window)],
    );
    drop(runtime);
    Ok(())
}
