//! Skvaller, the desktop RSS reader: the native window.
//!
//! Usage: `skvaller [--import-opml <file> | --seed-test <count>]`. The flag adds the file's feeds to the local
//! account, adding that account if there is none, then starts as usual: a way to get
//! articles onto a machine without accounts. `--seed-test` adds that many synthetic
//! articles instead, to test the list at 5,000+ items.

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
mod settings;
mod shortcuts;
mod thumbs;
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
            // Until the app shows a native dialog.
            eprintln!("Skvaller couldn't start: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    // Read while the app loads; the window waits for it below.
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

    let window = chrome::frame(
        WindowAttributes::default()
            .with_title("Skvaller")
            .with_surface_size(LogicalSize::new(1200.0, 800.0))
            .with_min_surface_size(LogicalSize::new(640.0, 400.0)),
    );
    // Every document's requests, the article frame's among them, may reach only public
    // web addresses: article HTML is untrusted. The app's own UI loads only cached
    // thumbnails, from the cache.
    let net = ArticleNet::new().map_err(|e| format!("building the HTTP client failed: {e}"))?;
    let config = Config::default()
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
