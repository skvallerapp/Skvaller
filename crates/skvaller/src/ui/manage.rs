//! Managing feeds and folders from the sidebar: the feed and folder menus, renaming in
//! place, the questions before removing, and Add Feed.
//!
//! Only the local account manages its own feeds; a sync account's feed menu has only
//! Reader View. Blitz has no native menus, so these are drawn in the page, as the
//! account menu is.

use std::sync::Arc;

use dioxus_native::prelude::*;
use skvaller_app::{AccountKind, App, Library};
use skvaller_core::Scope;

use super::{Command, Overlay, Reading};
use crate::dropdown::Dropdown;
use crate::i18n::{sentence, t, t_with};
use crate::icons::{CHECK, Icon, PLUS};
use crate::shortcuts::field_key;
use crate::ui::focus;

/// Heights in a menu, in CSS pixels, as app.css draws it: an item, a line between
/// groups with its margins, the Move To heading, and the menu's padding and borders.
/// They add up to what a menu needs to fit above the window's bottom edge.
const ITEM_HEIGHT: f64 = 28.0;
const SEPARATOR_HEIGHT: f64 = 11.0;
const HEADING_HEIGHT: f64 = 20.0;
const MENU_CHROME: f64 = 12.0;

/// The folder select's value for a folder made on the spot, which no folder ID can be.
const NEW_FOLDER: &str = "\u{0}new";

/// What the sidebar has open for managing feeds, as the window's [`Overlay`].
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Manage {
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
            Self::Feed(id) => lib.feed(id).map(|f| f.title.clone()),
            Self::Folder(id) => lib.folder(id).map(|f| f.title.clone()),
        }
    }
}

