//! What the main window does, and which overlay it has open.
//!
//! Every way to ask the window for something ends in [`Reading::run`] with a
//! [`Command`]: a key, through [`route`]; an item of the macOS menu bar; a button. Each
//! command's guards are there, so they hold however it was asked for.
//!
//! The decisions are plain functions of plain data, tested without a window, as
//! `overscroll` does for a pull: [`route`] decides what a key does, the [`Overlay`]
//! methods which overlay stays open, and [`leave`] what selecting a list resets.
//! `Reading` applies them to its signals.

use std::collections::HashMap;
use std::sync::Arc;

use dioxus_native::Windows;
use dioxus_native::prelude::*;
use skvaller_app::{App, FontSize};
use skvaller_core::Selection;

use super::manage::Manage;
use super::{Pane, Reading, Search};
use crate::settings::{Pane as SettingsPane, SettingsWindow};
use crate::shortcuts::{
    ACCOUNT_MODIFIER, ARROWS, Action, COMMAND_MODIFIER, account_index, highlight, key_name,
};

/// The popover, question or form open over the window: one at a time.
///
/// Search isn't one: its field stays open beside the list. Opening it closes these,
/// which would keep its keys, such as a paste, from it.
#[derive(Clone, Debug, Default, PartialEq)]
pub(super) enum Overlay {
    #[default]
    None,
    /// The account menu, with its highlighted account.
    AccountMenu { at: usize },
    /// The mark-all-read question.
    MarkAll,
    /// The feed or folder menu, rename, question or Add Feed in the sidebar.
    Manage(Manage),
}

impl Overlay {
    /// Add Feed while its feed is fetched: closing would drop the result, so nothing
    /// takes its place.
    pub(super) fn busy(&self) -> bool {
        matches!(self, Self::Manage(Manage::AddFeed { busy: true }))
    }

    /// `next` in this one's place, or `None` while this one is busy.
    pub(super) fn open(&self, next: Self) -> Option<Self> {
        (!self.busy()).then_some(next)
    }

    /// Has a field or button that may have the focus, which goes when it closes.
    pub(super) fn holds_focus(&self) -> bool {
        !matches!(self, Self::None | Self::AccountMenu { .. })
    }

    /// What a click outside the overlay leaves open: a name being edited, which saves
    /// as its field loses the focus, and a busy Add Feed.
    pub(super) fn after_click(&self) -> Self {
        match self {
            Self::Manage(Manage::Rename(_) | Manage::AddFeed { busy: true }) => self.clone(),
            _ => Self::None,
        }
    }

    /// What hiding the sidebar leaves open: not its own popovers.
    pub(super) fn after_sidebar_hidden(&self) -> Self {
        match self {
            Self::AccountMenu { .. } | Self::Manage(_) if !self.busy() => Self::None,
            _ => self.clone(),
        }
    }

    /// What opening an article or another list leaves open: not the question about
    /// the list.
    pub(super) fn after_list_change(&self) -> Self {
        match self {
            Self::MarkAll => Self::None,
            _ => self.clone(),
        }
    }
}

/// Something the window can be asked to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Command {
    /// Open Settings, or bring it to the front.
    Settings,
    /// Open Settings at adding an account.
    AddAccount,
    /// Show the account with this ID.
    Account(String),
    /// Show the account at this place in the list: ⌘1 to ⌘9.
    AccountAt(usize),
    ToggleSidebar,
    /// A step larger or smaller font size, or M again: ⌘+, ⌘− and ⌘0.
    FontSize(FontStep),
    /// Open the search field, or focus it.
    Search,
    CloseSearch,
    /// A shortcut's action; `held` while its key repeats.
    Action {
        action: Action,
        held: bool,
    },
    /// The arrow keys: give the sidebar or the list the arrow keys, or step in them.
    FocusSidebar,
    FocusList,
    StepSidebar(isize),
    StepList(isize),
    /// Highlight the open menu's item at this index.
    Highlight(usize),
    /// Return on the open overlay: pick the menu's highlighted item, or answer yes.
    Accept,
    CloseOverlay,
}

/// How [`Command::FontSize`] changes the font size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FontStep {
    Larger,
    Smaller,
    Reset,
}

