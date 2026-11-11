//! Managing feeds and folders from the sidebar: the feed and folder menus, renaming in
//! place, the questions before removing, and Add Feed.
//!
//! Only the local account manages its own feeds, as in the Tauri version; a sync
//! account's feed menu has only Reader View. Blitz has no native menus, so these are
//! drawn in the page, as the account menu is.

use std::sync::Arc;

use dioxus_native::prelude::*;
use skvaller_app::{AccountKind, App, Library};
use skvaller_core::Scope;

use super::{ACCOUNT_MODIFIER, COMMAND_MODIFIER, Reading};
use crate::i18n::{sentence, t, t_with};
use crate::icons::{CHEVRONS_UP_DOWN, Icon, PLUS};

/// Heights in a menu, in CSS pixels, as app.css draws it: an item, a line between
/// groups with its margins, the Move To heading, and the menu's padding and borders.
/// They add up to what a menu needs to fit above the window's bottom edge.
const ITEM_HEIGHT: f64 = 28.0;
const SEPARATOR_HEIGHT: f64 = 11.0;
const HEADING_HEIGHT: f64 = 20.0;
const MENU_CHROME: f64 = 12.0;

/// The folder select's value for a folder made on the spot, which no folder ID can be.
const NEW_FOLDER: &str = "\u{0}new";

/// What the sidebar has open for managing feeds, one thing at a time.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum Manage {
    #[default]
    Closed,
    /// A feed's or folder's menu, at the pointer, with its highlighted item.
    Menu {
        target: Target,
        x: f64,
        y: f64,
        at: Option<usize>,
    },
    /// The feed's or folder's name is a field in its row.
    Rename(Target),
    /// The question before unsubscribing or deleting a folder.
    Confirm(Target),
    /// Add Feed, `busy` while the feed is fetched: it can't close then, as closing
    /// would drop the result.
    AddFeed { busy: bool },
}

/// A feed or folder of the shown account, by ID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Target {
    Feed(String),
    Folder(String),
}

impl Target {
    pub(super) fn of(scope: &Scope) -> Option<Self> {
        match scope {
            Scope::Feed { id } => Some(Self::Feed(id.clone())),
            Scope::Folder { id } => Some(Self::Folder(id.clone())),
            Scope::All => None,
        }
    }

    fn scope(&self) -> Scope {
        match self {
            Self::Feed(id) => Scope::Feed { id: id.clone() },
            Self::Folder(id) => Scope::Folder { id: id.clone() },
        }
    }

    /// Its name, if it is still there.
    fn title(&self, lib: &Library) -> Option<String> {
        match self {
            Self::Feed(id) => lib
                .feeds
                .iter()
                .find(|f| &f.id == id)
                .map(|f| f.title.clone()),
            Self::Folder(id) => lib
                .folders
                .iter()
                .find(|f| &f.id == id)
                .map(|f| f.title.clone()),
        }
    }
}

/// An item of a feed's or folder's menu.
#[derive(Clone, Debug, PartialEq)]
enum Item {
    ReaderView {
        on: bool,
    },
    Rename,
    /// Move To this folder, or out of any with `None`; `checked` where the feed is.
    Move {
        folder: Option<String>,
        title: String,
        checked: bool,
    },
    Unsubscribe,
    DeleteFolder,
}

/// The menu's items for `target`, as the Tauri version has them. Empty when it has none,
/// as a sync account's folder.
fn items(target: &Target, lib: &Library, local: bool) -> Vec<Item> {
    match target {
        Target::Feed(id) => {
            let Some(feed) = lib.feeds.iter().find(|f| &f.id == id) else {
                return Vec::new();
            };
            let mut items = vec![Item::ReaderView {
                on: feed.reader_view,
            }];
            if local {
                items.push(Item::Rename);
                let folders = lib
                    .folders
                    .iter()
                    .map(|f| (Some(f.id.clone()), f.title.clone()));
                items.extend(
                    std::iter::once((None, t("sidebar.noFolder")))
                        .chain(folders)
                        .map(|(folder, title)| Item::Move {
                            checked: folder == feed.folder_id,
                            folder,
                            title,
                        }),
                );
                items.push(Item::Unsubscribe);
            }
            items
        }
        Target::Folder(_) if local => vec![Item::Rename, Item::DeleteFolder],
        Target::Folder(_) => Vec::new(),
    }
}

