// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;

use tauri::Manager;

fn main() {
    tauri::Builder::default()
        // Only article links open in the browser, through the reader. The app's own
        // window has no external links, so the plugin's click handler is off.
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running Skvaller");
}
