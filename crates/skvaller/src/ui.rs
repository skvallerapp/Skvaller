//! The main window: sidebar, article list and reader.
//!
//! Everything shown is read from the local caches through [`App`]; nothing here waits
//! for the network. A sync that changes the cache bumps a revision, which reloads the
//! sidebar and the list. A change made here shows at once and is saved to the cache;
//! the next sync sends it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use dioxus_native::prelude::*;
use dioxus_native::{use_window_event, use_windows};
use skvaller_app::{App, Library, MarkReadOn, SyncStatus, ThumbnailSize};
use skvaller_core::{ArticleSummary, Scope, Selection, View};
use tokio::sync::broadcast::error::RecvError;

use crate::add_account::AddAccount;
use crate::article;
use crate::fonts;
use crate::i18n::{t, t_count, t_with};
use crate::icons::{CHEVRON_DOWN, CHEVRON_RIGHT, Icon, LAYERS, SETTINGS};
use crate::live::{Live, use_live};
use crate::settings::{Pane as SettingsPane, SettingsWindow};
use crate::shortcuts::{ARROWS, Action, key_name};
use crate::thumbs;

mod manage;

use manage::{AddFeed, ConfirmRemove, Manage, ManageMenu, RenameField, Target, is_local};

pub(crate) const APP_CSS: &str = include_str!("app.css");

/// Height of an article list row, in CSS pixels: the Tauri version's row with three
/// lines of headline. Rows are placed from these heights, never by measuring them.
const ROW_HEIGHT: f64 = 87.0;

/// What a Large thumbnail adds to its row: a gap above it, its height and the row's
/// bottom padding. It is about 16:9 of the row's width: the list's fixed 360 px less its
/// 1 px border and the rows' 20 px sides, 319 px.
const LARGE_THUMB_EXTRA: f64 = 8.0 + 180.0 + 11.0;

/// Rows rendered above and below the visible ones, so a fast scroll doesn't show gaps
/// before the next render.
const OVERSCAN: usize = 8;

/// The list's height before its first scroll event says what it is. Generous: rendering
/// a few rows too many costs little, too few shows a gap.
const INITIAL_VIEWPORT: f64 = 1600.0;

/// How long an article is open before it is marked read, with [`MarkReadOn::Delay`].
const MARK_READ_DELAY: Duration = Duration::from_secs(2);
/// How long an article of a Reader View feed waits for its text before showing the
/// feed's content, as in the Tauri version. Long enough for the cache, which most
/// pages come from after a sync's prefetch; short enough not to feel stuck.
const READER_VIEW_HOLD: Duration = Duration::from_millis(400);

/// How much of the article's height reading on scrolls: a page, less some lines to keep
/// the place, as in the Tauri version.
const READ_ON_PAGE: f64 = 0.9;

/// How near the end, in CSS pixels, counts as the end: scroll offsets can fall short of
/// it by a rounded fraction.
const END_SLACK: f64 = 1.0;

/// The modifier of the app's own shortcuts, such as Settings: ⌘ on macOS, Ctrl elsewhere.
const COMMAND_MODIFIER: Modifiers = if cfg!(target_os = "macos") {
    Modifiers::META
} else {
    Modifiers::CONTROL
};

/// The modifier of the account shortcuts, ⌘ on macOS and Alt elsewhere.
const ACCOUNT_MODIFIER: Modifiers = if cfg!(target_os = "macos") {
    Modifiers::META
} else {
    Modifiers::ALT
};

/// The column the arrow keys move in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Sidebar,
    List,
}

/// The article list as loaded, numbered so changes made on top of it know which load
/// they belong to.
#[derive(Clone, Debug, Default, PartialEq)]
struct Listed {
    load: u64,
    rows: Vec<ArticleSummary>,
}

/// Read and starred changes made here since the list loaded, kept on top of it, so an
/// article marked read stays in the Unread list until the list loads again. A new load
/// has them in its rows, from the cache, and starts these over.
#[derive(Clone, Debug, Default, PartialEq)]
struct Marks {
    /// The [`Listed::load`] these are on top of.
    load: u64,
    read: HashMap<String, bool>,
    starred: HashMap<String, bool>,
}

impl Marks {
    /// `a` with these changes, if they are on top of `load`.
    fn apply(&self, load: u64, a: &ArticleSummary) -> ArticleSummary {
        let mut a = a.clone();
        if self.load == load {
            a.read = self.read.get(&a.id).copied().unwrap_or(a.read);
            a.starred = self.starred.get(&a.id).copied().unwrap_or(a.starred);
        }
        a
    }
}

/// The open article's Reader View, as the Tauri version's store keeps it.
#[derive(Clone, Debug, Default, PartialEq)]
struct ReaderView {
    /// The account and article it is for.
    article: Option<(String, String)>,
    /// Asked for: the text shows instead of the feed's content once it is in.
    on: bool,
    /// The page's text, untrusted, once loaded.
    html: Option<String>,
    fetch: Fetch,
    /// Bumped by every load and every new article: an older load's result is dropped.
    request: u64,
}

/// Where loading the open article's Reader View stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Fetch {
    #[default]
    Idle,
    /// `awaiting`: just after the article opened, the frame waits for the text rather
    /// than show the feed's content and swap.
    Loading { awaiting: bool },
    /// The last load failed while Reader View was on. Shown until the next try.
    Failed,
}

/// What the window shows and what was changed here, shared by its parts.
#[derive(Clone, Copy, PartialEq)]
struct Reading {
    account: Signal<Option<String>>,
    selection: Signal<Selection>,
    /// The open article, as it was when opened: [`Reading::open_article`] has its flags
    /// now.
    article: Signal<Option<ArticleSummary>>,
    /// Bumped when the cache changed under the lists: they load again, keeping what was
    /// read or unstarred here meanwhile.
    revision: Signal<u64>,
    /// Bumped to load the list again without keeping those, as after marking all read.
    cleared: Signal<u64>,
    /// Bumped when a change made here changed the counts.
    counts: Signal<u64>,
    marks: Signal<Marks>,
    library: Memo<Library>,
    /// The feeds the Unread or Starred view's sidebar shows: those with articles in it
    /// when the account, view or selection was picked or a sync finished, and since.
    pinned: Memo<HashSet<String>>,
    articles: Memo<Listed>,
    pane: Signal<Pane>,
    /// The mark-all-read question is open.
    confirming: Signal<bool>,
    /// Bumped by every open and read change, so a delayed mark knows it is stale.
    mark_generation: Signal<u64>,
    /// The open article's frame, which "read on" scrolls.
    frame: Signal<Option<Rc<MountedData>>>,
    /// The shown account's sync status.
    status: Signal<Option<SyncStatus>>,
    /// The account menu, when open: the index of its highlighted account.
    menu: Signal<Option<usize>>,
    /// The window's root, which takes focus back when the focused element goes away.
    root: Signal<Option<Rc<MountedData>>>,
    reader_view: Signal<ReaderView>,
    /// The feed or folder menu, rename, question or Add Feed open in the sidebar.
    manage: Signal<Manage>,
    /// The ids of the folders whose feeds the sidebar hides, by account, for this
    /// session.
    collapsed: Signal<HashMap<String, HashSet<String>>>,
}

impl Reading {
    /// Whether `a`, a row of the list, is read, with the changes made here.
    fn is_read(&self, a: &ArticleSummary) -> bool {
        let marks = self.marks.read();
        let current = marks.load == self.articles.read().load;
        current
            .then(|| marks.read.get(&a.id).copied())
            .flatten()
            .unwrap_or(a.read)
    }

    fn is_starred(&self, a: &ArticleSummary) -> bool {
        let marks = self.marks.read();
        let current = marks.load == self.articles.read().load;
        current
            .then(|| marks.starred.get(&a.id).copied())
            .flatten()
            .unwrap_or(a.starred)
    }

    fn unread_count(&self) -> usize {
        self.articles
            .read()
            .rows
            .iter()
            .filter(|a| !self.is_read(a))
            .count()
    }

    /// The open article as the list has it now, which a sync may have changed since it
    /// opened. The list always has it.
    fn open_article(&self) -> Option<ArticleSummary> {
        let open = self.article.read().clone()?;
        let list = self.articles.read();
        Some(
            list.rows
                .iter()
                .find(|a| a.id == open.id)
                .cloned()
                .unwrap_or(open),
        )
    }

    /// Records changes made here on top of the list as loaded now.
    fn mark(mut self, ids: &[String], read: Option<bool>, starred: Option<bool>) {
        let load = self.articles.peek().load;
        let mut marks = self.marks.write();
        if marks.load != load {
            *marks = Marks {
                load,
                ..Marks::default()
            };
        }
        for id in ids {
            if let Some(read) = read {
                marks.read.insert(id.clone(), read);
            }
            if let Some(starred) = starred {
                marks.starred.insert(id.clone(), starred);
            }
        }
    }

    fn select(mut self, selection: Selection) {
        self.selection.set(selection);
        self.article.set(None);
        // Reader View starts over, as in the Tauri version, and a load under way is
        // dropped: reopening the article doesn't find it stuck loading.
        self.reset_reader_view(None);
        self.confirming.set(false);
        // A delayed mark of the article left is off.
        self.mark_generation += 1;
    }

    /// Shows `view`, keeping the scope.
    fn select_view(self, view: View) {
        let scope = self.selection.peek().scope.clone();
        if self.selection.peek().view != view {
            self.select(Selection { view, scope });
        }
    }