/// Whether the account keeps its own feeds: the local account.
pub(super) fn is_local(app: &App, account_id: Option<&str>) -> bool {
    account_id.is_some_and(|id| {
        app.accounts()
            .iter()
            .any(|a| a.id == id && a.kind == AccountKind::Local)
    })
}

impl Reading {
    /// Opens `target`'s menu at the pointer, unless it has no items.
    pub(super) fn open_manage_menu(mut self, app: &App, target: Target, x: f64, y: f64) {
        let local = is_local(app, self.account.peek().as_deref());
        if items(&target, &self.library.peek(), local).is_empty() {
            return;
        }
        self.menu.set(None);
        self.confirming.set(false);
        self.manage.set(Manage::Menu {
            target,
            x,
            y,
            at: None,
        });
    }

    /// Closes what is open, unless Add Feed is busy. Focus goes back to the window,
    /// as the focused field or button goes.
    pub(super) fn close_manage(mut self) {
        if matches!(*self.manage.peek(), Manage::AddFeed { busy: true }) {
            return;
        }
        self.manage.set(Manage::Closed);
        self.refocus();
    }

    /// Does what the menu item says, for `target`.
    fn pick(mut self, app: &App, target: Target, item: &Item) {
        let Some(account) = self.account.peek().clone() else {
            return;
        };
        self.manage.set(Manage::Closed);
        let saved = match (item, &target) {
            (Item::ReaderView { on }, Target::Feed(id)) => {
                app.set_feed_reader_view(&account, id, !on)
            }
            (Item::Move { folder, .. }, Target::Feed(id)) => {
                let moved = app.move_feed(&account, id, folder.as_deref());
                // The feed shows where it went.
                if let Some(folder) = folder {
                    self.set_collapsed(folder, false);
                }
                // A folder's list gains or loses the feed's articles.
                self.revision += 1;
                moved
            }
            (Item::Rename, _) => {
                // A folder opens, showing what is being renamed.
                if let Target::Folder(id) = &target {
                    self.set_collapsed(id, false);
                }
                self.manage.set(Manage::Rename(target));
                return;
            }
            (Item::Unsubscribe | Item::DeleteFolder, _) => {
                self.manage.set(Manage::Confirm(target));
                return;
            }
            _ => Ok(()),
        };
        if let Err(e) = saved {
            tracing::error!("changing a feed failed: {e}");
        }
        self.refocus();
    }

    /// Saves the new name of the feed or folder being renamed, unless it is empty or
    /// unchanged, and closes the field. Does nothing once it is closed, as on the blur
    /// that follows Enter.
    fn finish_rename(mut self, app: &App, target: &Target, title: Option<&str>) {
        if *self.manage.peek() != Manage::Rename(target.clone()) {
            return;
        }
        self.close_manage();
        let (Some(account), Some(title)) = (self.account.peek().clone(), title.map(str::trim))
        else {
            return;
        };
        if title.is_empty() || target.title(&self.library.peek()).as_deref() == Some(title) {
            return;
        }
        let renamed = match target {
            Target::Feed(id) => app.rename_feed(&account, id, title),
            Target::Folder(id) => app.rename_folder(&account, id, title),
        };
        match renamed {
            // The list shows feeds' names.
            Ok(()) => self.revision += 1,
            Err(e) => tracing::error!("renaming failed: {e}"),
        }
    }

