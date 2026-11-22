//! Skvaller, the desktop RSS reader: the native window.
//!
//! Usage: `skvaller [--import-opml <file> | --seed-test <count>]`. `--import-opml` adds
//! the file's feeds to the local account, creating it if there is none, then starts as
//! usual: a way to get articles onto a machine without accounts. `--seed-test` adds
//! that many synthetic articles instead, to test the list at 5,000+ items.

#![cfg_attr(all(not(test), target_os = "windows"), windows_subsystem = "windows")]

mod add_account;
mod appearance;
mod article;
mod chrome;
mod dates;
mod dropdown;
mod fonts;
mod i18n;
mod icons;
mod lines;
mod live;
#[cfg(target_os = "macos")]
mod menu;
mod overscroll;
mod place;
mod scale;
mod settings;
#[cfg(target_os = "linux")]
mod shadow;
mod shortcuts;
mod spin;
mod thumbs;
#[cfg(target_os = "windows")]
mod top_border;
mod ui;

use std::process::ExitCode;
use std::sync::Arc;

use dioxus_native::{Config, LogicalSize, WindowAttributes};
use mimalloc::MiMalloc;
use skvaller_app::{App, Dirs, import_local, seed_local};
use skvaller_core::ArticleNet;

#[global_allocator]
static GLOBAL: MiMalloc = MiMalloc;

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            // Stderr only: there is no native error dialog yet.
            eprintln!("Skvaller couldn't start: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    // Reads the system appearance while the app loads; the window waits for it below.
    appearance::start();
    let dirs = Dirs::system().map_err(|e| e.to_string())?;
    let mut args = std::env::args().skip(1);
    if let Some(flag) = args.next() {
        if flag == "--seed-test" {
            let count = args.next().ok_or("--seed-test needs a number")?;
            let count: usize = count
                .parse()
                .map_err(|e| format!("{count} is not a number: {e}"))?;
            let stored = seed_local(&dirs, count).map_err(|e| e.to_string())?;
            eprintln!("seeded {stored} test articles");
        } else if flag == "--import-opml" {
            let path = args.next().ok_or("--import-opml needs a file")?;
            let opml = std::fs::read(&path).map_err(|e| format!("reading {path} failed: {e}"))?;
            let report = import_local(&dirs, &opml).map_err(|e| e.to_string())?;
            eprintln!("imported from {path}: {report:?}");
        } else {
            return Err(format!(
                "unknown argument {flag}; use --import-opml <file> or --seed-test <count>"
            ));
        }
    }

    // A notice about config.json, logged already, shows in the main window.
    let app = Arc::new(App::load(&dirs).map_err(|e| e.to_string())?);

    // The syncs' own runtime, apart from the one Dioxus Native runs for loading images.
    let runtime = tokio::runtime::Runtime::new()
        .map_err(|e| format!("starting the sync runtime failed: {e}"))?;
    let entered = runtime.enter();
    app.start_sync();
    drop(entered);

    // Article HTML is untrusted, so every document's requests, the article frame's
    // included, may reach only public web addresses. The app's own UI loads only
    // cached thumbnails.
    let net = ArticleNet::new().map_err(|e| format!("building the HTTP client failed: {e}"))?;
    let (config, window) = chrome::window(
        Config::default(),
        WindowAttributes::default()
            .with_title("Skvaller")
            .with_surface_size(LogicalSize::new(1200.0, 800.0))
            .with_min_surface_size(LogicalSize::new(ui::MIN_WINDOW_WIDTH, 400.0)),
    );
    let config = config
        .with_font_ctx(fonts::font_context())
        .with_http_client(net.client())
        .with_request_filter(move |request| {
            request.url.scheme() == thumbs::SCHEME || net.allows(&request.url)
        })
        .with_protocol(
            thumbs::SCHEME,
            Arc::new(thumbs::Thumbnails(Arc::clone(&app))),
        );
    appearance::wait_for_first_answer();
    let context = Arc::clone(&app);
    dioxus_native::launch_cfg(
        ui::Root,
        vec![Box::new(move || Box::new(Arc::clone(&context)))],
        vec![Box::new(config), Box::new(window)],
    );
    drop(runtime);
    Ok(())
}
