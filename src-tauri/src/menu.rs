//! The macOS menu bar. Windows and Linux have none, so there `rebuild_accounts` does
//! nothing: the app has no menu.

// Rust guideline compliant 2026-10-09

use tauri::menu::{
    CheckMenuItem, Menu, MenuEvent, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu,
    WINDOW_SUBMENU_ID,
};
use tauri::{AppHandle, Manager};
use tracing::Level;

use crate::commands;

const SETTINGS_ITEM: &str = "settings";
const ACCOUNTS_SUBMENU: &str = "accounts";
const ADD_ACCOUNT_ITEM: &str = "add-account";
/// Prefixes an account's ID to make its item's ID.
const ACCOUNT_ITEM_PREFIX: &str = "account:";
/// Accounts past the ninth have no shortcut: ⌘1 to ⌘9, as in `accountShortcut` in
/// shortcuts.ts.
const ACCOUNT_SHORTCUTS: usize = 9;

/// The default menu, with Settings… after About and an empty Accounts menu before Window.
#[cfg_attr(
    not(target_os = "macos"),
    expect(dead_code, reason = "only macOS has a menu bar")
)]
pub fn build(app: &AppHandle) -> tauri::Result<Menu<tauri::Wry>> {
    let menu = Menu::default(app)?;
    let items = menu.items()?;
    if let Some(MenuItemKind::Submenu(app_menu)) = items.first() {
        let settings =
            MenuItem::with_id(app, SETTINGS_ITEM, "Settings…", true, Some("CmdOrCtrl+,"))?;
        app_menu.insert_items(&[&settings, &PredefinedMenuItem::separator(app)?], 2)?;
    }
    // Filled in by `rebuild_accounts` once the accounts are loaded.
    let accounts = Submenu::with_id(app, ACCOUNTS_SUBMENU, "Accounts", true)?;
    let window_at = items
        .iter()
        .position(|item| item.id() == WINDOW_SUBMENU_ID)
        .unwrap_or(items.len());
    menu.insert(&accounts, window_at)?;
    Ok(menu)
}

/// Lists the accounts in the Accounts menu, with a check by the active one.
///
/// Call it whenever an account is added, renamed or removed, or another becomes active.
/// Each rebuild runs whole on the main thread, so two can't interleave, and reads the
/// accounts there, so the last one shows the current state.
pub fn rebuild_accounts(app: &AppHandle) {
    let handle = app.clone();
    let queued = app.run_on_main_thread(move || {
        if let Err(e) = try_rebuild_accounts(&handle) {
            log_failure(&e);
        }
    });
    if let Err(e) = queued {
        log_failure(&e);
    }
}

fn log_failure(e: &tauri::Error) {
    tracing::event!(
        name: "menu.accounts.failure",
        Level::WARN,
        error.message = %e,
        "updating the Accounts menu failed: {{error.message}}",
    );
}

fn try_rebuild_accounts(app: &AppHandle) -> tauri::Result<()> {
    let Some(MenuItemKind::Submenu(submenu)) =
        app.menu().and_then(|menu| menu.get(ACCOUNTS_SUBMENU))
    else {
        return Ok(());
    };
    let (accounts, active) = app.state::<commands::AppState>().accounts_for_menu();
    for item in submenu.items()? {
        submenu.remove(&item)?;
    }
    for (i, account) in accounts.iter().enumerate() {
        let shortcut = (i < ACCOUNT_SHORTCUTS).then(|| format!("Cmd+{}", i + 1));
        let item = CheckMenuItem::with_id(
            app,
            format!("{ACCOUNT_ITEM_PREFIX}{}", account.id),
            // A lone `&` would mark a mnemonic.
            account.description.replace('&', "&&"),
            true,
            active.as_deref() == Some(account.id.as_str()),
            shortcut,
        )?;
        submenu.append(&item)?;
    }
    if !accounts.is_empty() {
        submenu.append(&PredefinedMenuItem::separator(app)?)?;
    }
    submenu.append(&MenuItem::with_id(
        app,
        ADD_ACCOUNT_ITEM,
        "Add Account…",
        true,
        None::<&str>,
    )?)?;
    Ok(())
}

/// Runs the menu item the user picked.
pub fn on_event(app: &AppHandle, event: &MenuEvent) {
    // Not while the startup error shows: the commands need the state.
    if app.try_state::<commands::AppState>().is_none() {
        return;
    }
    let id = event.id().as_ref();
    if id == SETTINGS_ITEM {
        let _ = commands::show_settings_window(app);
    } else if id == ADD_ACCOUNT_ITEM {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            let state = app.state::<commands::AppState>();
            let pane = Some("add-account".to_owned());
            let _ = commands::open_settings(app.clone(), state, pane).await;
        });
    } else if let Some(account) = id.strip_prefix(ACCOUNT_ITEM_PREFIX) {
        let (app, account) = (app.clone(), account.to_owned());
        tauri::async_runtime::spawn(async move {
            let state = app.state::<commands::AppState>();
            // The command tells every window, and so the menu, about the new account.
            if commands::set_active_account(app.clone(), state, account)
                .await
                .is_err()
            {
                // A check menu item checks itself when clicked: put the checks back.
                rebuild_accounts(&app);
            }
        });
    }
}