    /// Unsubscribes from the feed or deletes the folder, as just confirmed. A selected
    /// one gives way to All; any other list loads again, as it may have its articles.
    fn remove(mut self, app: &App, target: &Target) {
        self.close_manage();
        let Some(account) = self.account.peek().clone() else {
            return;
        };
        let removed = match target {
            Target::Feed(id) => app.unsubscribe(&account, id),
            Target::Folder(id) => app.delete_folder(&account, id),
        };
        if let Err(e) = removed {
            tracing::error!("removing a feed or folder failed: {e}");
            return;
        }
        if self.selection.peek().scope == target.scope() {
            self.select_scope(Scope::All);
        } else {
            self.revision += 1;
        }
    }

    /// Acts on a key while something is open here. Returns whether it did: then the key
    /// goes nowhere else. The fields take their own keys first.
    pub(super) fn on_manage_key(mut self, app: &App, key: &Key, mods: Modifiers) -> bool {
        let manage = self.manage.peek().clone();
        // The app's and the accounts' shortcuts still work, except over the fields.
        let shortcut = mods.intersects(COMMAND_MODIFIER | ACCOUNT_MODIFIER);
        match manage {
            Manage::Closed => false,
            Manage::Menu {
                target, x, y, at, ..
            } => {
                let local = is_local(app, self.account.peek().as_deref());
                let items = items(&target, &self.library.peek(), local);
                let count = items.len();
                let next = |step: usize| {
                    at.map_or(if step == 1 { 0 } else { count - 1 }, |i| {
                        (i + step) % count
                    })
                };
                match key {
                    Key::Named(NamedKey::ArrowDown) if count > 0 => {
                        let at = Some(next(1));
                        self.manage.set(Manage::Menu { target, x, y, at });
                    }
                    Key::Named(NamedKey::ArrowUp) if count > 0 => {
                        let at = Some(next(count - 1));
                        self.manage.set(Manage::Menu { target, x, y, at });
                    }
                    Key::Named(NamedKey::Enter) => {
                        if let Some(item) = at.and_then(|i| items.get(i)) {
                            self.pick(app, target, item);
                        }
                    }
                    Key::Named(NamedKey::Escape | NamedKey::Tab) => self.close_manage(),
                    _ => return !shortcut,
                }
                true
            }
            Manage::Confirm(target) => {
                match key {
                    Key::Named(NamedKey::Escape) => self.close_manage(),
                    Key::Named(NamedKey::Enter) => self.remove(app, &target),
                    _ => return !shortcut,
                }
                true
            }
            Manage::Rename(_) | Manage::AddFeed { .. } => {
                if *key == Key::Named(NamedKey::Escape) {
                    self.close_manage();
                    return true;
                }
                !shortcut
            }
        }
    }
}