impl FontStep {
    /// The size after this change of `size`.
    fn apply(self, size: FontSize) -> FontSize {
        match self {
            Self::Larger => size.larger(),
            Self::Smaller => size.smaller(),
            Self::Reset => FontSize::M,
        }
    }

    /// The change `key` asks for with the command modifier, and Shift if `shift`, by
    /// the character it types, so it follows the keyboard's layout: `+` is a key of its
    /// own on some, and Shift and `=` on others, so `=` alone counts as `+`, as browsers
    /// have it. Only `+` may come with Shift: Shift and `0` types `=` on a Swedish
    /// keyboard.
    fn of(key: &Key, shift: bool) -> Option<Self> {
        let Key::Character(c) = key else {
            return None;
        };
        match (c.as_str(), shift) {
            ("+", _) | ("=", false) => Some(Self::Larger),
            ("-", false) => Some(Self::Smaller),
            ("0", false) => Some(Self::Reset),
            _ => None,
        }
    }
}

impl From<Action> for Command {
    fn from(action: Action) -> Self {
        Self::Action {
            action,
            held: false,
        }
    }
}

impl Command {
    /// Needs the panes, which a screen to add an account or sign in may replace. The
    /// others work there too: that screen has the account menu.
    const fn needs_panes(&self) -> bool {
        !matches!(
            self,
            Self::Settings
                | Self::AddAccount
                | Self::Account(_)
                | Self::AccountAt(_)
                | Self::FontSize(_)
                | Self::Highlight(_)
                | Self::Accept
                | Self::CloseOverlay
        )
    }
}

/// A key pressed in the window.
#[derive(Clone, Debug)]
pub(super) struct Press {
    pub key: Key,
    pub code: Code,
    pub mods: Modifiers,
    /// Repeated while held down.
    pub repeat: bool,
}

impl Press {
    pub(super) fn of(e: &KeyboardData) -> Self {
        Self {
            key: e.key(),
            code: e.code(),
            mods: e.modifiers(),
            repeat: e.is_auto_repeating(),
        }
    }
}

/// What the window has that decides what a key does.
pub(super) struct Context<'a> {
    pub overlay: &'a Overlay,
    /// The items of the open menu, if one is open.
    pub menu_items: usize,
    /// The list's unread articles: the mark-all-read question is for them, and is gone
    /// once there are none.
    pub unread: usize,
    pub search_open: bool,
    pub sidebar_open: bool,
    /// The pane the arrow keys move in.
    pub pane: Pane,
    /// The single-key shortcuts, by key name.
    pub bindings: &'a HashMap<String, Action>,
}

/// What a key comes to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Route {
    Run(Command),
    /// Taken, doing nothing, as a key a menu doesn't use.
    Eat,
    /// Not the window's: it goes on, as to the article.
    Pass,
}

/// What `press` does, in this order: the app's chords; the account chords; the open
/// overlay's keys; Escape closing the search; the arrow keys; the shortcuts.
pub(super) fn route(press: &Press, cx: &Context<'_>) -> Route {
    let Press {
        key,
        code,
        mods,
        repeat,
    } = press;
    let (key, mods, repeat) = (key, *mods, *repeat);
    // On macOS the menu bar takes ⌘, and the account chords first.
    if mods == COMMAND_MODIFIER {
        match code {
            Code::Comma => return Route::Run(Command::Settings),
            Code::KeyF => return Route::Run(Command::Search),
            _ => {}
        }
    }
    if mods == COMMAND_MODIFIER | Modifiers::SHIFT && *code == Code::KeyS {
        return Route::Run(Command::ToggleSidebar);
    }
    if mods - Modifiers::SHIFT == COMMAND_MODIFIER
        && let Some(step) = FontStep::of(key, mods.contains(Modifiers::SHIFT))
    {
        return Route::Run(Command::FontSize(step));
    }
    if let Some(index) = account_index(*code, mods) {
        return if repeat {
            Route::Eat
        } else {
            Route::Run(Command::AccountAt(index))
        };
    }
    if let Some(route) = overlay_key(key, mods, cx) {
        return route;
    }
    if *key == Key::Named(NamedKey::Escape) && cx.search_open {
        return Route::Run(Command::CloseSearch);
    }
    let Some(name) = key_name(key, &code.to_string(), mods) else {
        return Route::Pass;
    };
    if ARROWS.contains(&name.as_str()) {
        return arrow(key, repeat, cx);
    }
    let Some(&action) = cx.bindings.get(&name) else {
        return Route::Pass;
    };
    if repeat && !action.repeats() {
        return Route::Eat;
    }
    Route::Run(Command::Action {
        action,
        held: repeat,
    })
}

