// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Drawn for Windows only; tested everywhere.
#[cfg_attr(not(target_os = "windows"), allow(dead_code))]
mod badge;
mod commands;
mod config;
mod sync_gate;

use tauri::menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem};
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_opener::OpenerExt;

const SETTINGS_MENU_ITEM: &str = "settings";

fn main() {
    let builder = tauri::Builder::default();
    // macOS has an app menu, so Settings is there too, with ⌘,. Windows and Linux have no
    // menu bar; they open Settings from the sidebar button and Ctrl+, only.
    #[cfg(target_os = "macos")]
    let builder = builder.menu(menu);
    builder
        // Only article links open in the browser, through the reader. The app's own
        // window has no external links, so the plugin's click handler is off.
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .on_menu_event(|app, event| {
            if event.id() == SETTINGS_MENU_ITEM {
                let _ = commands::show_settings_window(app);
            }
        })
        // Settings has no use without the main window, and no way to bring it back.
        .on_window_event(|window, event| {
            if window.label() == "main"
                && matches!(event, WindowEvent::Destroyed)
                && let Some(settings) = window.app_handle().get_webview_window("settings")
            {
                let _ = settings.close();
            }
        })
        .setup(|app| {
            let state = commands::AppState::load(app.handle())?;
            app.set_theme(state.theme().native());
            app.manage(state);
            commands::update_badge(app.handle());
            commands::start_background_sync(app.handle().clone());
            create_main_window(app.handle())?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::session,
            commands::accounts,
            commands::sign_in,
            commands::add_local_account,
            commands::set_active_account,
            commands::update_account,
            commands::remove_account,
            commands::sync_now,
            commands::library,
            commands::articles,
            commands::article_html,
            commands::full_text,
            commands::set_feed_reader_view,
            commands::set_read,
            commands::set_starred,
            commands::set_settings,
            commands::subscribe,
            commands::unsubscribe,
            commands::rename_feed,
            commands::move_feed,
            commands::create_folder,
            commands::rename_folder,
            commands::delete_folder,
            commands::import_opml,
            commands::export_opml,
            commands::open_settings,
            commands::take_settings_pane,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Skvaller");
}

/// The main window, from tauri.conf.json. Article links ask for a new window (see
/// `readyLinks` in articleHtml.ts): it is refused, and an http(s) link opens in the
/// system browser instead.
fn create_main_window(app: &AppHandle) -> tauri::Result<()> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == "main")
        .expect("tauri.conf.json defines the main window");
    let handle = app.clone();
    WebviewWindowBuilder::from_config(app, config)?
        .on_new_window(move |url, _| {
            if matches!(url.scheme(), "http" | "https")
                && let Err(e) = handle.opener().open_url(url.as_str(), None::<&str>)
            {
                eprintln!("Opening {url} failed: {e}");
            }
            NewWindowResponse::Deny
        })
        .build()?;
    Ok(())
}

/// The default menu, with Settings… after About in the app menu.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn menu(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::default(app)?;
    if let Some(MenuItemKind::Submenu(app_menu)) = menu.items()?.first() {
        let settings = MenuItem::with_id(
            app,
            SETTINGS_MENU_ITEM,
            "Settings…",
            true,
            Some("CmdOrCtrl+,"),
        )?;
        app_menu.insert_items(&[&settings, &PredefinedMenuItem::separator(app)?], 2)?;
    }
    Ok(menu)
}