    /// Selects `scope` in the sidebar, keeping the view.
    fn select_scope(mut self, scope: Scope) {
        self.pane.set(Pane::Sidebar);
        let view = self.selection.peek().view;
        self.select(Selection { view, scope });
    }

    /// Gives focus back to the window's root, so keys still reach it after the focused
    /// element, such as the article's frame, goes away.
    fn refocus(self) {
        let Some(root) = self.root.peek().clone() else {
            return;
        };
        spawn(async move {
            if let Err(e) = root.set_focus(true).await {
                tracing::warn!("focusing the window's root failed: {e}");
            }
        });
    }

    /// Opens `a`, marking it read as the settings say.
    fn open(mut self, app: &Arc<App>, a: ArticleSummary) {
        let id = a.id.clone();
        let unread = !self.is_read(&a);
        self.follow_reader_view(app, &a);
        self.article.set(Some(a));
        self.confirming.set(false);
        self.mark_generation += 1;
        // The old article's frame, if focused, goes away with it.
        self.refocus();
        if !unread {
            return;
        }
        match app.settings().mark_read_on {
            MarkReadOn::Open => self.set_read(app, &[id], true),
            // Stepping on before then, as with j and k, leaves it unread.
            MarkReadOn::Delay => {
                let (app, generation) = (Arc::clone(app), *self.mark_generation.peek());
                spawn(async move {
                    tokio::time::sleep(MARK_READ_DELAY).await;
                    // Unless another article opened, or the setting changed, meanwhile.
                    if *self.mark_generation.peek() == generation
                        && app.settings().mark_read_on == MarkReadOn::Delay
                    {
                        self.set_read(&app, &[id], true);
                    }
                });
            }
            MarkReadOn::Manual => {}
        }
    }

    /// Starts Reader View afresh for `a`, about to open: on at once, waiting for the
    /// text, when its feed always uses it. The same article again keeps its state.
    fn follow_reader_view(self, app: &Arc<App>, a: &ArticleSummary) {
        let Some(account) = self.account.peek().clone() else {
            return;
        };
        let key = Some((account, a.id.clone()));
        if self.reader_view.peek().article == key {
            return;
        }
        self.reset_reader_view(key);
        let always = !a.url.is_empty()
            && self
                .library
                .peek()
                .feeds
                .iter()
                .any(|f| f.id == a.feed_id && f.reader_view);
        if always {
            self.load_full_text(app, false, true);
        }
    }

    /// Clears Reader View for `article`, or for none. Results of loads under way are
    /// dropped.
    fn reset_reader_view(mut self, article: Option<(String, String)>) {
        let request = self.reader_view.peek().request + 1;
        self.reader_view.set(ReaderView {
            article,
            request,
            ..ReaderView::default()
        });
    }

    /// Switches the open article between Reader View and the feed's content.
    fn toggle_reader_view(mut self, app: &Arc<App>) {
        if self
            .article
            .peek()
            .as_ref()
            .is_none_or(|a| a.url.is_empty())
        {
            return;
        }
        let load = {
            let mut rv = self.reader_view.write();
            rv.on = !rv.on;
            rv.on && rv.html.is_none() && !matches!(rv.fetch, Fetch::Loading { .. })
        };
        if load {
            self.load_full_text(app, true, false);
        }
    }

    /// Loads the open article's Reader View in the background. A failure turns it off
    /// and says so. `retry` fetches a page given up on; `hold` keeps the frame waiting
    /// for a moment.
    fn load_full_text(mut self, app: &Arc<App>, retry: bool, hold: bool) {
        let (key, request) = {
            let mut rv = self.reader_view.write();
            rv.request += 1;
            rv.on = true;
            rv.fetch = Fetch::Loading { awaiting: hold };
            (rv.article.clone(), rv.request)
        };
        let Some((account, id)) = key else {
            return;
        };
        // Not the calling component's tasks: the toolbar button that started a load goes
        // with the article, and the load must still finish or be dropped by `request`.
        if hold {
            dioxus_core::spawn_forever(async move {
                tokio::time::sleep(READER_VIEW_HOLD).await;
                // Copied out: a guard held into the block would make the write panic.
                let now = {
                    let rv = self.reader_view.peek();
                    (rv.request, rv.fetch)
                };
                if now == (request, Fetch::Loading { awaiting: true }) {
                    self.reader_view.write().fetch = Fetch::Loading { awaiting: false };
                }
            });
        }
        let app = Arc::clone(app);
        dioxus_core::spawn_forever(async move {
            let result = app.full_text(&account, &id, retry).await;
            if self.reader_view.peek().request != request {
                return;
            }
            let mut rv = self.reader_view.write();
            rv.fetch = Fetch::Idle;
            match result {
                Ok(Some(html)) => rv.html = Some(html),
                failed => {
                    if let Err(e) = failed {
                        tracing::warn!("Reader View of article {id} failed: {e}");
                    }
                    // Turned off meanwhile: nothing to say.
                    if rv.on {
                        rv.fetch = Fetch::Failed;
                    }
                    rv.on = false;
                }
            }
        });
    }

    /// Opens the next (1) or previous (-1) article in the list; with none open, the first.
    fn step(self, app: &Arc<App>, delta: isize) {
        let list = self.articles.peek();
        let open = self.article.peek().as_ref().map(|a| a.id.clone());
        let at = open.and_then(|id| list.rows.iter().position(|a| a.id == id));
        let next = match at {
            None => list.rows.first(),
            Some(i) => i.checked_add_signed(delta).and_then(|i| list.rows.get(i)),
        }
        .cloned();
        drop(list);
        if let Some(next) = next {
            self.open(app, next);
        }
    }

    /// Scrolls the article on or back by most of a page. At its end, reading on opens
    /// the next article, unless the key is held: then it stops there. With none open, it
    /// opens the first.
    fn read_on(self, app: &Arc<App>, on: bool, held: bool) {
        let frame = self.frame.peek().clone();
        let Some(frame) = frame.filter(|_| self.article.peek().is_some()) else {
            if on {
                self.step(app, 1);
            }
            return;
        };
        let (app, animate) = (Arc::clone(app), app.settings().animations);
        spawn(async move {
            // `range` is the largest offset, as Blitz has it for a frame, not the
            // document's height.
            let (Ok(offset), Ok(range), Ok(rect)) = (
                frame.get_scroll_offset().await,
                frame.get_scroll_size().await,
                frame.get_client_rect().await,
            ) else {
                // No frame, as for an article without a body: nothing to scroll.
                if on && !held {
                    self.step(&app, 1);
                }
                return;
            };
            if on && offset.y >= range.height - END_SLACK {
                // A frame just opened isn't laid out yet and looks scrolled to its end:
                // holding the key would run through the articles.
                if !held {
                    self.step(&app, 1);
                }
                return;
            }
            let page = rect.height() * READ_ON_PAGE;
            let to = dioxus_html::geometry::PixelsVector2D::new(
                offset.x,
                if on { offset.y + page } else { offset.y - page },
            );
            let behavior = if animate {
                ScrollBehavior::Smooth
            } else {
                ScrollBehavior::Instant
            };
            if let Err(e) = frame.scroll(to, behavior).await {
                tracing::warn!("scrolling the article failed: {e}");
            }
        });
    }

    fn set_read(mut self, app: &App, ids: &[String], read: bool) {
        let Some(account) = self.account.peek().clone() else {
            return;
        };
        self.mark_generation += 1;
        self.mark(ids, Some(read), None);
        if let Err(e) = app.set_read(&account, ids, read) {
            tracing::error!("saving a read change failed: {e}");
        }
        self.counts += 1;
    }

    fn toggle_read(self, app: &App) {
        if let Some(a) = self.open_article() {
            self.set_read(app, std::slice::from_ref(&a.id), !self.is_read(&a));
        }
    }

    fn toggle_starred(mut self, app: &App) {
        let (Some(a), Some(account)) = (self.open_article(), self.account.peek().clone()) else {
            return;
        };
        let starred = !self.is_starred(&a);
        self.mark(std::slice::from_ref(&a.id), None, Some(starred));
        if let Err(e) = app.set_starred(&account, &[a.id], starred) {
            tracing::error!("saving a star failed: {e}");
        }
        self.counts += 1;
    }

    /// Marks all as read, asking first when the settings say so.
    fn request_mark_all(mut self, app: &App) {
        if self.unread_count() == 0 {
            return;
        }
        if app.settings().confirm_mark_all_read {
            self.confirming.set(true);
        } else {
            self.mark_all_read(app);
        }
    }

    /// Marks every article in the list read. The Unread list then empties at once,
    /// except for the article being read.
    fn mark_all_read(mut self, app: &App) {
        self.close_confirm();
        let ids: Vec<String> = self
            .articles
            .peek()
            .rows
            .iter()
            .filter(|a| !self.is_read(a))
            .map(|a| a.id.clone())
            .collect();
        self.set_read(app, &ids, true);
        if self.selection.peek().view == View::Unread {
            self.cleared += 1;
        }
    }

    /// Closes the mark-all-read question; its focused button goes with it.
    fn close_confirm(mut self) {
        self.confirming.set(false);
        self.refocus();
    }

    fn open_in_browser(self) {
        let Some(a) = self.article.peek().clone() else {
            return;
        };
        if a.url.is_empty() {
            return;
        }
        if let Err(e) = webbrowser::open(&a.url) {
            tracing::error!("opening {} in the browser failed: {e}", a.url);
        }
    }