/// The open feed or folder menu, at the pointer.
#[component]
pub(super) fn ManageMenu() -> Element {
    let app = use_context::<Arc<App>>();
    let mut reading = use_context::<Reading>();
    let Manage::Menu { target, x, y, at } = (reading.manage)() else {
        return rsx! {};
    };
    let local = is_local(&app, reading.account.read().as_deref());
    let items = items(&target, &(reading.library)(), local);
    let move_heading = items.iter().position(|i| matches!(i, Item::Move { .. }));
    // A line goes above each group but the first: Rename, Move To and Unsubscribe.
    let separated: Vec<bool> = (0..items.len())
        .map(|i| {
            i > 0
                && (matches!(items[i], Item::Rename | Item::Unsubscribe) || move_heading == Some(i))
        })
        .collect();
    // Moved up when there is no room below, as native menus are.
    #[expect(clippy::cast_precision_loss, reason = "a menu has few items")]
    let height = items.len() as f64 * ITEM_HEIGHT
        + separated.iter().filter(|s| **s).count() as f64 * SEPARATOR_HEIGHT
        + if move_heading.is_some() {
            HEADING_HEIGHT
        } else {
            0.0
        }
        + MENU_CHROME;
    let style = format!("left: {x}px; top: max(8px, min({y}px, calc(100vh - {height}px - 8px)))");

    rsx! {
        div { class: "menu popover context-menu", role: "menu", style,
            // The window closes the menu on any click that reaches it.
            onclick: move |e| e.stop_propagation(),
            oncontextmenu: move |e| e.prevent_default(),
            for (i , item) in items.iter().cloned().enumerate() {
                if separated[i] {
                    div { class: "separator", role: "separator" }
                }
                if move_heading == Some(i) {
                    div { class: "menu-heading", {t("sidebar.moveTo")} }
                }
                {
                    let (app, target) = (Arc::clone(&app), target.clone());
                    let (label, checked, role) = match &item {
                        Item::ReaderView { on } => (t("sidebar.alwaysReaderView"), *on, "menuitemcheckbox"),
                        Item::Rename => (t("sidebar.rename"), false, "menuitem"),
                        Item::Move { title, checked, .. } => (title.clone(), *checked, "menuitemradio"),
                        Item::Unsubscribe => (t("sidebar.unsubscribe"), false, "menuitem"),
                        Item::DeleteFolder => (t("sidebar.deleteFolder"), false, "menuitem"),
                    };
                    let mover_target = target.clone();
                    rsx! {
                        div {
                            key: "{i}",
                            class: if at == Some(i) { "menu-item highlighted" } else { "menu-item" },
                            role,
                            "aria-checked": "{checked}",
                            onmouseenter: move |_| {
                                let target = mover_target.clone();
                                reading.manage.set(Manage::Menu { target, x, y, at: Some(i) });
                            },
                            onclick: move |_| reading.pick(&app, target.clone(), &item),
                            span { class: "check",
                                if checked {
                                    svg { width: "14", height: "14", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2.5", stroke_linecap: "round", stroke_linejoin: "round",
                                        path { d: "M20 6 9 17l-5-5" }
                                    }
                                }
                            }
                            span { class: "name", "{label}" }
                        }
                    }
                }
            }
        }
    }
}

/// The question before unsubscribing from a feed or deleting a folder.
#[component]
pub(super) fn ConfirmRemove() -> Element {
    let app = use_context::<Arc<App>>();
    let reading = use_context::<Reading>();
    let Manage::Confirm(target) = (reading.manage)() else {
        return rsx! {};
    };
    let Some(title) = target.title(&(reading.library)()) else {
        return rsx! {};
    };
    let (question, message, confirm) = match target {
        Target::Feed(_) => (
            t_with("sidebar.unsubscribeTitle", &[("feed", &title)]),
            t("sidebar.unsubscribeMessage"),
            t("sidebar.unsubscribeConfirm"),
        ),
        Target::Folder(_) => (
            t_with("sidebar.deleteFolderTitle", &[("folder", &title)]),
            t("sidebar.deleteFolderMessage"),
            t("sidebar.deleteFolderConfirm"),
        ),
    };

    rsx! {
        div { class: "confirm popover sidebar-popover", role: "alertdialog", "aria-labelledby": "remove-question",
            onclick: move |e| e.stop_propagation(),
            p { id: "remove-question", "{question}" }
            p { class: "message", "{message}" }
            div { class: "actions",
                button { class: "button", onclick: move |_| reading.close_manage(), {t("common.cancel")} }
                button { class: "button destructive", onclick: move |_| reading.remove(&app, &target), "{confirm}" }
            }
        }
    }
}

/// The name of the feed or folder being renamed, as a field in its row. Enter or
/// leaving the field saves it; Escape leaves it as it was.
#[component]
pub(super) fn RenameField(target: Target, title: String) -> Element {
    let app = use_context::<Arc<App>>();
    let reading = use_context::<Reading>();
    let mut draft = use_signal(|| title.clone());
    let blur_app = Arc::clone(&app);
    let blur_target = target.clone();

    rsx! {
        input {
            class: "rename",
            aria_label: t("sidebar.name"),
            value: "{draft}",
            spellcheck: "false",
            onmounted: move |e: MountedEvent| async move {
                if let Err(err) = e.data().set_focus(true).await {
                    tracing::warn!("focusing the name field failed: {err}");
                }
            },
            oninput: move |e| draft.set(e.value()),
            // The row would select itself.
            onclick: move |e| e.stop_propagation(),
            onkeydown: move |e: Event<KeyboardData>| {
                if e.modifiers().intersects(COMMAND_MODIFIER | ACCOUNT_MODIFIER) {
                    return;
                }
                // Other keys typed here are the field's, not the window's.
                e.stop_propagation();
                match e.key() {
                    Key::Named(NamedKey::Enter) => reading.finish_rename(&app, &target, Some(&draft.peek())),
                    Key::Named(NamedKey::Escape) => reading.finish_rename(&app, &target, None),
                    _ => {}
                }
            },
            onblur: move |_| reading.finish_rename(&blur_app, &blur_target, Some(&draft.peek())),
        }
    }
}