/// What `key` does to the open overlay; `None` without one, or a stale question.
fn overlay_key(key: &Key, mods: Modifiers, cx: &Context<'_>) -> Option<Route> {
    let named = |k| *key == Key::Named(k);
    let menu = |at| {
        if let Some(i) = highlight(at, cx.menu_items, key) {
            Some(Route::Run(Command::Highlight(i)))
        } else if named(NamedKey::Enter) {
            Some(Route::Run(Command::Accept))
        } else if named(NamedKey::Escape) || named(NamedKey::Tab) {
            Some(Route::Run(Command::CloseOverlay))
        } else {
            None
        }
    };
    let question = || {
        if named(NamedKey::Enter) {
            Some(Route::Run(Command::Accept))
        } else if named(NamedKey::Escape) {
            Some(Route::Run(Command::CloseOverlay))
        } else {
            None
        }
    };
    let (handled, rest) = match cx.overlay {
        Overlay::None => return None,
        // A sync may have emptied the list meanwhile: then the question is gone.
        Overlay::MarkAll if cx.unread == 0 => return None,
        Overlay::MarkAll => (question(), Route::Eat),
        Overlay::AccountMenu { at } => (menu(Some(*at)), Route::Eat),
        Overlay::Manage(manage) => {
            let handled = match manage {
                Manage::Menu { at, .. } => menu(*at),
                Manage::Confirm(_) => question(),
                Manage::Rename(_) | Manage::AddFeed { .. } => {
                    named(NamedKey::Escape).then_some(Route::Run(Command::CloseOverlay))
                }
            };
            // The app's and the accounts' chords not taken above go on, as to a
            // field.
            let chord = mods.intersects(COMMAND_MODIFIER | ACCOUNT_MODIFIER);
            (handled, if chord { Route::Pass } else { Route::Eat })
        }
    };
    Some(handled.unwrap_or(rest))
}

/// What an arrow key does: Left and Right choose the pane, Up and Down step in it.
fn arrow(key: &Key, repeat: bool, cx: &Context<'_>) -> Route {
    let delta = match key {
        Key::Named(NamedKey::ArrowLeft) => {
            // With the sidebar hidden, there is nothing to the left.
            return if cx.sidebar_open {
                Route::Run(Command::FocusSidebar)
            } else {
                Route::Eat
            };
        }
        Key::Named(NamedKey::ArrowRight) => return Route::Run(Command::FocusList),
        Key::Named(NamedKey::ArrowDown) => 1,
        _ => -1,
    };
    match cx.pane {
        // Each sidebar step loads a whole list: they don't repeat.
        Pane::Sidebar if repeat => Route::Eat,
        Pane::Sidebar => Route::Run(Command::StepSidebar(delta)),
        Pane::List => Route::Run(Command::StepList(delta)),
    }
}

/// What selecting another list changes besides the selection.
///
/// Always, too: the open article closes, its Reader View starts over and a load
/// under way is dropped, a delayed mark of it is off, the list a search replaced is
/// no longer the one to go back to, and a list sliding away goes at once.
#[derive(Debug, PartialEq)]
pub(super) struct Leave {
    pub overlay: Overlay,
    /// Search every feed, which is for the selection it was asked from.
    pub everywhere: bool,
}

/// What selecting `next` from `prev` leaves of the overlay and the search.
pub(super) fn leave(
    prev: &Selection,
    next: &Selection,
    overlay: &Overlay,
    search: &Search,
) -> Leave {
    Leave {
        overlay: overlay.after_list_change(),
        everywhere: search.everywhere && prev.scope == next.scope,
    }
}