    fn sync(self, app: &Arc<App>) {
        let (Some(account), app) = (self.account.peek().clone(), Arc::clone(app)) else {
            return;
        };
        if let Err(e) = app.sync_now(&account) {
            tracing::error!("syncing failed: {e}");
        }
    }

    /// The shown account's collapsed folders, without subscribing to changes.
    fn collapsed_folders(&self) -> HashSet<String> {
        let account = self.account.peek();
        let collapsed = self.collapsed.peek();
        account
            .as_ref()
            .and_then(|id| collapsed.get(id))
            .cloned()
            .unwrap_or_default()
    }

    /// Whether the shown account's `folder` hides its feeds.
    fn is_collapsed(&self, folder: &str) -> bool {
        let account = self.account.read();
        let collapsed = self.collapsed.read();
        account
            .as_ref()
            .and_then(|id| collapsed.get(id))
            .is_some_and(|folders| folders.contains(folder))
    }

    /// Hides or shows the feeds of the shown account's `folder`.
    fn set_collapsed(mut self, folder: &str, collapse: bool) {
        let Some(account) = self.account.peek().clone() else {
            return;
        };
        let mut collapsed = self.collapsed.write();
        let folders = collapsed.entry(account).or_default();
        if collapse {
            folders.insert(folder.to_owned());
        } else {
            folders.remove(folder);
        }
    }

    /// Moves the sidebar's selection to the next (1) or previous (-1) scope, keeping
    /// the view, as the Tauri version does.
    fn step_sidebar(self, delta: isize) {
        let next = {
            let lib = self.library.peek();
            let collapsed = self.collapsed_folders();
            let pinned = self.pinned.peek();
            let Selection {
                view,
                scope: current,
            } = self.selection.peek().clone();
            // A feed in a collapsed folder has no row: step from its folder's.
            let row = match &current {
                Scope::Feed { id } => lib
                    .feeds
                    .iter()
                    .find(|f| &f.id == id)
                    .and_then(|f| f.folder_id.clone())
                    .filter(|folder| collapsed.contains(folder))
                    .map_or_else(|| current.clone(), |id| Scope::Folder { id }),
                _ => current.clone(),
            };
            let tree = sidebar_tree(&lib, view, &pinned, &current);
            let scopes = sidebar_scopes(&tree, &collapsed);
            scopes
                .iter()
                .position(|scope| *scope == row)
                .and_then(|i| i.checked_add_signed(delta))
                .and_then(|i| scopes.get(i))
                .cloned()
        };
        if let Some(scope) = next {
            self.select_scope(scope);
        }
    }

    /// Shows the account and makes it the active one.
    fn switch_account(mut self, app: &App, id: &str) {
        self.menu.set(None);
        if self.account.peek().as_deref() == Some(id) {
            return;
        }
        // Only the next start misses a choice that wasn't saved.
        if let Err(e) = app.set_active(id) {
            tracing::error!("saving account {id} as the one shown failed: {e}");
        }
        self.show_account(app, Some(id.to_owned()));
    }

    /// Shows the account, keeping the view: its feeds, articles and status. `None` once
    /// there is no account.
    fn show_account(mut self, app: &App, id: Option<String>) {
        // Another account's feeds are its own.
        self.manage.set(Manage::Closed);
        let view = self.selection.peek().view;
        self.select(Selection {
            view,
            scope: Scope::All,
        });
        self.status
            .set(id.as_deref().and_then(|id| app.status(id).ok()));
        self.account.set(id);
    }

    /// Up and Down move in the open account menu, Enter picks, Escape closes.
    fn on_menu_key(mut self, app: &App, key: &Key, at: usize) {
        let count = app.accounts().len();
        match key {
            Key::Named(NamedKey::ArrowDown) => self.menu.set(Some((at + 1) % count)),
            Key::Named(NamedKey::ArrowUp) => self.menu.set(Some((at + count - 1) % count)),
            Key::Named(NamedKey::Enter) => {
                if let Some(account) = app.accounts().get(at) {
                    self.switch_account(app, &account.id);
                }
            }
            Key::Named(NamedKey::Escape | NamedKey::Tab) => self.menu.set(None),
            _ => {}
        }
    }

    /// Acts on a key pressed anywhere in the window. Returns whether it did, so the key
    /// doesn't reach the article too.
    fn on_key(mut self, app: &Arc<App>, keys: &HashMap<String, Action>, e: &KeyboardData) -> bool {
        let (key, code) = (e.key(), e.code().to_string());
        if let Some(index) = account_index(&code, e.modifiers()) {
            if !e.is_auto_repeating()
                && let Some(account) = app.accounts().get(index)
            {
                self.switch_account(app, &account.id);
            }
            return true;
        }
        // Copied out: a borrow held into the block would make setting the menu panic.
        let menu = *self.menu.peek();
        if let Some(at) = menu {
            self.on_menu_key(app, &key, at);
            return true;
        }
        if self.on_manage_key(app, &key, e.modifiers()) {
            return true;
        }
        // A sync may have emptied the list meanwhile: then the question is gone.
        if *self.confirming.peek() && self.unread_count() == 0 {
            self.close_confirm();
        }
        if *self.confirming.peek() {
            match key {
                Key::Named(NamedKey::Escape) => self.close_confirm(),
                Key::Named(NamedKey::Enter) => self.mark_all_read(app),
                _ => {}
            }
            return true;
        }
        let Some(name) = key_name(&key, &code, e.modifiers()) else {
            return false;
        };
        let repeat = e.is_auto_repeating();
        if ARROWS.contains(&name.as_str()) {
            let pane = *self.pane.peek();
            let delta = match key {
                Key::Named(NamedKey::ArrowDown) => 1,
                Key::Named(NamedKey::ArrowUp) => -1,
                _ => 0,
            };
            match key {
                Key::Named(NamedKey::ArrowLeft) => self.pane.set(Pane::Sidebar),
                Key::Named(NamedKey::ArrowRight) => {
                    self.pane.set(Pane::List);
                    if self.article.peek().is_none() {
                        self.step(app, 1);
                    }
                }
                // Each sidebar step loads a whole list: they don't repeat.
                _ if pane == Pane::Sidebar && !repeat => self.step_sidebar(delta),
                _ if pane == Pane::List => self.step(app, delta),
                _ => {}
            }
            return true;
        }
        let Some(&action) = keys.get(&name) else {
            return false;
        };
        if repeat && !action.repeats() {
            return true;
        }
        match action {
            Action::Next => {
                self.pane.set(Pane::List);
                self.step(app, 1);
            }
            Action::Previous => {
                self.pane.set(Pane::List);
                self.step(app, -1);
            }
            Action::ReadOn => self.read_on(app, true, repeat),
            Action::ReadBack => self.read_on(app, false, repeat),
            // Reader View comes with a later milestone.
            Action::ToggleReaderView => self.toggle_reader_view(app),
            Action::ToggleRead => self.toggle_read(app),
            Action::ToggleStar => self.toggle_starred(app),
            Action::OpenInBrowser => self.open_in_browser(),
            Action::Sync => self.sync(app),
            Action::MarkAllRead => self.request_mark_all(app),
        }
        true
    }
}

/// The account a key switches to, by index: ⌘1 to ⌘9 on macOS, Alt+1 to Alt+9
/// elsewhere. Goes by the key's place, not its character, which depends on the layout.
fn account_index(code: &str, mods: Modifiers) -> Option<usize> {
    if mods != ACCOUNT_MODIFIER {
        return None;
    }
    let digit = code.strip_prefix("Digit")?.parse::<usize>().ok()?;
    (1..=9).contains(&digit).then(|| digit - 1)
}