/// Add Feed's button, for the local account, and its form while open.
#[component]
pub(super) fn AddFeed() -> Element {
    let app = use_context::<Arc<App>>();
    let mut reading = use_context::<Reading>();
    let manage = (reading.manage)();
    let open = matches!(manage, Manage::AddFeed { .. });
    let busy = manage == Manage::AddFeed { busy: true };
    let mut url = use_signal(String::new);
    let mut folder = use_signal(String::new);
    let mut new_folder = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let folders = (reading.library)().folders;

    let mut submit = move |app: Arc<App>| {
        let Some(account) = reading.account.peek().clone() else {
            return;
        };
        if *reading.manage.peek() != (Manage::AddFeed { busy: false }) {
            return;
        }
        let address = url.peek().trim().to_owned();
        if address.is_empty() {
            return;
        }
        let chosen = folder.peek().clone();
        let folder_id = if chosen == NEW_FOLDER {
            match app.create_folder(&account, &new_folder.peek()) {
                Ok(id) => Some(id),
                Err(e) => {
                    error.set(Some(sentence(&e.to_string())));
                    return;
                }
            }
        } else {
            Some(chosen).filter(|f| !f.is_empty())
        };
        error.set(None);
        reading.manage.set(Manage::AddFeed { busy: true });
        spawn(async move {
            let added = app.add_feed(&account, &address, folder_id.as_deref()).await;
            reading.manage.set(Manage::AddFeed { busy: false });
            match added {
                Ok(id) => {
                    url.set(String::new());
                    new_folder.set(String::new());
                    reading.close_manage();
                    // Another account showing now: the feed is in the one it went to.
                    if reading.account.peek().as_deref() == Some(account.as_str()) {
                        reading.select_scope(Scope::Feed { id });
                        reading.revision += 1;
                    }
                }
                Err(e) => error.set(Some(add_error(&e))),
            }
        });
    };
    let on_key = {
        let app = Arc::clone(&app);
        move |e: Event<KeyboardData>| {
            if e.modifiers()
                .intersects(COMMAND_MODIFIER | ACCOUNT_MODIFIER)
            {
                return;
            }
            e.stop_propagation();
            match e.key() {
                Key::Named(NamedKey::Enter) => submit(Arc::clone(&app)),
                Key::Named(NamedKey::Escape) => reading.close_manage(),
                _ => {}
            }
        }
    };
    let submit_app = Arc::clone(&app);

    rsx! {
        div { class: "add-feed",
            // The window closes the form on any click that reaches it.
            onclick: move |e| e.stop_propagation(),
            button {
                class: "icon-button",
                aria_label: t("addFeed.title"),
                title: t("addFeed.title"),
                "aria-haspopup": "dialog",
                "aria-expanded": "{open}",
                onclick: move |_| {
                    if open {
                        reading.close_manage();
                        return;
                    }
                    reading.menu.set(None);
                    reading.confirming.set(false);
                    error.set(None);
                    // The selected folder, if any, is where a new feed most likely goes.
                    folder.set(match &reading.selection.peek().scope {
                        Scope::Folder { id } => id.clone(),
                        _ => String::new(),
                    });
                    reading.manage.set(Manage::AddFeed { busy: false });
                },
                Icon { size: 18, stroke: "2", paths: PLUS }
            }
            if open {
                div { class: "popover sidebar-popover add-feed-form", role: "dialog", aria_label: t("addFeed.title"),
                    label {
                        span { {t("addFeed.address")} }
                        input {
                            value: "{url}",
                            placeholder: "example.com/feed.xml",
                            spellcheck: "false",
                            onmounted: move |e: MountedEvent| async move {
                                if let Err(err) = e.data().set_focus(true).await {
                                    tracing::warn!("focusing the address field failed: {err}");
                                }
                            },
                            oninput: move |e| url.set(e.value()),
                            onkeydown: on_key.clone(),
                        }
                    }
                    label {
                        span { {t("addFeed.folder")} }
                        div { class: "select",
                            select {
                                onchange: move |e| folder.set(e.value()),
                                option { value: "", selected: folder().is_empty(), {t("addFeed.noFolder")} }
                                for f in folders {
                                    option { key: "{f.id}", value: "{f.id}", selected: folder() == f.id, "{f.title}" }
                                }
                                option { value: NEW_FOLDER, selected: folder() == NEW_FOLDER, {t("addFeed.newFolderOption")} }
                            }
                            Icon { size: 14, stroke: "2", paths: CHEVRONS_UP_DOWN }
                        }
                    }
                    if folder() == NEW_FOLDER {
                        label {
                            span { {t("addFeed.newFolder")} }
                            input {
                                value: "{new_folder}",
                                oninput: move |e| new_folder.set(e.value()),
                                onkeydown: on_key.clone(),
                            }
                        }
                    }
                    if let Some(message) = error() {
                        p { class: "error", role: "alert", "{message}" }
                    }
                    div { class: "actions",
                        button { class: "button", onclick: move |_| reading.close_manage(), {t("common.cancel")} }
                        button {
                            class: "button primary",
                            disabled: busy || url().trim().is_empty() || (folder() == NEW_FOLDER && new_folder().trim().is_empty()),
                            onclick: move |_| submit(Arc::clone(&submit_app)),
                            if busy { {t("common.adding")} } else { {t("addFeed.add")} }
                        }
                    }
                }
            }
        }
    }
}

