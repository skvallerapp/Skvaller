//! The macOS menu bar: the app menu with Settings…, the Accounts menu and the Window
//! menu. Windows and Linux have none.
//!
//! A menu item's key, such as ⌘, or ⌘1, reaches the menu before the window, so on macOS
//! the menu runs those shortcuts and the window never sees them.

use std::rc::Rc;
use std::sync::Arc;

use dioxus_native::prelude::*;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{AboutMetadata, CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
use skvaller_app::{Account, App};
use tokio::sync::mpsc;

use crate::i18n::t;

const SETTINGS_ITEM: &str = "settings";
const ADD_ACCOUNT_ITEM: &str = "add-account";
/// Prefixes an account's ID to make its item's ID.
const ACCOUNT_ITEM_PREFIX: &str = "account:";
/// The first nine accounts' keys, ⌘1 to ⌘9, as in the window's account shortcuts.
const ACCOUNT_KEYS: [Code; 9] = [
    Code::Digit1,
    Code::Digit2,
    Code::Digit3,
    Code::Digit4,
    Code::Digit5,
    Code::Digit6,
    Code::Digit7,
    Code::Digit8,
    Code::Digit9,
];

/// What a menu item asks the main window to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    Settings,
    AddAccount,
    /// Show the account with this ID.
    Account(String),
}

impl Command {
    fn from_id(id: &str) -> Option<Self> {
        match id {
            SETTINGS_ITEM => Some(Self::Settings),
            ADD_ACCOUNT_ITEM => Some(Self::AddAccount),
            _ => id
                .strip_prefix(ACCOUNT_ITEM_PREFIX)
                .map(|account| Self::Account(account.to_owned())),
        }
    }
}

/// The installed menu bar. The menu shows only while it lives.
struct MenuBar {
    _menu: Menu,
    accounts: Submenu,
}

impl MenuBar {
    /// Builds the menu bar and makes it the app's. The Accounts menu starts empty.
    fn install() -> muda::Result<Self> {
        let about = PredefinedMenuItem::about(
            Some(t("menu.about").as_str()),
            Some(AboutMetadata {
                name: Some("Skvaller".to_owned()),
                version: Some(env!("CARGO_PKG_VERSION").to_owned()),
                ..AboutMetadata::default()
            }),
        );
        let settings = MenuItem::with_id(
            SETTINGS_ITEM,
            t("menu.settings"),
            true,
            Some(Accelerator::new(Modifiers::META, Code::Comma)),
        );
        // The app menu's title shows as the app's name, whatever it says.
        let app_menu = Submenu::with_items(
            "Skvaller",
            true,
            &[
                &about,
                &PredefinedMenuItem::separator(),
                &settings,
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::services(Some(t("menu.services").as_str())),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::hide(Some(t("menu.hide").as_str())),
                &PredefinedMenuItem::hide_others(Some(t("menu.hideOthers").as_str())),
                &PredefinedMenuItem::show_all(Some(t("menu.showAll").as_str())),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::quit(Some(t("menu.quit").as_str())),
            ],
        )?;
        let accounts = Submenu::new(t("accounts.menu"), true);
        let window = Submenu::with_items(
            t("menu.window"),
            true,
            &[
                &PredefinedMenuItem::minimize(Some(t("menu.minimize").as_str())),
                &PredefinedMenuItem::zoom(Some(t("menu.zoom").as_str())),
                &PredefinedMenuItem::separator(),
                &PredefinedMenuItem::bring_all_to_front(Some(t("menu.bringAllToFront").as_str())),
            ],
        )?;
        let menu = Menu::with_items(&[&app_menu, &accounts, &window])?;
        menu.init_for_nsapp();
        // macOS lists the open windows at its end.
        window.set_as_windows_menu_for_nsapp();
        Ok(Self {
            _menu: menu,
            accounts,
        })
    }

    /// Lists the accounts in the Accounts menu, with a check by the shown one.
    fn show_accounts(&self, accounts: &[Account], shown: Option<&str>) -> muda::Result<()> {
        while self.accounts.remove_at(0).is_some() {}
        for (i, account) in accounts.iter().enumerate() {
            let item = CheckMenuItem::with_id(
                format!("{ACCOUNT_ITEM_PREFIX}{}", account.id),
                // A lone `&` would mark a mnemonic.
                account.description.replace('&', "&&"),
                true,
                shown == Some(account.id.as_str()),
                ACCOUNT_KEYS
                    .get(i)
                    .map(|&key| Accelerator::new(Modifiers::META, key)),
            );
            self.accounts.append(&item)?;
        }
        if !accounts.is_empty() {
            self.accounts.append(&PredefinedMenuItem::separator())?;
        }
        self.accounts.append(&MenuItem::with_id(
            ADD_ACCOUNT_ITEM,
            t("accounts.add"),
            true,
            None,
        ))
    }

    /// [`Self::show_accounts`], logging a failure: the menu then lists what it did.
    fn refresh(&self, app: &App, shown: Option<&str>) {
        if let Err(e) = self.show_accounts(&app.accounts(), shown) {
            tracing::warn!("updating the Accounts menu failed: {e}");
        }
    }
}

/// Installs the menu bar for the main window, whose root calls it once.
///
/// The Accounts menu follows the accounts and `shown`, the account the window shows.
/// `revision` is bumped on every account change. Each item picked runs `on_command`.
pub fn use_menu_bar(
    app: &Arc<App>,
    shown: Signal<Option<String>>,
    revision: Signal<u64>,
    mut on_command: impl FnMut(Command) + 'static,
) {
    let bar = use_hook(|| match MenuBar::install() {
        Ok(bar) => Some(Rc::new(bar)),
        Err(e) => {
            tracing::warn!("building the menu bar failed: {e}");
            None
        }
    });
    use_hook({
        let (app, bar) = (Arc::clone(app), bar.clone());
        move || {
            // Menu events come on the main thread, outside the window's tasks.
            let (sender, mut commands) = mpsc::unbounded_channel();
            MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
                // Closed only once the window is gone.
                let _ = sender.send(event.id.0);
            }));
            spawn(async move {
                while let Some(id) = commands.recv().await {
                    if let Some(command) = Command::from_id(&id) {
                        on_command(command);
                    }
                    // A check item checks or unchecks itself when picked, even the shown
                    // account's: put the checks back.
                    if let Some(bar) = &bar {
                        bar.refresh(&app, shown.peek().as_deref());
                    }
                }
            });
        }
    });
    let app = Arc::clone(app);
    use_effect(move || {
        revision();
        let shown = shown();
        if let Some(bar) = &bar {
            bar.refresh(&app, shown.as_deref());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_ids_name_their_commands() {
        assert_eq!(Command::from_id(SETTINGS_ITEM), Some(Command::Settings));
        assert_eq!(
            Command::from_id(ADD_ACCOUNT_ITEM),
            Some(Command::AddAccount)
        );
        assert_eq!(
            Command::from_id("account:3"),
            Some(Command::Account("3".to_owned()))
        );
        assert_eq!(Command::from_id("other"), None);
    }
}