/// The window's root component.
#[component]
pub fn Root() -> Element {
    let app = use_context::<Arc<App>>();
    let account = use_signal(|| app.active());
    let selection = use_signal(|| Selection {
        view: View::Unread,
        scope: Scope::All,
    });
    let article = use_signal(|| None::<ArticleSummary>);
    let mut revision = use_signal(|| 0_u64);
    let cleared = use_signal(|| 0_u64);
    let counts = use_signal(|| 0_u64);
    let marks = use_signal(Marks::default);
    let mut status = use_signal(|| account.peek().as_deref().and_then(|id| app.status(id).ok()));

    // Sync reports: the status for the sidebar, and a reload when articles changed.
    use_hook({
        let app = Arc::clone(&app);
        move || {
            spawn(async move {
                let mut events = app.subscribe();
                // A report sent before this listened, as by the first sync at start: its
                // status, and what it changed since the lists first loaded.
                let shown = account.peek().clone();
                if let Some(id) = shown {
                    status.set(app.status(&id).ok());
                    revision += 1;
                }
                loop {
                    match events.recv().await {
                        Ok(report) => {
                            if Some(&report.account_id) != account.peek().as_ref() {
                                continue;
                            }
                            if report.changed {
                                revision += 1;
                            }
                            status.set(Some(report));
                        }
                        // Missed some: reload, the latest status follows.
                        Err(RecvError::Lagged(_)) => revision += 1,
                        Err(RecvError::Closed) => break,
                    }
                }
            });
        }
    });

    // Before the library, which follows it: Settings changes feeds' Reader View.
    let live = use_live(&app);
    let library = use_memo({
        let app = Arc::clone(&app);
        move || {
            revision();
            counts();
            (live.revision)();
            account()
                .and_then(|id| app.library(&id).ok())
                .unwrap_or_default()
        }
    });
    // A feed read to zero keeps its row until the next of these, so rows don't jump
    // while you read, as the list keeps its articles.
    let pinned = use_memo({
        let held = Rc::new(RefCell::new((None, HashSet::new())));
        move || {
            let selection = selection();
            let last_sync = status.read().as_ref().and_then(|s| s.last_sync);
            let key = Some((account(), selection.clone(), last_sync));
            let lib = library.read();
            let counts = match selection.view {
                View::Starred => &lib.counts.starred_by_feed,
                View::Unread | View::All => &lib.counts.unread_by_feed,
            };
            let mut held = held.borrow_mut();
            if held.0 != key {
                *held = (key, HashSet::new());
            }
            held.1.extend(
                counts
                    .iter()
                    .filter(|(_, n)| **n > 0)
                    .map(|(id, _)| id.clone()),
            );
            held.1.clone()
        }
    });
    // The last load, what it listed and what it was for: a sync's reload keeps rows.
    let last = use_hook(|| Rc::new(RefCell::new((Listed::default(), None, 0_u64))));
    let articles = use_memo({
        let app = Arc::clone(&app);
        move || {
            revision();
            let (selection, account, cleared) = (selection(), account(), cleared());
            let mut rows = account
                .as_ref()
                .and_then(|id| app.articles(id, &selection).ok())
                .unwrap_or_default();
            let mut last = last.borrow_mut();
            let (before, last_for, last_cleared) = &*last;
            let marks = marks.peek();
            let open = article.peek().as_ref().map(|a| a.id.clone());
            let listed: HashSet<String> = rows.iter().map(|a| a.id.clone()).collect();
            // The changes made here are in the cache, and so in the rows; those of rows
            // the cache no longer lists here are only in the marks.
            let same = last_for.as_ref() == Some(&(account.clone(), selection.clone()));
            let keep = |a: &ArticleSummary| {
                // The open article stays, read or not, until another opens.
                if open.as_deref() == Some(a.id.as_str()) {
                    return true;
                }
                // After a sync, the Unread view keeps what was read here since it opened,
                // and the Starred view what was unstarred, so the list doesn't jump.
                let a = marks.apply(before.load, a);
                same && *last_cleared == cleared
                    && match selection.view {
                        View::Unread => a.read,
                        View::Starred => !a.starred,
                        View::All => false,
                    }
            };
            let kept: Vec<ArticleSummary> = before
                .rows
                .iter()
                .filter(|a| !listed.contains(&a.id) && keep(a))
                .map(|a| marks.apply(before.load, a))
                .collect();
            // The open article from another list, as after a new selection.
            let from_elsewhere = article
                .peek()
                .clone()
                .filter(|a| !listed.contains(&a.id) && !kept.iter().any(|k| k.id == a.id));
            if !kept.is_empty() || from_elsewhere.is_some() {
                rows.extend(kept);
                rows.extend(from_elsewhere.map(|a| marks.apply(before.load, &a)));
                rows.sort_by_key(|a| std::cmp::Reverse(a.published));
            }
            let listed = Listed {
                load: before.load + 1,
                rows,
            };
            *last = (listed.clone(), Some((account, selection)), cleared);
            listed
        }
    });
    let mut reading = use_context_provider(|| Reading {
        account,
        selection,
        article,
        revision,
        cleared,
        counts,
        marks,
        library,
        pinned,
        articles,
        pane: Signal::new(Pane::List),
        confirming: Signal::new(false),
        mark_generation: Signal::new(0),
        frame: Signal::new(None),
        status,
        menu: Signal::new(None),
        root: Signal::new(None),
        reader_view: Signal::new(ReaderView::default()),
        manage: Signal::new(Manage::default()),
        collapsed: Signal::new(HashMap::new()),
    });
    use_context_provider(|| live);
    // The active account, as changed elsewhere: one added or removed in Settings.
    use_effect({
        let app = Arc::clone(&app);
        move || {
            (live.revision)();
            let (active, shown) = (app.active(), reading.account.peek().clone());
            if active != shown {
                reading.show_account(&app, active);
                // The add-account screen, gone, took the focused field with it.
                reading.refocus();
            }
        }
    });
    let windows = use_windows();
    let settings = use_context_provider(SettingsWindow::default);
    // Settings goes with the main window, as the app does.
    use_window_event({
        let (settings, windows) = (settings.clone(), windows.clone());
        move |event, _| {
            if matches!(
                event,
                dioxus_native::winit::event::WindowEvent::CloseRequested
            ) {
                settings.close(&windows);
            }
        }
    });
    let signed_out = (reading.status)().is_some_and(|s| s.unauthorized);
    let adding = reading.account.peek().is_none() || signed_out;
    let settings_now = live.settings.read();
    let theme = article::theme_name(settings_now.theme);
    let ui_font = format!("--ui-font: {}", fonts::stack(settings_now.ui_font));
    drop(settings_now);

    rsx! {
        style { {APP_CSS} }
        // Focusable, and focused from the start: a key press goes to the focused element,
        // and a click keeps focus on the nearest focusable ancestor, so every key the
        // window gets passes through here. -1 keeps the root out of the Tab order.
        div {
            class: "root",
            "data-theme": theme,
            style: ui_font,
            tabindex: "-1",
            onmounted: move |e: MountedEvent| {
                // Without focus the app hears no keys, but works otherwise.
                reading.root.set(Some(e.data()));
                // The add-account screen focuses its first field instead.
                if !adding {
                    reading.refocus();
                }
            },
            onkeydown: move |e: Event<KeyboardData>| {
                if e.code() == Code::Comma && e.modifiers() == COMMAND_MODIFIER {
                    settings.open(&windows, &app);
                    e.prevent_default();
                } else if reading.on_key(&app, &live.keys.peek(), &e.data()) {
                    e.prevent_default();
                }
            },
            // A click outside the account menu, the mark-all-read question or a sidebar
            // popover closes it; their own clicks don't get here.
            onclick: move |_| {
                if reading.menu.peek().is_some() {
                    reading.menu.set(None);
                }
                if *reading.confirming.peek() {
                    reading.confirming.set(false);
                }
                if matches!(*reading.manage.peek(), Manage::Menu { .. } | Manage::Confirm(_) | Manage::AddFeed { busy: false }) {
                    reading.close_manage();
                }
            },
            if (reading.account)().is_none() || signed_out {
                AddAccountScreen {}
            } else {
                Sidebar {}
                ArticleList {}
                Reader {}
            }
        }
    }
}

/// The main window without an account to show: adding the first one, or signing in
/// again to the shown one after its server rejected the token. The other accounts stay
/// a click away.
#[component]
fn AddAccountScreen() -> Element {
    let app = use_context::<Arc<App>>();
    let reading = use_context::<Reading>();
    let signed_out = (reading.status)().is_some_and(|s| s.unauthorized);
    let again = (reading.account)()
        .filter(|_| signed_out)
        .and_then(|id| app.accounts().into_iter().find(|a| a.id == id));
    let error = again
        .as_ref()
        .map(|a| t_with("app.signInAgain", &[("account", &a.description)]));
    rsx! {
        div { class: "add-account-screen",
            AddAccount { embedded: false, again, initial_error: error, on_added: |_| {} }
            if app.accounts().len() > 1 {
                div { class: "other-accounts", AccountSwitcher {} }
            }
        }
    }
}

/// The subscribed feeds in `folder`, or outside every folder for `None`.
fn feeds_in<'a>(lib: &'a Library, folder: Option<&str>) -> Vec<&'a skvaller_core::Feed> {
    lib.feeds
        .iter()
        .filter(|f| f.subscribed && f.folder_id.as_deref() == folder)
        .collect()
}

/// The folders and feeds the sidebar shows, in its order.
#[derive(Debug)]
struct Tree<'a> {
    /// Each shown folder with its shown feeds, collapsed or not.
    folders: Vec<(&'a skvaller_core::Folder, Vec<&'a skvaller_core::Feed>)>,
    /// The shown feeds outside every folder.
    loose: Vec<&'a skvaller_core::Feed>,
}

/// What the sidebar shows of `lib` in `view`. All shows everything. Unread and Starred
/// show the `pinned` feeds and the folders with any, and always the selected `scope`.
fn sidebar_tree<'a>(
    lib: &'a Library,
    view: View,
    pinned: &HashSet<String>,
    scope: &Scope,
) -> Tree<'a> {
    let feeds = |folder: Option<&str>| -> Vec<_> {
        feeds_in(lib, folder)
            .into_iter()
            .filter(|f| {
                view == View::All
                    || pinned.contains(&f.id)
                    || matches!(scope, Scope::Feed { id } if *id == f.id)
            })
            .collect()
    };
    let folders = lib
        .folders
        .iter()
        .map(|folder| (folder, feeds(Some(&folder.id))))
        .filter(|(folder, feeds)| {
            view == View::All
                || !feeds.is_empty()
                || matches!(scope, Scope::Folder { id } if *id == folder.id)
        })
        .collect();
    Tree {
        folders,
        loose: feeds(None),
    }
}

/// The sidebar's scopes from top to bottom, as the arrow keys step through them: all
/// feeds, then each folder with its feeds unless `collapsed`, then the feeds outside
/// folders.
fn sidebar_scopes(tree: &Tree<'_>, collapsed: &HashSet<String>) -> Vec<Scope> {
    let mut scopes = vec![Scope::All];
    for (folder, feeds) in &tree.folders {
        scopes.push(Scope::Folder {
            id: folder.id.clone(),
        });
        if collapsed.contains(&folder.id) {
            continue;
        }
        for feed in feeds {
            scopes.push(Scope::Feed {
                id: feed.id.clone(),
            });
        }
    }
    for feed in &tree.loose {
        scopes.push(Scope::Feed {
            id: feed.id.clone(),
        });
    }
    scopes
}