impl Reading {
    /// Does `command`, if its guards allow it now. Every key, menu bar item and button
    /// that asks the window for something comes here.
    pub(crate) fn run(mut self, app: &Arc<App>, command: Command) {
        if command.needs_panes() && !self.panes_shown() {
            return;
        }
        match command {
            Command::Settings => {
                consume_context::<SettingsWindow>().open(&consume_context::<Windows>(), app);
            }
            Command::AddAccount => {
                if matches!(*self.overlay.peek(), Overlay::AccountMenu { .. }) {
                    self.overlay.set(Overlay::None);
                }
                consume_context::<SettingsWindow>().open_at(
                    &consume_context::<Windows>(),
                    app,
                    SettingsPane::AddAccount,
                );
            }
            Command::Account(id) => self.switch_account(app, &id),
            Command::AccountAt(index) => {
                if let Some(account) = app.accounts().get(index) {
                    self.switch_account(app, &account.id);
                }
            }
            Command::ToggleSidebar => self.toggle_sidebar(app),
            Command::FontSize(step) => {
                let now = app.settings().font_size;
                let size = step.apply(now);
                if size != now
                    && let Err(e) = app.change_settings(|s| s.font_size = size)
                {
                    tracing::error!("saving the font size failed: {e}");
                }
            }
            Command::Search => self.open_search(),
            Command::CloseSearch => self.close_search(),
            Command::Action { action, held } => self.act(app, action, held),
            Command::FocusSidebar => self.pane.set(Pane::Sidebar),
            Command::FocusList => {
                self.pane.set(Pane::List);
                if self.article.peek().is_none() {
                    self.step(app, 1);
                }
            }
            Command::StepSidebar(delta) => {
                self.step_sidebar(delta, app.settings().sort_subscriptions);
            }
            Command::StepList(delta) => self.step(app, delta),
            Command::Highlight(i) => {
                // Copied out: a borrow held into the block would make the write panic.
                let overlay = self.overlay.peek().clone();
                match overlay {
                    Overlay::AccountMenu { .. } => self.overlay.set(Overlay::AccountMenu { at: i }),
                    Overlay::Manage(Manage::Menu { target, x, y, .. }) => {
                        self.overlay.set(Overlay::Manage(Manage::Menu {
                            target,
                            x,
                            y,
                            at: Some(i),
                        }));
                    }
                    _ => {}
                }
            }
            Command::Accept => {
                let overlay = self.overlay.peek().clone();
                match overlay {
                    Overlay::AccountMenu { at } => {
                        if let Some(account) = app.accounts().get(at) {
                            self.switch_account(app, &account.id);
                        }
                    }
                    Overlay::MarkAll if self.panes_shown() => self.mark_all_read(app),
                    Overlay::Manage(Manage::Menu {
                        target,
                        at: Some(at),
                        ..
                    }) => self.pick_at(app, target, at),
                    Overlay::Manage(Manage::Confirm(target)) => self.remove(app, &target),
                    _ => {}
                }
            }
            Command::CloseOverlay => self.close_overlay(),
        }
    }

    /// Does a shortcut's action. `held` while its key repeats.
    fn act(mut self, app: &Arc<App>, action: Action, held: bool) {
        match action {
            Action::Next => {
                self.pane.set(Pane::List);
                self.step(app, 1);
            }
            Action::Previous => {
                self.pane.set(Pane::List);
                self.step(app, -1);
            }
            Action::ReadOn => self.read_on(app, true, held),
            Action::ReadBack => self.read_on(app, false, held),
            Action::ToggleReaderView => self.toggle_reader_view(app),
            Action::ToggleRead => self.toggle_read(app),
            Action::ToggleStar => self.toggle_starred(app),
            Action::OpenInBrowser => self.open_in_browser(),
            Action::Sync => drop(self.sync(app)),
            Action::MarkAllRead => self.request_mark_all(app),
        }
    }

    /// Acts on a key pressed anywhere in the window. Returns whether it did, so the key
    /// doesn't reach the article too.
    pub(super) fn on_key(
        self,
        app: &Arc<App>,
        bindings: &HashMap<String, Action>,
        e: &KeyboardData,
    ) -> bool {
        // Copied out: a borrow held while running the command would make its writes
        // panic.
        let overlay = self.overlay.peek().clone();
        let unread = self.unread_count();
        if overlay == Overlay::MarkAll && unread == 0 {
            self.close_overlay();
        }
        let cx = Context {
            menu_items: self.menu_items(app, &overlay),
            overlay: &overlay,
            unread,
            search_open: self.search.peek().open,
            sidebar_open: self.layout.peek().sidebar_open,
            pane: self.active_pane(),
            bindings,
        };
        match route(&Press::of(e), &cx) {
            Route::Run(command) => {
                self.run(app, command);
                true
            }
            Route::Eat => true,
            Route::Pass => false,
        }
    }