/// An item of a feed's or folder's menu.
#[derive(Clone, Debug, PartialEq)]
pub(super) enum Item {
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

/// The menu's items for `target`. Empty when it has none, as for a sync account's
/// folder.
fn items(target: &Target, lib: &Library, local: bool) -> Vec<Item> {
    match target {
        Target::Feed(id) => {
            let Some(feed) = lib.feed(id) else {
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
    account_id
        .and_then(|id| app.account(id))
        .is_some_and(|a| a.kind == AccountKind::Local)
}

impl Reading {
    /// The items of `target`'s menu.
    pub(super) fn manage_items(&self, app: &App, target: &Target) -> Vec<Item> {
        let local = is_local(app, self.account.peek().as_deref());
        items(target, &self.library.peek(), local)
    }

    /// Opens `target`'s menu at the pointer, unless it has no items.
    pub(super) fn open_manage_menu(self, app: &App, target: Target, x: f64, y: f64) {
        if self.manage_items(app, &target).is_empty() {
            return;
        }
        self.open_overlay(Overlay::Manage(Manage::Menu {
            target,
            x,
            y,
            at: None,
        }));
    }

    /// Does what the item at `at` of `target`'s menu says.
    pub(super) fn pick_at(self, app: &App, target: Target, at: usize) {
        if let Some(item) = self.manage_items(app, &target).get(at) {
            self.pick(app, target, item);
        }
    }

    /// Does what the menu item says, for `target`.
    fn pick(mut self, app: &App, target: Target, item: &Item) {
        let Some(account) = self.account.peek().clone() else {
            return;
        };
        self.overlay.set(Overlay::None);
        let saved = match (item, &target) {
            // No reload here: `App` notifies the window of every feed change, including
            // those made in Settings.
            (Item::ReaderView { on }, Target::Feed(id)) => {
                app.set_feed_reader_view(&account, id, !on)
            }
            (Item::Move { folder, .. }, Target::Feed(id)) => {
                let moved = app.move_feed(&account, id, folder.as_deref());
                // The feed shows where it went.
                if let Some(folder) = folder {
                    self.set_collapsed(folder, false);
                }
                moved
            }
            (Item::Rename, _) => {
                // A folder opens, showing what is being renamed.
                if let Target::Folder(id) = &target {
                    self.set_collapsed(id, false);
                }
                self.overlay.set(Overlay::Manage(Manage::Rename(target)));
                return;
            }
            (Item::Unsubscribe | Item::DeleteFolder, _) => {
                self.overlay.set(Overlay::Manage(Manage::Confirm(target)));
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
    ///
    /// Leaves the focus alone: on a blur it has moved on already, as to the article a
    /// press went to. Enter and Escape give it back to the window.
    fn finish_rename(self, app: &App, target: &Target, title: Option<&str>) {
        if *self.overlay.peek() != Overlay::Manage(Manage::Rename(target.clone())) {
            return;
        }
        self.open_overlay(Overlay::None);
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
        if let Err(e) = renamed {
            tracing::error!("renaming failed: {e}");
        }
    }

    /// Unsubscribes from the feed or deletes the folder, as just confirmed. A selected
    /// one gives way to All.
    pub(super) fn remove(self, app: &App, target: &Target) {
        self.close_overlay();
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
        }
    }
}

/// The open feed or folder menu, at the pointer.
#[component]
pub(super) fn ManageMenu() -> Element {
    let app = use_context::<Arc<App>>();
    let reading = use_context::<Reading>();
    let Overlay::Manage(Manage::Menu { target, at, x, y }) = (reading.overlay)() else {
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
                    let (app, hover_app, target) = (Arc::clone(&app), Arc::clone(&app), target.clone());
                    let (label, checked, role) = match &item {
                        Item::ReaderView { on } => (t("sidebar.alwaysReaderView"), *on, "menuitemcheckbox"),
                        Item::Rename => (t("sidebar.rename"), false, "menuitem"),
                        Item::Move { title, checked, .. } => (title.clone(), *checked, "menuitemradio"),
                        Item::Unsubscribe => (t("sidebar.unsubscribe"), false, "menuitem"),
                        Item::DeleteFolder => (t("sidebar.deleteFolder"), false, "menuitem"),
                    };
                    rsx! {
                        div {
                            key: "{i}",
                            class: if at == Some(i) { "menu-item highlighted" } else { "menu-item" },
                            role,
                            "aria-checked": "{checked}",
                            onmouseenter: move |_| reading.run(&hover_app, Command::Highlight(i)),
                            onclick: move |_| reading.pick(&app, target.clone(), &item),
                            span { class: "check",
                                if checked {
                                    Icon { size: 14, stroke: "2.75", paths: CHECK }
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
    let Overlay::Manage(Manage::Confirm(target)) = (reading.overlay)() else {
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
    let cancel_app = Arc::clone(&app);

    rsx! {
        div { class: "confirm popover sidebar-popover", role: "alertdialog", "aria-labelledby": "remove-question",
            onclick: move |e| e.stop_propagation(),
            p { id: "remove-question", "{question}" }
            p { class: "message", "{message}" }
            div { class: "actions",
                button { class: "button", onclick: move |_| reading.run(&cancel_app, Command::CloseOverlay), {t("common.cancel")} }
                button { class: "button destructive", onclick: move |_| reading.run(&app, Command::Accept), "{confirm}" }
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
                focus(&e.data(), "the name field").await;
            },
            oninput: move |e| draft.set(e.value()),
            // Keeps the click from selecting the row.
            onclick: move |e| e.stop_propagation(),
            onkeydown: move |e: Event<KeyboardData>| {
                let Some(key) = field_key(&e) else { return };
                // The field has the focus, and goes.
                match key {
                    Key::Named(NamedKey::Enter) => {
                        reading.finish_rename(&app, &target, Some(&draft.peek()));
                        reading.refocus();
                    }
                    Key::Named(NamedKey::Escape) => {
                        reading.finish_rename(&app, &target, None);
                        reading.refocus();
                    }
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
    let state = use_memo(move || match *reading.overlay.read() {
        Overlay::Manage(Manage::AddFeed { busy }) => Some(busy),
        _ => None,
    })();
    let open = state.is_some();
    let busy = state == Some(true);
    let mut url = use_signal(String::new);
    let mut folder = use_signal(String::new);
    let mut new_folder = use_signal(String::new);
    let mut error = use_signal(|| None::<String>);
    let folders = reading.library.read().folders.clone();
    // The folder dropdown's choices: none, the folders, and a new one.
    let folder_ids: Vec<String> = std::iter::once(String::new())
        .chain(folders.iter().map(|f| f.id.clone()))
        .chain(std::iter::once(NEW_FOLDER.to_owned()))
        .collect();
    let folder_labels: Vec<String> = std::iter::once(t("addFeed.noFolder"))
        .chain(folders.iter().map(|f| f.title.clone()))
        .chain(std::iter::once(t("addFeed.newFolderOption")))
        .collect();

    let mut submit = move |app: Arc<App>| {
        let Some(account) = reading.account.peek().clone() else {
            return;
        };
        let adding = Overlay::Manage(Manage::AddFeed { busy: false });
        if *reading.overlay.peek() != adding {
            return;
        }
        if !ready(&url.peek(), &folder.peek(), &new_folder.peek()) {
            return;
        }
        let address = url.peek().trim().to_owned();
        let chosen = folder.peek().clone();
        // A folder made for this feed goes again if adding it fails. One with the same
        // title, ignoring case as the cache does, is the user's, and stays.
        let mut made = None;
        let folder_id = if chosen == NEW_FOLDER {
            let title = new_folder.peek().trim().to_owned();
            let existed = reading
                .library
                .peek()
                .folders
                .iter()
                .any(|f| f.title.eq_ignore_ascii_case(&title));
            match app.create_folder(&account, &title) {
                Ok(id) => {
                    if !existed {
                        made = Some(id.clone());
                    }
                    Some(id)
                }
                Err(e) => {
                    error.set(Some(sentence(&e.to_string())));
                    return;
                }
            }
        } else {
            Some(chosen).filter(|f| !f.is_empty())
        };
        error.set(None);
        let busy = Overlay::Manage(Manage::AddFeed { busy: true });
        reading.overlay.set(busy.clone());
        spawn(async move {
            let added = app.add_feed(&account, &address, folder_id.as_deref()).await;
            // Back to the idle form, unless showing another account closed it meanwhile.
            if *reading.overlay.peek() == busy {
                reading.overlay.set(adding);
            }
            match added {
                Ok(id) => {
                    url.set(String::new());
                    new_folder.set(String::new());
                    reading.close_overlay();
                    // Select the new feed only if its account is still the one shown.
                    if reading.account.peek().as_deref() == Some(account.as_str()) {
                        reading.select_scope(Scope::Feed { id });
                    }
                }
                Err(e) => {
                    if let Some(made) = made
                        && let Err(e) = app.delete_folder(&account, &made)
                    {
                        tracing::warn!("removing the folder made for a feed failed: {e}");
                    }
                    error.set(Some(add_error(&e)));
                }
            }
        });
    };
    let on_key = {
        let app = Arc::clone(&app);
        move |e: Event<KeyboardData>| {
            let Some(key) = field_key(&e) else { return };
            match key {
                Key::Named(NamedKey::Enter) => submit(Arc::clone(&app)),
                Key::Named(NamedKey::Escape) => reading.close_overlay(),
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
                        reading.close_overlay();
                        return;
                    }
                    if !reading.open_overlay(Overlay::Manage(Manage::AddFeed { busy: false })) {
                        return;
                    }
                    error.set(None);
                    // The selected folder, if any, is where a new feed most likely goes.
                    folder.set(match &reading.selection.peek().scope {
                        Scope::Folder { id } => id.clone(),
                        _ => String::new(),
                    });
                },
                Icon { size: 18, stroke: "2.25", paths: PLUS }
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
                                focus(&e.data(), "the address field").await;
                            },
                            oninput: move |e| url.set(e.value()),
                            onkeydown: on_key.clone(),
                        }
                    }
                    label {
                        span { {t("addFeed.folder")} }
                        Dropdown {
                            label: t("addFeed.folder"),
                            choices: folder_labels,
                            selected: folder_ids.iter().position(|id| *id == folder()).unwrap_or(0),
                            onchange: move |i: usize| {
                                if let Some(id) = folder_ids.get(i) {
                                    folder.set(id.clone());
                                }
                            },
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
                        button { class: "button", onclick: move |_| reading.close_overlay(), {t("common.cancel")} }
                        button {
                            class: "button primary",
                            disabled: busy || !ready(&url(), &folder(), &new_folder()),
                            onclick: move |_| submit(Arc::clone(&submit_app)),
                            if busy { {t("common.adding")} } else { {t("addFeed.add")} }
                        }
                    }
                }
            }
        }
    }
}

/// Whether the form can be sent: an address, and a name for a new folder.
fn ready(url: &str, folder: &str, new_folder: &str) -> bool {
    !url.trim().is_empty() && (folder != NEW_FOLDER || !new_folder.trim().is_empty())
}

/// Why adding a feed failed, as the user is told.
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
