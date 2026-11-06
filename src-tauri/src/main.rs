// Prevents an extra console window on Windows in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Drawn for Windows only; tested everywhere. Not `expect`: test builds use all of it.
#[cfg_attr(
    not(target_os = "windows"),
    allow(dead_code, reason = "the badge is drawn on Windows only")
)]
mod badge;
mod commands;
mod config;
mod sync_gate;

use tauri::menu::{Menu, MenuItem, MenuItemKind, PredefinedMenuItem};
use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Manager, WebviewWindowBuilder, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;

const SETTINGS_MENU_ITEM: &str = "settings";

fn main() {
    let builder = tauri::Builder::default();
    // macOS has an app menu, so Settings is there too, with ⌘,. Windows and Linux have no
    // menu bar; they open Settings from the sidebar button and Ctrl+, only.
    #[cfg(target_os = "macos")]
    let builder = builder.menu(menu);
    builder
        // First, so a second launch exits before it sets anything up. Two instances would
        // save over each other's config.json and sync the same caches.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(main) = app.get_webview_window("main") {
                let _ = main.unminimize();
                let _ = main.show();
                let _ = main.set_focus();
            }
        }))
        // Only article links open in the browser, through the reader. The app's own
        // window has no external links, so the plugin's click handler is off.
        .plugin(
            tauri_plugin_opener::Builder::new()
                .open_js_links_on_click(false)
                .build(),
        )
        .plugin(tauri_plugin_dialog::init())
        .on_menu_event(|app, event| {
            // Not while the startup error shows: Settings' commands need the state.
            if event.id() == SETTINGS_MENU_ITEM && app.try_state::<commands::AppState>().is_some() {
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
            if let Err(e) = start(app.handle()) {
                show_startup_error(app.handle(), &e.to_string());
            }
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
            commands::rebuild_cache,
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

/// Loads the state, starts the background sync and opens the main window.
fn start(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let state = commands::AppState::load(app)?;
    app.set_theme(state.theme().native());
    app.manage(state);
    // Before the sync, so a run that ends in the startup error doesn't sync first.
    create_main_window(app).map_err(|e| format!("Opening the main window failed: {e}"))?;
    commands::start_background_sync(app.clone());
    // Needs the main window, so the badge shows before the first sync ends.
    commands::update_badge(app);
    Ok(())
}

/// Tells the user why Skvaller can't start, then quits when they close the dialog.
///
/// A setup error would only panic, and release builds on Windows have no console to
/// show it in. The dialog can't block here: setup runs on the main thread, which has
/// to be free to show it.
fn show_startup_error(app: &AppHandle, error: &str) {
    eprintln!("Starting failed: {error}");
    let handle = app.clone();
    app.dialog()
        .message(error)
        .title("Skvaller can't start")
        .kind(MessageDialogKind::Error)
        .show(move |_| handle.exit(1));
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
#[cfg_attr(
    not(target_os = "macos"),
    expect(dead_code, reason = "only macOS has an app menu")
)]
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

#[cfg(test)]
mod tests {
    use tauri::ipc::{CallbackFn, InvokeBody};
    use tauri::test::{INVOKE_KEY, MockRuntime, get_ipc_response, mock_builder};
    use tauri::webview::InvokeRequest;
    use tauri::{WebviewUrl, WebviewWindow, WebviewWindowBuilder};

    /// The app's two windows, under the capabilities in `capabilities/`. App commands
    /// that pass them succeed, without running.
    fn windows() -> (WebviewWindow<MockRuntime>, WebviewWindow<MockRuntime>) {
        let app = mock_builder()
            .invoke_handler(|invoke| {
                invoke.resolver.resolve(());
                true
            })
            .build(tauri::generate_context!(test = true))
            .expect("the mock app builds");
        let window = |label| {
            WebviewWindowBuilder::new(&app, label, WebviewUrl::default())
                .build()
                .expect("the window opens")
        };
        (window("main"), window("settings"))
    }

    /// The app's own URL, which the capabilities apply to.
    const APP_URL: &str = if cfg!(windows) {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    };

    /// Calls `cmd` from `window` showing `url`, with no arguments.
    fn call(window: &WebviewWindow<MockRuntime>, url: &str, cmd: &str) -> Result<(), String> {
        let request = InvokeRequest {
            cmd: cmd.into(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: url.parse().expect("the URL parses"),
            body: InvokeBody::default(),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_owned(),
        };
        get_ipc_response(window, request)
            .map(drop)
            .map_err(|e| e.to_string())
    }

    /// Whether the capabilities let `window` call `cmd`. Any error but a refusal, such
    /// as missing arguments, means the call got past them. Panics on a command that
    /// doesn't exist, so a typo can't pass as a refusal.
    fn allowed(window: &WebviewWindow<MockRuntime>, cmd: &str) -> bool {
        match call(window, APP_URL, cmd) {
            Err(e) if e.contains("Plugin not found") || e.contains("Command not found") => {
                panic!("{cmd} isn't a command: {e}")
            }
            Err(e) => !e.contains(" not allowed"),
            Ok(()) => true,
        }
    }

    #[test]
    fn main_window_cannot_call_what_only_settings_uses() {
        let (main, settings) = windows();
        for cmd in [
            "update_account",
            "remove_account",
            "set_settings",
            "import_opml",
            "export_opml",
            "take_settings_pane",
        ] {
            assert!(!allowed(&main, cmd), "main may call {cmd}");
            assert!(allowed(&settings, cmd), "settings may not call {cmd}");
        }
    }

    #[test]
    fn settings_window_cannot_call_what_only_main_uses() {
        let (main, settings) = windows();
        for cmd in [
            "articles",
            "article_html",
            "set_read",
            "sync_now",
            "open_settings",
        ] {
            assert!(allowed(&main, cmd), "main may not call {cmd}");
            assert!(!allowed(&settings, cmd), "settings may call {cmd}");
        }
    }

    #[test]
    fn main_window_gets_only_the_core_permissions_it_uses() {
        let (main, _) = windows();
        for cmd in [
            "plugin:event|listen",
            "plugin:resources|close",
            "plugin:window|start_dragging",
        ] {
            assert!(allowed(&main, cmd), "main may not call {cmd}");
        }
        for cmd in [
            "plugin:event|emit",
            "plugin:window|close",
            "plugin:webview|create_webview_window",
            "plugin:app|app_hide",
        ] {
            assert!(!allowed(&main, cmd), "main may call {cmd}");
        }
    }

    #[test]
    fn a_remote_page_in_the_main_window_can_call_nothing() {
        let (main, _) = windows();
        let refused = call(&main, "https://evil.example/", "articles")
            .expect_err("a remote page may call articles");
        assert!(refused.contains("articles not allowed"), "{refused}");
    }
}