#[component]
fn Sidebar() -> Element {
    let app = use_context::<Arc<App>>();
    let dates = (use_context::<Live>().dates)();
    let mut reading = use_context::<Reading>();
    let lib = (reading.library)();
    let pinned = (reading.pinned)();
    let unread_total: i64 = lib.counts.unread_by_feed.values().sum();
    let starred_total: i64 = lib.counts.starred_by_feed.values().sum();
    let current = (reading.selection)();
    let mut select = move |next: Selection| {
        reading.pane.set(Pane::Sidebar);
        reading.select(next);
    };
    let all_label = t(&format!("views.{}.all", view_key(current.view)));
    let all_count = match current.view {
        View::Starred => starred_total,
        View::Unread | View::All => unread_total,
    };
    let counts_for = |view: View, id: &str| match view {
        View::Unread | View::All => lib.counts.unread_by_feed.get(id).copied().unwrap_or(0),
        View::Starred => lib.counts.starred_by_feed.get(id).copied().unwrap_or(0),
    };
    let status = (reading.status)();
    let syncing = status.as_ref().is_some_and(|s| s.syncing);
    let sync_app = Arc::clone(&app);
    let settings = use_context::<SettingsWindow>();
    let windows = use_windows();
    let settings_app = Arc::clone(&app);
    let settings_key = if COMMAND_MODIFIER == Modifiers::META {
        "⌘,".to_owned()
    } else {
        format!("{}+,", t("keys.ctrl"))
    };
    let status_line = match status {
        // The Tauri version shows the reason on hover, but Blitz shows no titles yet.
        Some(SyncStatus {
            cache_broken: true,
            error,
            ..
        }) => match error {
            Some(e) => (format!("{}: {e}", t("sidebar.cacheDamaged")), true),
            None => (t("sidebar.cacheDamaged"), true),
        },
        Some(s) if s.syncing => (t("sidebar.syncing"), false),
        Some(SyncStatus { error: Some(e), .. }) => {
            (format!("{}: {e}", t("sidebar.syncFailed")), true)
        }
        Some(SyncStatus {
            last_sync: Some(ms),
            ..
        }) => (dates.synced(ms), false),
        _ => (t("sidebar.notSynced"), false),
    };
    let class = if (reading.pane)() == Pane::Sidebar {
        "sidebar active"
    } else {
        "sidebar"
    };
    let local = is_local(&app, reading.account.read().as_deref());
    let title = reading
        .account
        .read()
        .as_deref()
        .and_then(|id| app.accounts().into_iter().find(|a| a.id == id))
        .map(|a| a.description)
        .unwrap_or_default();
    let tree = sidebar_tree(&lib, current.view, &pinned, &current.scope);
    let FeedsPane(mut feeds_pane) = use_context_provider(|| FeedsPane(Signal::new(None)));
    // Every feed is at the top: selected, as by the arrow keys, it shows the top.
    use_effect(move || {
        if reading.selection.read().scope != Scope::All {
            return;
        }
        if let Some(pane) = feeds_pane.peek().clone() {
            spawn(async move {
                let top = dioxus_html::geometry::PixelsVector2D::new(0.0, 0.0);
                if let Err(e) = pane.scroll(top, ScrollBehavior::Instant).await {
                    tracing::warn!("scrolling the sidebar failed: {e}");
                }
            });
        }
    });

    rsx! {
        nav { class, aria_label: t("sidebar.label"),
            div { class: "toolbar",
                if local {
                    AddFeed {}
                }
                if app.accounts().len() > 1 {
                    AccountSwitcher {}
                }
                ConfirmRemove {}
                button {
                    class: "icon-button",
                    aria_label: t("sidebar.settings"),
                    title: t_with("sidebar.settingsWithKey", &[("key", settings_key.as_str())]),
                    onclick: move |_| settings.open(&windows, &settings_app),
                    Icon { size: 18, stroke: "2", paths: SETTINGS }
                }
                button {
                    class: if syncing { "icon-button sync syncing" } else { "icon-button sync" },
                    aria_label: t("sidebar.sync"),
                    title: t("sidebar.sync"),
                    onclick: move |_| reading.sync(&sync_app),
                    // Lucide's rotate-cw.
                    svg { width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8" }
                        path { d: "M21 3v5h-5" }
                    }
                }
            }
            header { class: "account",
                h1 { "{title}" }
                p { class: if status_line.1 { "failed" } else { "" }, "{status_line.0}" }
            }
            div { class: "feeds",
                onmounted: move |e: MountedEvent| feeds_pane.set(Some(e.data())),
                div {
                    class: if current.scope == Scope::All { "item selected" } else { "item" },
                    onclick: move |_| select(Selection { view: current.view, scope: Scope::All }),
                    span { class: "name", "{all_label}" }
                    if all_count > 0 { span { class: "count", "{all_count}" } }
                }
                div { class: "heading", {t("sidebar.feeds")} }
                for (folder, feeds) in tree.folders {
                    {
                        let scope = Scope::Folder { id: folder.id.clone() };
                        let count: i64 = feeds_in(&lib, Some(&folder.id)).iter().map(|f| counts_for(current.view, &f.id)).sum();
                        let expanded = !reading.is_collapsed(&folder.id);
                        let feeds: Vec<_> = if expanded {
                            feeds.into_iter().cloned().collect()
                        } else {
                            Vec::new()
                        };
                        rsx! {
                            ScopeItem { key: "folder-{folder.id}", scope, title: folder.title.clone(), count, nested: false, expanded }
                            for feed in feeds {
                                FeedItem { key: "{feed.id}", count: counts_for(current.view, &feed.id), feed, nested: true }
                            }
                        }
                    }
                }
                for feed in tree.loose.into_iter().cloned() {
                    FeedItem { key: "{feed.id}", count: counts_for(current.view, &feed.id), feed, nested: false }
                }
            }
            ViewSwitcher {}
            ManageMenu {}
        }
    }
}

/// The views, in the order of the sidebar's switcher.
const VIEWS: [View; 3] = [View::Unread, View::Starred, View::All];

/// The view's name in its text keys: `views.<name>.label` and the like.
fn view_key(view: View) -> &'static str {
    match view {
        View::Unread => "unread",
        View::Starred => "starred",
        View::All => "all",
    }
}

/// The switcher at the bottom of the sidebar: one button per view, which filters the
/// feeds' counts and the article list. The arrow keys move between them, as in a radio
/// group.
#[component]
fn ViewSwitcher() -> Element {
    let reading = use_context::<Reading>();
    let mut buttons = use_signal(|| [const { None::<Rc<MountedData>> }; VIEWS.len()]);
    let shown = (reading.selection)().view;

    rsx! {
        div {
            class: "views",
            role: "radiogroup",
            aria_label: t("sidebar.viewPicker"),
            onkeydown: move |e: Event<KeyboardData>| {
                // Back a step is round the other way.
                let step = match e.key() {
                    Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp) => VIEWS.len() - 1,
                    Key::Named(NamedKey::ArrowRight | NamedKey::ArrowDown) => 1,
                    _ => return,
                };
                // The window's own arrow keys would step the sidebar as well.
                e.stop_propagation();
                e.prevent_default();
                let view = reading.selection.peek().view;
                let at = VIEWS.iter().position(|v| *v == view).unwrap_or(0);
                let next = (at + step) % VIEWS.len();
                reading.select_view(VIEWS[next]);
                let Some(button) = buttons.peek()[next].clone() else {
                    return;
                };
                spawn(async move {
                    if let Err(e) = button.set_focus(true).await {
                        tracing::warn!("focusing the view's button failed: {e}");
                    }
                });
            },
            for (i , view) in VIEWS.into_iter().enumerate() {
                button {
                    key: "{view_key(view)}",
                    role: "radio",
                    aria_label: t(&format!("views.{}.label", view_key(view))),
                    title: t(&format!("views.{}.label", view_key(view))),
                    "aria-checked": "{view == shown}",
                    tabindex: if view == shown { "0" } else { "-1" },
                    onmounted: move |e: MountedEvent| buttons.write()[i] = Some(e.data()),
                    onclick: move |_| {
                        reading.select_view(view);
                        // As in the Tauri version, a clicked button lets the arrow keys
                        // go back to the sidebar and the list. Tab still reaches it.
                        reading.refocus();
                    },
                    match view {
                        // The unread mark, as on the reader's toolbar.
                        View::Unread => rsx! {
                            svg { width: "16", height: "16", view_box: "0 0 24 24", fill: "currentColor", stroke: "currentColor", stroke_width: "1.6", "aria-hidden": "true",
                                circle { cx: "12", cy: "12", r: "7.5" }
                            }
                        },
                        View::Starred => rsx! {
                            svg { width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
                                path { d: STAR_PATH }
                            }
                        },
                        View::All => rsx! { Icon { size: 16, stroke: "2", paths: LAYERS } },
                    }
                }
            }
        }
    }
}