/// Why adding a feed failed, as the Tauri version says it.
fn add_error(e: &skvaller_app::Error) -> String {
    match e {
        skvaller_app::Error::Core(skvaller_core::Error::NoFeed) => t("addFeed.noFeed"),
        skvaller_app::Error::Core(skvaller_core::Error::BadUrl) => t("addFeed.badUrl"),
        skvaller_app::Error::Core(skvaller_core::Error::Http(_)) => t("addFeed.network"),
        e => sentence(&e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use skvaller_core::{Feed, Folder};

    fn library() -> Library {
        Library {
            folders: vec![Folder {
                id: "folder/1".into(),
                title: "Tech".into(),
            }],
            feeds: vec![Feed {
                id: "feed/1".into(),
                title: "A".into(),
                folder_id: Some("folder/1".into()),
                subscribed: true,
                icon: None,
                reader_view: true,
                url: None,
                site_url: None,
                fetch_error: None,
            }],
            ..Library::default()
        }
    }

    #[test]
    fn the_local_accounts_feed_menu_moves_renames_and_unsubscribes() {
        let items = items(&Target::Feed("feed/1".into()), &library(), true);
        assert_eq!(items[0], Item::ReaderView { on: true });
        assert_eq!(items[1], Item::Rename);
        assert!(matches!(
            &items[2],
            Item::Move {
                folder: None,
                checked: false,
                ..
            }
        ));
        assert!(
            matches!(&items[3], Item::Move { folder: Some(f), checked: true, .. } if f == "folder/1")
        );
        assert_eq!(items[4], Item::Unsubscribe);
    }

    #[test]
    fn a_sync_account_has_only_reader_view() {
        let lib = library();
        assert_eq!(
            items(&Target::Feed("feed/1".into()), &lib, false),
            [Item::ReaderView { on: true }]
        );
        assert_eq!(items(&Target::Folder("folder/1".into()), &lib, false), []);
        assert_eq!(
            items(&Target::Folder("folder/1".into()), &lib, true),
            [Item::Rename, Item::DeleteFolder]
        );
    }
}
