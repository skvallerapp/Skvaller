// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;

use tauri::menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem};
use tauri::{AppHandle, Manager, WindowEvent};

const SETTINGS_MENU_ITEM: &str = "settings";

fn main() {
    let builder = tauri::Builder::default();
    // macOS has an app menu, so Settings goes there. Windows and Linux have no menu bar;
    // the UI opens Settings from a button and Ctrl+, instead.
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
            app.manage(state);
            commands::start_background_sync(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::session,
            commands::sign_in,
            commands::sync_now,
            commands::library,
            commands::articles,
            commands::article_html,
            commands::set_read,
            commands::set_starred,
            commands::set_settings,
            commands::sign_out,
            commands::open_settings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Skvaller");
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