/// The account menu's button, and the menu while open: each account with its unread
/// count and shortcut, the shown one checked.
#[component]
fn AccountSwitcher() -> Element {
    let app = use_context::<Arc<App>>();
    let mut reading = use_context::<Reading>();
    let settings = use_context::<SettingsWindow>();
    let windows = use_windows();
    let menu = (reading.menu)();
    let active = (reading.account)();
    // The counts change with changes made here and with syncs, as the library does.
    (reading.library)();
    let accounts = app.accounts();
    let menu_app = Arc::clone(&app);
    let app_for_add = Arc::clone(&app);
    let shortcut_key = if ACCOUNT_MODIFIER == Modifiers::META {
        "⌘".to_owned()
    } else {
        format!("{}+", t("keys.alt"))
    };

    rsx! {
        div { class: "switcher",
            button {
                class: "icon-button",
                aria_label: t("accounts.switch"),
                title: t("accounts.switch"),
                "aria-haspopup": "menu",
                "aria-expanded": "{menu.is_some()}",
                onclick: move |e| {
                    // The window closes the menu on any click that reaches it.
                    e.stop_propagation();
                    let at = reading.account.peek().as_ref().and_then(|id| app.accounts().iter().position(|a| &a.id == id));
                    reading.menu.set(if reading.menu.peek().is_some() { None } else { Some(at.unwrap_or(0)) });
                },
                // Lucide's rss.
                svg { width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                    path { d: "M4 11a9 9 0 0 1 9 9" }
                    path { d: "M4 4a16 16 0 0 1 16 16" }
                    circle { cx: "5", cy: "19", r: "1" }
                }
            }
            if let Some(at) = menu {
                div { class: "menu popover", role: "menu", aria_label: t("accounts.menu"),
                    onclick: move |e| e.stop_propagation(),
                    for (i , account) in accounts.into_iter().enumerate() {
                        {
                            let app = Arc::clone(&menu_app);
                            let checked = active.as_deref() == Some(account.id.as_str());
                            let count = app.unread_total(&account.id).unwrap_or(0);
                            let id = account.id.clone();
                            rsx! {
                                div {
                                    key: "{account.id}",
                                    class: if i == at { "menu-item highlighted" } else { "menu-item" },
                                    role: "menuitemradio",
                                    "aria-checked": "{checked}",
                                    onmouseenter: move |_| reading.menu.set(Some(i)),
                                    onclick: move |_| reading.switch_account(&app, &id),
                                    span { class: "check",
                                        if checked {
                                            svg { width: "14", height: "14", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2.5", stroke_linecap: "round", stroke_linejoin: "round",
                                                path { d: "M20 6 9 17l-5-5" }
                                            }
                                        }
                                    }
                                    span { class: "name", "{account.description}" }
                                    if count > 0 { span { class: "count", "{count}" } }
                                    if i < 9 { span { class: "shortcut", "{shortcut_key}{i + 1}" } }
                                }
                            }
                        }
                    }
                    div { class: "separator", role: "separator" }
                    div {
                        class: "menu-item",
                        role: "menuitem",
                        onclick: move |_| {
                            reading.menu.set(None);
                            settings.open_at(&windows, &app_for_add, SettingsPane::AddAccount);
                        },
                        span { class: "check" }
                        span { class: "name", {t("accounts.add")} }
                    }
                }
            }
        }
    }
}

#[component]
fn FeedItem(feed: skvaller_core::Feed, count: i64, nested: bool) -> Element {
    let scope = Scope::Feed { id: feed.id };
    rsx! {
        ScopeItem { scope, title: feed.title, count, nested, expanded: None }
    }
}

/// The sidebar's scrolling list of feeds, for its items to scroll themselves into.
#[derive(Clone, Copy)]
struct FeedsPane(Signal<Option<Rc<MountedData>>>);

/// A folder or feed in the sidebar. Selected, as by the arrow keys, it scrolls into view.
/// A folder, with `expanded`, has a disclosure button that shows or hides its feeds.
#[component]
fn ScopeItem(
    scope: Scope,
    title: String,
    count: i64,
    nested: bool,
    expanded: Option<bool>,
) -> Element {
    let app = use_context::<Arc<App>>();
    let reading = use_context::<Reading>();
    let target = Target::of(&scope);
    let renaming = target
        .as_ref()
        .filter(|t| *reading.manage.read() == Manage::Rename((*t).clone()))
        .cloned();
    let mut mounted = use_signal(|| None::<Rc<MountedData>>);
    let selected = reading.selection.read().scope == scope;
    let class = match (selected, nested) {
        (true, true) => "item nested selected",
        (true, false) => "item selected",
        (false, true) => "item nested",
        (false, false) => "item",
    };
    let FeedsPane(pane) = use_context::<FeedsPane>();
    // After the render, when the item has its place. Blitz scrolls only the window into
    // view, not the feeds' own scroll: done here.
    use_effect({
        let scope = scope.clone();
        move || {
            if reading.selection.read().scope != scope {
                return;
            }
            let (Some(item), Some(pane)) = (mounted.peek().clone(), pane.peek().clone()) else {
                return;
            };
            spawn(async move {
                let (Ok(item), Ok(view), Ok(offset)) = (
                    item.get_client_rect().await,
                    pane.get_client_rect().await,
                    pane.get_scroll_offset().await,
                ) else {
                    return;
                };
                // Blitz counts the pane's own scroll in its place too: undone here.
                let top = view.min_y() + offset.y;
                let bottom = top + view.height();
                let by = if item.min_y() < top {
                    item.min_y() - top
                } else if item.max_y() > bottom {
                    item.max_y() - bottom
                } else {
                    return;
                };
                let to = dioxus_html::geometry::PixelsVector2D::new(offset.x, offset.y + by);
                if let Err(e) = pane.scroll(to, ScrollBehavior::Instant).await {
                    tracing::warn!("scrolling the sidebar failed: {e}");
                }
            });
        }
    });
    rsx! {
        div {
            class,
            onmounted: move |e: MountedEvent| mounted.set(Some(e.data())),
            onclick: move |_| reading.select_scope(scope.clone()),
            oncontextmenu: move |e: Event<MouseData>| {
                e.prevent_default();
                if let Some(target) = target.clone() {
                    let at = e.client_coordinates();
                    reading.open_manage_menu(&app, target, at.x, at.y);
                }
            },
            if let (Some(expanded), Scope::Folder { id }) = (expanded, &scope) {
                {
                    let id = id.clone();
                    let key = if expanded { "sidebar.collapse" } else { "sidebar.expand" };
                    rsx! {
                        button {
                            class: "disclosure",
                            aria_label: t_with(key, &[("folder", title.as_str())]),
                            "aria-expanded": "{expanded}",
                            onclick: move |e| {
                                // Not a click on the folder: its selection stays.
                                e.stop_propagation();
                                reading.set_collapsed(&id, expanded);
                            },
                            Icon { size: 16, stroke: "2", paths: if expanded { CHEVRON_DOWN } else { CHEVRON_RIGHT } }
                        }
                    }
                }
            }
            if let Some(target) = renaming {
                RenameField { target, title: title.clone() }
            } else {
                span { class: "name", "{title}" }
            }
            if count > 0 { span { class: "count", "{count}" } }
        }
    }
}

/// What the list shows: the view's label for every feed, else the feed's or folder's
/// name. Empty for one gone from the library.
fn list_title(lib: &Library, selection: &Selection) -> String {
    match &selection.scope {
        Scope::All => t(&format!("views.{}.all", view_key(selection.view))),
        Scope::Feed { id } => lib
            .feeds
            .iter()
            .find(|f| &f.id == id)
            .map(|f| f.title.clone())
            .unwrap_or_default(),
        Scope::Folder { id } => lib
            .folders
            .iter()
            .find(|f| &f.id == id)
            .map(|f| f.title.clone())
            .unwrap_or_default(),
    }
}

#[component]
fn ArticleList() -> Element {
    let app = use_context::<Arc<App>>();
    let reading = use_context::<Reading>();
    let unread = i64::try_from(reading.unread_count()).unwrap_or(i64::MAX);
    let selection = (reading.selection)();
    let title = list_title(&(reading.library)(), &selection);
    let unread_label = if unread == 0 {
        t("list.noUnread")
    } else {
        t_count("list.unread", unread)
    };
    // A new list starts at the top: the viewport is keyed on what it lists, so a new
    // selection or account gets a new viewport, with its own scroll state.
    let key = format!(
        "{:?}/{:?}",
        reading.account.read(),
        reading.selection.read()
    );
    let confirming = (reading.confirming)() && unread > 0;
    let class = if (reading.pane)() == Pane::List {
        "list active"
    } else {
        "list"
    };
    let confirm_app = Arc::clone(&app);

    rsx! {
        section { class, aria_label: t("list.label"),
            div { class: "toolbar",
                div { class: "list-title",
                    h1 { "{title}" }
                    p { "{unread_label}" }
                }
                div { class: "mark-all",
                    // The window closes the question on any click that reaches it.
                    onclick: move |e| e.stop_propagation(),
                    button {
                        class: "icon-button",
                        aria_label: t("list.markAll"),
                        title: t("list.markAll"),
                        "aria-haspopup": "dialog",
                        "aria-expanded": "{confirming}",
                        disabled: unread == 0,
                        onclick: move |_| reading.request_mark_all(&app),
                        svg { width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                            circle { cx: "12", cy: "12", r: "10" }
                            path { d: "m9 12 2 2 4-4" }
                        }
                    }
                    if confirming {
                        div { class: "confirm popover", role: "dialog", "aria-labelledby": "mark-all-question",
                            p { id: "mark-all-question", {t_count("list.markAllQuestion", unread)} }
                            div { class: "actions",
                                button { class: "button", onclick: move |_| reading.close_confirm(), {t("common.cancel")} }
                                button { class: "button primary", onclick: move |_| reading.mark_all_read(&confirm_app), {t("list.markAsRead")} }
                            }
                        }
                    }
                }
            }
            for key in std::iter::once(key) {
                Viewport { key: "{key}" }
            }
        }
    }
}