    /// The number of items in the open menu.
    fn menu_items(&self, app: &App, overlay: &Overlay) -> usize {
        match overlay {
            Overlay::AccountMenu { .. } => app.accounts().len(),
            Overlay::Manage(Manage::Menu { target, .. }) => self.manage_items(app, target).len(),
            _ => 0,
        }
    }

    /// Opens `next` in place of the open overlay, unless that one is busy. Returns
    /// whether it opened.
    pub(super) fn open_overlay(mut self, next: Overlay) -> bool {
        // Copied out: a borrow held into the write would make it panic.
        let opened = self.overlay.peek().open(next);
        match opened {
            Some(next) => {
                if *self.overlay.peek() != next {
                    self.overlay.set(next);
                }
                true
            }
            None => false,
        }
    }

    /// Closes the open overlay, unless it is busy. Focus goes back to the window, as
    /// the overlay's focused field or button goes with it.
    pub(super) fn close_overlay(self) {
        let focused = self.overlay.peek().holds_focus();
        if self.open_overlay(Overlay::None) && focused {
            self.refocus();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::manage::Target;
    use super::*;
    use crate::shortcuts::{Overrides, bindings};
    use skvaller_core::{Scope, View};

    fn press(key: Key, code: Code) -> Press {
        Press {
            key,
            code,
            mods: Modifiers::empty(),
            repeat: false,
        }
    }

    fn named(key: NamedKey) -> Press {
        let code = match key {
            NamedKey::ArrowDown => Code::ArrowDown,
            NamedKey::ArrowUp => Code::ArrowUp,
            NamedKey::ArrowLeft => Code::ArrowLeft,
            NamedKey::ArrowRight => Code::ArrowRight,
            NamedKey::Enter => Code::Enter,
            NamedKey::Escape => Code::Escape,
            NamedKey::Tab => Code::Tab,
            _ => Code::Unidentified,
        };
        press(Key::Named(key), code)
    }

    fn letter(c: char) -> Press {
        let code = format!("Key{}", c.to_ascii_uppercase())
            .parse()
            .unwrap_or(Code::Unidentified);
        press(Key::Character(c.to_string()), code)
    }

    fn chord(mods: Modifiers, code: Code, key: &str) -> Press {
        Press {
            mods,
            ..press(Key::Character(key.to_owned()), code)
        }
    }

    fn routed(overlay: &Overlay, unread: usize, p: &Press) -> Route {
        let keys = bindings(&Overrides::new());
        let cx = Context {
            overlay,
            menu_items: 3,
            unread,
            search_open: true,
            sidebar_open: true,
            pane: Pane::List,
            bindings: &keys,
        };
        route(p, &cx)
    }

    fn run(command: Command) -> Route {
        Route::Run(command)
    }

    fn feed_menu(at: Option<usize>) -> Overlay {
        Overlay::Manage(Manage::Menu {
            target: Target::Feed("1".to_owned()),
            x: 0.0,
            y: 0.0,
            at,
        })
    }

    #[test]
    fn with_nothing_open_keys_go_to_search_arrows_and_shortcuts() {
        let none = Overlay::None;
        assert_eq!(
            routed(&none, 1, &named(NamedKey::Escape)),
            run(Command::CloseSearch)
        );
        assert_eq!(
            routed(&none, 1, &named(NamedKey::ArrowDown)),
            run(Command::StepList(1))
        );
        assert_eq!(
            routed(&none, 1, &named(NamedKey::ArrowLeft)),
            run(Command::FocusSidebar)
        );
        assert_eq!(routed(&none, 1, &letter('j')), run(Action::Next.into()));
        assert_eq!(
            routed(&none, 1, &letter('a')),
            run(Action::MarkAllRead.into())
        );
        assert_eq!(routed(&none, 1, &letter('x')), Route::Pass);
        assert_eq!(routed(&none, 1, &named(NamedKey::Enter)), Route::Pass);
    }

    #[test]
    fn held_keys_repeat_only_the_moves() {
        let keys = bindings(&Overrides::new());
        let mut cx = Context {
            overlay: &Overlay::None,
            menu_items: 0,
            unread: 1,
            search_open: false,
            sidebar_open: false,
            pane: Pane::List,
            bindings: &keys,
        };
        let held = |p: Press| Press { repeat: true, ..p };
        assert_eq!(
            route(&held(letter('j')), &cx),
            run(Command::Action {
                action: Action::Next,
                held: true
            })
        );
        assert_eq!(route(&held(letter('s')), &cx), Route::Eat);
        // The sidebar's steps load whole lists.
        cx.pane = Pane::Sidebar;
        assert_eq!(
            route(&named(NamedKey::ArrowUp), &cx),
            run(Command::StepSidebar(-1))
        );
        assert_eq!(route(&held(named(NamedKey::ArrowUp)), &cx), Route::Eat);
        // Nothing to the left of a hidden sidebar.
        assert_eq!(route(&named(NamedKey::ArrowLeft), &cx), Route::Eat);
        // Escape with the search closed is nobody's.
        assert_eq!(route(&named(NamedKey::Escape), &cx), Route::Pass);
    }

    #[test]
    fn the_app_and_account_chords_come_before_any_overlay() {
        let menu = Overlay::AccountMenu { at: 0 };
        let command = COMMAND_MODIFIER;
        assert_eq!(
            routed(&menu, 1, &chord(command, Code::Comma, ",")),
            run(Command::Settings)
        );
        assert_eq!(
            routed(&menu, 1, &chord(command, Code::KeyF, "f")),
            run(Command::Search)
        );
        assert_eq!(
            routed(
                &menu,
                1,
                &chord(command | Modifiers::SHIFT, Code::KeyS, "S")
            ),
            run(Command::ToggleSidebar)
        );
        assert_eq!(
            routed(&menu, 1, &chord(command, Code::Equal, "=")),
            run(Command::FontSize(FontStep::Larger))
        );
        // `+` typed with Shift, or on a key of its own, as on a Swedish keyboard.
        assert_eq!(
            routed(
                &menu,
                1,
                &chord(command | Modifiers::SHIFT, Code::Equal, "+")
            ),
            run(Command::FontSize(FontStep::Larger))
        );
        assert_eq!(
            routed(&menu, 1, &chord(command, Code::Minus, "+")),
            run(Command::FontSize(FontStep::Larger))
        );
        assert_eq!(
            routed(&menu, 1, &chord(command, Code::Slash, "-")),
            run(Command::FontSize(FontStep::Smaller))
        );
        assert_eq!(
            routed(&menu, 1, &chord(command, Code::Digit0, "0")),
            run(Command::FontSize(FontStep::Reset))
        );
        // Shift and `0` on a Swedish keyboard: not a size.
        assert_eq!(
            routed(
                &Overlay::None,
                1,
                &chord(command | Modifiers::SHIFT, Code::Digit0, "=")
            ),
            Route::Pass
        );
        assert_eq!(FontStep::Larger.apply(FontSize::Xxxl), FontSize::Xxxl);
        assert_eq!(FontStep::Reset.apply(FontSize::Xs), FontSize::M);
        let second = chord(ACCOUNT_MODIFIER, Code::Digit2, "2");
        assert_eq!(
            routed(&feed_menu(None), 1, &second),
            run(Command::AccountAt(1))
        );
        assert_eq!(
            routed(
                &menu,
                1,
                &Press {
                    repeat: true,
                    ..second
                }
            ),
            Route::Eat
        );
    }

    #[test]
    fn an_open_menu_takes_the_keys() {
        let menu = Overlay::AccountMenu { at: 2 };
        assert_eq!(
            routed(&menu, 1, &named(NamedKey::ArrowDown)),
            run(Command::Highlight(0))
        );
        assert_eq!(
            routed(&menu, 1, &named(NamedKey::Enter)),
            run(Command::Accept)
        );
        assert_eq!(
            routed(&menu, 1, &named(NamedKey::Tab)),
            run(Command::CloseOverlay)
        );
        // Not the shortcuts, nor the search's Escape.
        assert_eq!(routed(&menu, 1, &letter('j')), Route::Eat);
        assert_eq!(
            routed(&menu, 1, &named(NamedKey::Escape)),
            run(Command::CloseOverlay)
        );

        let feeds = feed_menu(None);
        assert_eq!(
            routed(&feeds, 1, &named(NamedKey::ArrowUp)),
            run(Command::Highlight(2))
        );
        assert_eq!(routed(&feeds, 1, &letter('j')), Route::Eat);
        // A chord no one took goes on.
        assert_eq!(
            routed(&feeds, 1, &chord(COMMAND_MODIFIER, Code::KeyC, "c")),
            Route::Pass
        );
    }

    #[test]
    fn fields_in_the_sidebar_close_on_escape_and_keep_their_keys() {
        let rename = Overlay::Manage(Manage::Rename(Target::Folder("f".to_owned())));
        assert_eq!(
            routed(&rename, 1, &named(NamedKey::Escape)),
            run(Command::CloseOverlay)
        );
        assert_eq!(routed(&rename, 1, &letter('j')), Route::Eat);
        let adding = Overlay::Manage(Manage::AddFeed { busy: false });
        assert_eq!(routed(&adding, 1, &named(NamedKey::Enter)), Route::Eat);
        let confirm = Overlay::Manage(Manage::Confirm(Target::Feed("1".to_owned())));
        assert_eq!(
            routed(&confirm, 1, &named(NamedKey::Enter)),
            run(Command::Accept)
        );
    }

    #[test]
    fn the_mark_all_question_answers_return_and_escape_while_there_are_unread() {
        let question = Overlay::MarkAll;
        assert_eq!(
            routed(&question, 3, &named(NamedKey::Enter)),
            run(Command::Accept)
        );
        assert_eq!(
            routed(&question, 3, &named(NamedKey::Escape)),
            run(Command::CloseOverlay)
        );
        assert_eq!(routed(&question, 3, &letter('j')), Route::Eat);
        // A sync read them all meanwhile: the question is gone, keys work again.
        assert_eq!(routed(&question, 0, &letter('j')), run(Action::Next.into()));
        assert_eq!(routed(&question, 0, &named(NamedKey::Enter)), Route::Pass);
    }

    #[test]
    fn one_overlay_at_a_time_and_a_busy_add_feed_stays() {
        let menu = Overlay::AccountMenu { at: 0 };
        assert_eq!(menu.open(Overlay::MarkAll), Some(Overlay::MarkAll));
        let busy = Overlay::Manage(Manage::AddFeed { busy: true });
        assert_eq!(busy.open(Overlay::MarkAll), None);
        assert_eq!(busy.open(Overlay::None), None);
        assert_eq!(busy.after_click(), busy);
        assert_eq!(busy.after_sidebar_hidden(), busy);

        let rename = Overlay::Manage(Manage::Rename(Target::Feed("1".to_owned())));
        assert_eq!(rename.after_click(), rename);
        assert_eq!(feed_menu(Some(1)).after_click(), Overlay::None);
        assert_eq!(Overlay::MarkAll.after_click(), Overlay::None);
        assert_eq!(menu.after_click(), Overlay::None);

        // The question is in the list's toolbar, not the sidebar's.
        assert_eq!(Overlay::MarkAll.after_sidebar_hidden(), Overlay::MarkAll);
        assert_eq!(menu.after_sidebar_hidden(), Overlay::None);
        assert_eq!(feed_menu(None).after_sidebar_hidden(), Overlay::None);
    }

    #[test]
    fn selecting_another_list_drops_the_question_and_searching_everywhere() {
        let at = |scope: Scope, view: View| Selection { view, scope };
        let all = at(Scope::All, View::Unread);
        let feed = at(Scope::Feed { id: "1".to_owned() }, View::Unread);
        let everywhere = Search {
            open: true,
            query: "rust".to_owned(),
            everywhere: true,
        };
        assert_eq!(
            leave(&feed, &all, &Overlay::MarkAll, &everywhere),
            Leave {
                overlay: Overlay::None,
                everywhere: false
            }
        );
        // Another view of the same feed keeps searching everywhere, and the menus.
        let menu = Overlay::AccountMenu { at: 1 };
        assert_eq!(
            leave(
                &feed,
                &at(feed.scope.clone(), View::Starred),
                &menu,
                &everywhere
            ),
            Leave {
                overlay: menu,
                everywhere: true
            }
        );
    }
}