/// The scrolling rows. Only the visible ones, and some either side, are in the document.
#[component]
fn Viewport() -> Element {
    let app = use_context::<Arc<App>>();
    let live = use_context::<Live>();
    let dates = (live.dates)();
    let mut reading = use_context::<Reading>();
    let size = live.settings.read().thumbnail_size;
    // Where each row starts: a Large thumbnail makes its row taller.
    let offsets = use_memo(move || {
        let size = live.settings.read().thumbnail_size;
        row_offsets(&reading.articles.read().rows, size)
    });
    let mut top = use_signal(|| 0.0_f64);
    let mut height = use_signal(|| INITIAL_VIEWPORT);
    let mut mounted = use_signal(|| None::<Rc<MountedData>>);

    // Brings a newly opened article's row into view, as when stepping with the keyboard.
    // Only a change of article does this: a reload must not pull the list back.
    use_effect(move || {
        let Some(id) = reading.article.read().as_ref().map(|a| a.id.clone()) else {
            return;
        };
        let Some(el) = mounted.peek().clone() else {
            return;
        };
        spawn(async move {
            // The height scroll events report goes stale when the window resizes, and
            // there is none before the first scroll: measure it.
            let view_height = match el.get_client_rect().await {
                Ok(rect) => rect.height(),
                Err(_) => *height.peek(),
            };
            if (*height.peek() - view_height).abs() >= 1.0 {
                height.set(view_height);
            }
            // Found now, not when the effect ran: a reload meanwhile may have moved the
            // row, or taken it away.
            let place = {
                let (list, offsets) = (reading.articles.peek(), offsets.peek());
                list.rows
                    .iter()
                    .position(|a| a.id == id)
                    .and_then(|i| Some((*offsets.get(i)?, *offsets.get(i + 1)?)))
            };
            let Some((row_top, row_bottom)) = place else {
                return;
            };
            let view_top = *top.peek();
            let target = if row_top < view_top {
                row_top
            } else if row_bottom > view_top + view_height {
                row_bottom - view_height
            } else {
                return;
            };
            top.set(target);
            let to = dioxus_html::geometry::PixelsVector2D::new(0.0, target);
            if let Err(e) = el.scroll(to, ScrollBehavior::Instant).await {
                tracing::warn!("scrolling the list failed: {e}");
            }
        });
    });

    let listed = reading.articles.read();
    let list = &listed.rows;
    let offsets = offsets.read();
    let total = offsets[list.len()];
    // A list made shorter, as by a smaller thumbnail size, may still have the old scroll
    // position until a scroll event says where the viewport clamped it.
    let view_top = top().min((total - height()).max(0.0));
    let (first, last) = visible_rows(&offsets, view_top, height());
    let account = reading.account.read().clone().unwrap_or_default();
    let (beside, below, fit) = match size {
        ThumbnailSize::Small => (Some("thumb small"), false, thumbs::Fit::Square),
        ThumbnailSize::Medium => (Some("thumb medium"), false, thumbs::Fit::Square),
        ThumbnailSize::Large => (None, true, thumbs::Fit::Wide),
        ThumbnailSize::None => (None, false, thumbs::Fit::Square),
    };
    let selected = reading.article.read().as_ref().map(|a| a.id.clone());
    let empty = t(&format!(
        "views.{}.empty",
        view_key(reading.selection.read().view)
    ));
    let lib = reading.library.read();
    let feed_title = |id: &str| {
        lib.feeds
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.title.clone())
            .unwrap_or_default()
    };

    rsx! {
        div {
            class: "viewport",
            onmounted: move |e: MountedEvent| mounted.set(Some(e.data())),
            onscroll: move |e: Event<ScrollData>| {
                // Setting a signal re-renders even to the same value; a scroll event
                // that follows every layout would loop.
                let (t, h) = (e.data().scroll_top(), f64::from(e.data().client_height()));
                if (*top.peek() - t).abs() >= 1.0 {
                    top.set(t);
                }
                if (*height.peek() - h).abs() >= 1.0 {
                    height.set(h);
                }
            },
            if list.is_empty() {
                p { class: "empty", "{empty}" }
            }
            div { class: "spacer", style: "height: {total}px",
                for (i , a) in list.iter().enumerate().take(last).skip(first) {
                    {
                        let summary = a.clone();
                        let app = Arc::clone(&app);
                        let mut class = String::from("row");
                        if !reading.is_read(a) {
                            class.push_str(" unread");
                        }
                        if selected.as_deref() == Some(a.id.as_str()) {
                            class.push_str(" selected");
                        }
                        let (row_top, row_height) = (offsets[i], offsets[i + 1] - offsets[i]);
                        let thumb = a.thumbnail.then(|| thumbs::url(&account, &a.id, fit));
                        rsx! {
                            div {
                                key: "{a.id}",
                                class,
                                style: "top: {row_top}px; height: {row_height}px",
                                onclick: move |_| {
                                    reading.pane.set(Pane::List);
                                    reading.open(&app, summary.clone());
                                },
                                if let (Some(class), Some(src)) = (beside, thumb.clone()) {
                                    img { class, src, alt: "" }
                                }
                                div { class: "text",
                                    div { class: "meta",
                                        span { class: "feed", {feed_title(&a.feed_id)} }
                                        if reading.is_starred(a) {
                                            svg { class: "star", width: "11", height: "11", view_box: "0 0 24 24", fill: "currentColor",
                                                path { d: STAR_PATH }
                                            }
                                        }
                                        span { {dates.list(a.published)} }
                                    }
                                    div { class: "title", "{a.title}" }
                                    if let Some(src) = thumb.filter(|_| below) {
                                        img { class: "thumb large", src, alt: "" }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Lucide's star.
const STAR_PATH: &str = "M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a2.123 2.123 0 0 0 1.595 1.16l5.166.756a.53.53 0 0 1 .294.904l-3.736 3.638a2.123 2.123 0 0 0-.611 1.878l.882 5.14a.53.53 0 0 1-.771.56l-4.618-2.428a2.122 2.122 0 0 0-1.973 0L6.396 21.01a.53.53 0 0 1-.77-.56l.881-5.139a2.122 2.122 0 0 0-.611-1.879L2.16 9.795a.53.53 0 0 1 .294-.906l5.165-.755a2.122 2.122 0 0 0 1.597-1.16z";

#[component]
fn Reader() -> Element {
    let app = use_context::<Arc<App>>();
    let mut reading = use_context::<Reading>();
    let live = use_context::<Live>();
    // Only what the document shows remakes it: remade, the frame loads again, back at
    // the top.
    let look = use_memo(move || article::Look::from(&*live.settings.read()));
    // The Reader View text to show, apart: its loading states don't remake the document.
    let full_text = use_memo(move || {
        let rv = reading.reader_view.read();
        let key = Some(((reading.account)()?, (reading.article)()?.id));
        rv.html.clone().filter(|_| rv.on && rv.article == key)
    });
    let doc = use_memo({
        let app = Arc::clone(&app);
        move || {
            let a = (reading.article)()?;
            let account = (reading.account)()?;
            let html = match full_text() {
                Some(html) => html,
                None => app.article_html(&account, &a.id).ok().flatten()?,
            };
            let lib = reading.library.peek();
            let feed = lib.feeds.iter().find(|f| f.id == a.feed_id);
            let date = live.dates.read().full(a.published);
            Some(article::document(&a, feed, &html, &date, look()))
        }
    });
    let open = reading.open_article();
    // Reader View on its way, just after opening: no feed content to swap out yet.
    let holding = {
        let rv = reading.reader_view.read();
        rv.on && rv.fetch == (Fetch::Loading { awaiting: true })
    };

    rsx! {
        section { class: "reader", aria_label: t("reader.label"),
            div { class: "toolbar",
                if let Some(a) = open {
                    ReaderButtons { article: a }
                }
            }
            match doc() {
                Some(_) if holding => rsx! {},
                Some(doc) => rsx! {
                    iframe {
                        // Remade for new content, which then loads from the top.
                        key: "{full_text.read().is_some()}-{doc.len()}",
                        title: "",
                        "srcdoc": doc,
                        onmounted: move |e: MountedEvent| reading.frame.set(Some(e.data())),
                    }
                },
                None => rsx! { p { class: "empty", {t("reader.empty")} } },
            }
        }
    }
}

/// The open article's read, star and browser buttons.
#[component]
fn ReaderButtons(article: ArticleSummary) -> Element {
    let app = use_context::<Arc<App>>();
    let reading = use_context::<Reading>();
    let read = reading.is_read(&article);
    let starred = reading.is_starred(&article);
    let read_label = if read {
        t("reader.markUnread")
    } else {
        t("reader.markRead")
    };
    let star_label = if starred {
        t("reader.unstar")
    } else {
        t("reader.addStar")
    };
    let star_app = Arc::clone(&app);
    let reader_app = Arc::clone(&app);
    let (rv_on, rv_loading, rv_failed) = {
        let rv = reading.reader_view.read();
        (
            rv.on,
            rv.on && matches!(rv.fetch, Fetch::Loading { .. }),
            rv.fetch == Fetch::Failed,
        )
    };
    let rv_class = match (rv_on, rv_loading) {
        (_, true) => "icon-button reader-view loading",
        (true, false) => "icon-button reader-view",
        (false, false) => "icon-button",
    };
    rsx! {
        div { class: "spacer",
            // Announced, and shown until the next try or article.
            span { class: "notice", role: "status",
                if rv_failed { {t("reader.unavailable")} }
            }
        }
        button {
            class: if read { "icon-button" } else { "icon-button unread" },
            aria_label: read_label.clone(),
            title: read_label,
            onclick: move |_| reading.toggle_read(&app),
            // The unread mark: filled while unread, outlined once read.
            svg { width: "18", height: "18", view_box: "0 0 24 24", fill: if read { "none" } else { "currentColor" }, stroke: "currentColor", stroke_width: "1.6",
                circle { cx: "12", cy: "12", r: "7.5" }
            }
        }
        button {
            class: if starred { "icon-button starred" } else { "icon-button" },
            aria_label: star_label.clone(),
            title: star_label,
            onclick: move |_| reading.toggle_starred(&star_app),
            svg { width: "18", height: "18", view_box: "0 0 24 24", fill: if starred { "currentColor" } else { "none" }, stroke: "currentColor", stroke_width: "2", stroke_linejoin: "round",
                path { d: STAR_PATH }
            }
        }
        button {
            class: rv_class,
            aria_label: t("reader.readerView"),
            title: t("reader.readerView"),
            "aria-pressed": "{rv_on}",
            "aria-busy": "{rv_loading}",
            disabled: article.url.is_empty(),
            onclick: move |_| reading.toggle_reader_view(&reader_app),
            // Lucide's square-text.
            svg { width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                rect { x: "3", y: "3", width: "18", height: "18", rx: "2" }
                path { d: "M7 8h8" }
                path { d: "M7 12h10" }
                path { d: "M7 16h6" }
            }
        }
        button {
            class: "icon-button",
            aria_label: t("reader.openInBrowser"),
            title: t("reader.openInBrowser"),
            disabled: article.url.is_empty(),
            onclick: move |_| reading.open_in_browser(),
            svg { width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                circle { cx: "12", cy: "12", r: "10" }
                path { d: "M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20" }
                path { d: "M2 12h20" }
            }
        }
    }
}

/// Where each of `rows` starts, in CSS pixels, and then where the last one ends.
fn row_offsets(rows: &[ArticleSummary], size: ThumbnailSize) -> Vec<f64> {
    let mut offsets = Vec::with_capacity(rows.len() + 1);
    let mut at = 0.0;
    offsets.push(at);
    for a in rows {
        at += if size == ThumbnailSize::Large && a.thumbnail {
            ROW_HEIGHT + LARGE_THUMB_EXTRA
        } else {
            ROW_HEIGHT
        };
        offsets.push(at);
    }
    offsets
}

/// The rows to render, `first..last`, for a viewport scrolled to `top` that is `height`
/// tall, with `OVERSCAN` rows either side. `offsets` is [`row_offsets`]'s.
fn visible_rows(offsets: &[f64], top: f64, height: f64) -> (usize, usize) {
    let len = offsets.len().saturating_sub(1);
    // The row that contains `y`: the last that starts at or above it.
    let row_at = |y: f64| offsets.partition_point(|&o| o <= y).saturating_sub(1);
    let first = row_at(top).saturating_sub(OVERSCAN).min(len);
    let last = (row_at(top + height) + 1 + OVERSCAN).min(len);
    (first.min(last), last)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn articles(thumbnails: &[bool]) -> Vec<ArticleSummary> {
        thumbnails
            .iter()
            .enumerate()
            .map(|(i, &thumbnail)| ArticleSummary {
                id: i.to_string(),
                feed_id: String::new(),
                title: String::new(),
                author: String::new(),
                url: String::new(),
                published: 0,
                read: false,
                starred: false,
                thumbnail,
            })
            .collect()
    }

    #[test]
    fn only_the_visible_rows_and_some_either_side_render() {
        let offsets = row_offsets(&articles(&[false; 5_000]), ThumbnailSize::Large);
        assert_eq!(visible_rows(&offsets, 0.0, 870.0), (0, 19));
        assert_eq!(visible_rows(&offsets, 87_000.0, 870.0), (992, 1019));
        assert_eq!(visible_rows(&offsets, 434_913.0, 870.0), (4_991, 5_000));
        let three = row_offsets(&articles(&[false; 3]), ThumbnailSize::Medium);
        assert_eq!(visible_rows(&three, 0.0, 870.0), (0, 3));
        assert_eq!(visible_rows(&[0.0], 0.0, 870.0), (0, 0));
    }

    #[test]
    fn only_a_large_thumbnail_makes_its_row_taller() {
        let list = articles(&[true, false, true]);
        let tall = ROW_HEIGHT + LARGE_THUMB_EXTRA;
        assert_eq!(
            row_offsets(&list, ThumbnailSize::Large),
            [0.0, tall, tall + ROW_HEIGHT, 2.0 * tall + ROW_HEIGHT]
        );
        assert_eq!(
            row_offsets(&list, ThumbnailSize::Medium),
            [0.0, ROW_HEIGHT, 2.0 * ROW_HEIGHT, 3.0 * ROW_HEIGHT]
        );
        // Past the end, the last rows render.
        let offsets = row_offsets(&list, ThumbnailSize::Large);
        assert_eq!(visible_rows(&offsets, 10_000.0, 500.0), (0, 3));
    }

    #[test]
    fn account_shortcuts_go_by_the_digit_key_with_the_platform_modifier() {
        let (mods, other) = if cfg!(target_os = "macos") {
            (Modifiers::META, Modifiers::ALT)
        } else {
            (Modifiers::ALT, Modifiers::META)
        };
        assert_eq!(account_index("Digit1", mods), Some(0));
        assert_eq!(account_index("Digit9", mods), Some(8));
        assert_eq!(account_index("Digit0", mods), None);
        assert_eq!(account_index("KeyA", mods), None);
        assert_eq!(account_index("Digit1", other), None);
        assert_eq!(account_index("Digit1", mods | Modifiers::SHIFT), None);
    }

    fn feed(id: &str, folder: Option<&str>) -> skvaller_core::Feed {
        skvaller_core::Feed {
            id: id.into(),
            title: id.into(),
            folder_id: folder.map(Into::into),
            subscribed: true,
            icon: None,
            reader_view: false,
            url: None,
            site_url: None,
            fetch_error: None,
        }
    }

    fn folder(id: &str) -> skvaller_core::Folder {
        skvaller_core::Folder {
            id: id.into(),
            title: id.to_uppercase(),
        }
    }

    fn feed_scope(id: &str) -> Scope {
        Scope::Feed { id: id.into() }
    }

    fn folder_scope(id: &str) -> Scope {
        Scope::Folder { id: id.into() }
    }

    #[test]
    fn the_sidebar_steps_all_then_folders_with_their_open_feeds_then_loose_feeds() {
        let lib = Library {
            folders: vec![folder("d")],
            feeds: vec![feed("loose", None), feed("inner", Some("d"))],
            ..Default::default()
        };
        let tree = sidebar_tree(&lib, View::All, &HashSet::new(), &Scope::All);
        assert_eq!(
            sidebar_scopes(&tree, &HashSet::new()),
            [
                Scope::All,
                folder_scope("d"),
                feed_scope("inner"),
                feed_scope("loose")
            ]
        );
        // A collapsed folder's feeds have no rows.
        assert_eq!(
            sidebar_scopes(&tree, &HashSet::from(["d".to_owned()])),
            [Scope::All, folder_scope("d"), feed_scope("loose")]
        );
    }

    #[test]
    fn unread_and_starred_show_only_pinned_feeds_their_folders_and_the_selection() {
        let lib = Library {
            folders: vec![folder("d"), folder("e"), folder("f")],
            feeds: vec![
                feed("a", Some("d")),
                feed("b", Some("d")),
                feed("c", Some("e")),
                feed("x", None),
                feed("y", None),
            ],
            ..Default::default()
        };
        let pinned = HashSet::from(["b".to_owned(), "y".to_owned()]);
        let scopes = |view, selected: &Scope| {
            sidebar_scopes(
                &sidebar_tree(&lib, view, &pinned, selected),
                &HashSet::new(),
            )
        };
        assert_eq!(
            scopes(View::Unread, &Scope::All),
            [
                Scope::All,
                folder_scope("d"),
                feed_scope("b"),
                feed_scope("y")
            ]
        );
        assert_eq!(
            scopes(View::Starred, &Scope::All),
            scopes(View::Unread, &Scope::All)
        );
        // The selected feed shows, and so its folder; a selected folder shows empty.
        assert_eq!(
            scopes(View::Unread, &feed_scope("c")),
            [
                Scope::All,
                folder_scope("d"),
                feed_scope("b"),
                folder_scope("e"),
                feed_scope("c"),
                feed_scope("y"),
            ]
        );
        assert_eq!(
            scopes(View::Unread, &folder_scope("f")),
            [
                Scope::All,
                folder_scope("d"),
                feed_scope("b"),
                folder_scope("f"),
                feed_scope("y")
            ]
        );
        assert_eq!(scopes(View::All, &Scope::All).len(), 9);
    }
}
