//! The main window: sidebar, article list and reader.
//!
//! Everything shown is read from the local caches through [`App`]; nothing here waits
//! for the network. A sync that changes the cache bumps a revision, which reloads the
//! sidebar and the list. A change made here shows at once and is saved to the cache;
//! the next sync sends it.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use dioxus_native::prelude::*;
use skvaller_app::{App, Library, MarkReadOn, SyncStatus};
use skvaller_core::{ArticleSummary, Scope, Selection, View};
use tokio::sync::broadcast::error::RecvError;

use crate::article;
use crate::i18n::{t, t_count};
use crate::shortcuts::{ARROWS, Action, bindings, key_name};

const APP_CSS: &str = include_str!("app.css");

/// Height of an article list row, in CSS pixels: the Tauri version's row without
/// thumbnails. Every row has it, so the list can place rows without measuring them.
const ROW_HEIGHT: f64 = 87.0;

/// Rows rendered above and below the visible ones, so a fast scroll doesn't show gaps
/// before the next render.
const OVERSCAN: usize = 8;

/// The list's height before its first scroll event says what it is. Generous: rendering
/// a few rows too many costs little, too few shows a gap.
const INITIAL_VIEWPORT: f64 = 1600.0;

/// How long an article is open before it is marked read, with [`MarkReadOn::Delay`].
const MARK_READ_DELAY: Duration = Duration::from_secs(2);

/// The column the arrow keys move in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Sidebar,
    List,
}

/// What the window shows and what was changed here, shared by its parts.
///
/// Read and starred changes made here are kept on top of the list as loaded, so an
/// article marked read stays in the Unread list until the list loads again.
#[derive(Clone, Copy, PartialEq)]
struct Reading {
    account: Signal<Option<String>>,
    selection: Signal<Selection>,
    /// The open article.
    article: Signal<Option<ArticleSummary>>,
    /// Bumped when the cache changed under the lists: they load again.
    revision: Signal<u64>,
    /// Bumped when a change made here changed the counts.
    counts: Signal<u64>,
    read: Signal<HashMap<String, bool>>,
    starred: Signal<HashMap<String, bool>>,
    library: Memo<Library>,
    articles: Memo<Vec<ArticleSummary>>,
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
}

impl Reading {
    fn is_read(&self, a: &ArticleSummary) -> bool {
        self.read.read().get(&a.id).copied().unwrap_or(a.read)
    }

    fn is_starred(&self, a: &ArticleSummary) -> bool {
        self.starred.read().get(&a.id).copied().unwrap_or(a.starred)
    }

    fn unread_count(&self) -> usize {
        self.articles
            .read()
            .iter()
            .filter(|a| !self.is_read(a))
            .count()
    }

    fn select(mut self, selection: Selection) {
        self.selection.set(selection);
        self.article.set(None);
        self.confirming.set(false);
    }

    /// Opens `a`, marking it read as the settings say.
    fn open(mut self, app: &Arc<App>, a: ArticleSummary) {
        let id = a.id.clone();
        let unread = !self.is_read(&a);
        self.article.set(Some(a));
        self.mark_generation += 1;
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
                    if *self.mark_generation.peek() == generation {
                        self.set_read(&app, &[id], true);
                    }
                });
            }
            MarkReadOn::Manual => {}
        }
    }

    /// Opens the next (1) or previous (-1) article in the list; with none open, the first.
    fn step(self, app: &Arc<App>, delta: isize) {
        let list = self.articles.peek();
        let open = self.article.peek().as_ref().map(|a| a.id.clone());
        let at = open.and_then(|id| list.iter().position(|a| a.id == id));
        let next = match at {
            None => list.first(),
            Some(i) => i.checked_add_signed(delta).and_then(|i| list.get(i)),
        }
        .cloned();
        drop(list);
        if let Some(next) = next {
            self.open(app, next);
        }
    }

    /// Scrolls the article on (1) or back (-1) by most of a page. At its end, reading on
    /// opens the next article; with none open, the first.
    fn read_on(self, app: &Arc<App>, direction: f64) {
        let frame = self.frame.peek().clone();
        let Some(frame) = frame.filter(|_| self.article.peek().is_some()) else {
            if direction > 0.0 {
                self.step(app, 1);
            }
            return;
        };
        let (app, animate) = (Arc::clone(app), app.settings().animations);
        spawn(async move {
            let (Ok(offset), Ok(range), Ok(rect)) = (
                frame.get_scroll_offset().await,
                frame.get_scroll_size().await,
                frame.get_client_rect().await,
            ) else {
                return;
            };
            if direction > 0.0 && offset.y >= range.height - 1.0 {
                self.step(&app, 1);
                return;
            }
            let to = dioxus_html::geometry::PixelsVector2D::new(
                offset.x,
                offset.y + direction * rect.height() * 0.9,
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
        let mut overlay = self.read.write();
        for id in ids {
            overlay.insert(id.clone(), read);
        }
        drop(overlay);
        if let Err(e) = app.set_read(&account, ids, read) {
            tracing::error!("saving a read change failed: {e}");
        }
        self.counts += 1;
    }

    fn toggle_read(self, app: &App) {
        if let Some(a) = self.article.peek().clone() {
            self.set_read(app, std::slice::from_ref(&a.id), !self.is_read(&a));
        }
    }

    fn toggle_starred(mut self, app: &App) {
        let (Some(a), Some(account)) = (self.article.peek().clone(), self.account.peek().clone())
        else {
            return;
        };
        let starred = !self.is_starred(&a);
        self.starred.write().insert(a.id.clone(), starred);
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
        self.confirming.set(false);
        let ids: Vec<String> = self
            .articles
            .peek()
            .iter()
            .filter(|a| !self.is_read(a))
            .map(|a| a.id.clone())
            .collect();
        self.set_read(app, &ids, true);
        if self.selection.peek().view == View::Unread {
            self.revision += 1;
        }
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
        spawn(async move {
            if let Err(e) = app.sync_now(&account).await {
                tracing::error!("syncing failed: {e}");
            }
        });
    }

    /// Moves the sidebar's selection to the next (1) or previous (-1) scope, keeping
    /// the view, as the Tauri version does.
    fn step_sidebar(self, delta: isize) {
        let scopes = sidebar_scopes(&self.library.peek());
        let current = self.selection.peek().clone();
        let next = scopes
            .iter()
            .position(|scope| *scope == current.scope)
            .and_then(|i| i.checked_add_signed(delta))
            .and_then(|i| scopes.get(i));
        if let Some(scope) = next {
            self.select(Selection {
                view: current.view,
                scope: scope.clone(),
            });
        }
    }

    /// Shows the account, keeping the view: its feeds, articles and status.
    fn switch_account(mut self, app: &App, id: &str) {
        self.menu.set(None);
        if self.account.peek().as_deref() == Some(id) {
            return;
        }
        if let Err(e) = app.set_active(id) {
            tracing::error!("switching to account {id} failed: {e}");
            return;
        }
        let view = self.selection.peek().view;
        self.select(Selection {
            view,
            scope: Scope::All,
        });
        self.read.write().clear();
        self.starred.write().clear();
        self.status.set(app.status(id).ok());
        self.account.set(Some(id.to_owned()));
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
        let key = e.key();
        if let Some(index) = account_index(&e.code().to_string(), e.modifiers()) {
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
        if *self.confirming.peek() {
            match key {
                Key::Named(NamedKey::Escape) => self.confirming.set(false),
                Key::Named(NamedKey::Enter) => self.mark_all_read(app),
                _ => {}
            }
            return true;
        }
        let Some(name) = key_name(&key, &e.code().to_string(), e.modifiers()) else {
            return false;
        };
        let repeat = e.is_auto_repeating();
        if ARROWS.contains(&name.as_str()) {
            let pane = *self.pane.peek();
            match name.as_str() {
                "ArrowLeft" => self.pane.set(Pane::Sidebar),
                "ArrowRight" => {
                    self.pane.set(Pane::List);
                    if self.article.peek().is_none() {
                        self.step(app, 1);
                    }
                }
                // Each sidebar step loads a whole list: they don't repeat.
                vertical if pane == Pane::Sidebar && !repeat => {
                    self.step_sidebar(if vertical == "ArrowDown" { 1 } else { -1 });
                }
                vertical if pane == Pane::List => {
                    self.step(app, if vertical == "ArrowDown" { 1 } else { -1 });
                }
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
            Action::Next | Action::Previous => {
                self.pane.set(Pane::List);
                self.step(app, if action == Action::Next { 1 } else { -1 });
            }
            Action::ReadOn => self.read_on(app, 1.0),
            Action::ReadBack => self.read_on(app, -1.0),
            // Reader View comes with a later milestone.
            Action::ToggleReaderView => {}
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
    let modifier = if cfg!(target_os = "macos") {
        Modifiers::META
    } else {
        Modifiers::ALT
    };
    if mods != modifier {
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
    let counts = use_signal(|| 0_u64);
    let mut status = use_signal(|| account.peek().as_deref().and_then(|id| app.status(id).ok()));

    // Sync reports: the status for the sidebar, and a reload when articles changed.
    use_hook({
        let app = Arc::clone(&app);
        move || {
            spawn(async move {
                let mut events = app.subscribe();
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

    let library = use_memo({
        let app = Arc::clone(&app);
        move || {
            revision();
            counts();
            account()
                .and_then(|id| app.library(&id).ok())
                .unwrap_or_default()
        }
    });
    let articles = use_memo({
        let app = Arc::clone(&app);
        move || {
            revision();
            let selection = selection();
            let mut list = account()
                .and_then(|id| app.articles(&id, &selection).ok())
                .unwrap_or_default();
            // The open article stays listed, read or not, until another opens.
            if let Some(open) = article.peek().clone()
                && !list.iter().any(|a| a.id == open.id)
            {
                let at = list.partition_point(|a| a.published >= open.published);
                list.insert(at, open);
            }
            list
        }
    });
    let mut reading = use_context_provider(|| Reading {
        account,
        selection,
        article,
        revision,
        counts,
        read: Signal::new(HashMap::new()),
        starred: Signal::new(HashMap::new()),
        library,
        articles,
        pane: Signal::new(Pane::List),
        confirming: Signal::new(false),
        mark_generation: Signal::new(0),
        frame: Signal::new(None),
        status,
        menu: Signal::new(None),
    });
    let keys = use_hook(|| Rc::new(bindings(&app.settings().shortcuts)));
    let theme = article::theme_name(app.settings().theme);

    rsx! {
        style { {APP_CSS} }
        // Focusable, and focused from the start: a key press goes to the focused element,
        // and a click keeps focus on the nearest focusable ancestor, so every key the
        // window gets passes through here. -1 keeps the root out of the Tab order.
        div {
            class: "root",
            "data-theme": theme,
            tabindex: "-1",
            onmounted: move |e: MountedEvent| async move {
                // Without focus the app hears no keys, but works otherwise.
                if let Err(err) = e.data().set_focus(true).await {
                    tracing::warn!("focusing the window's root failed: {err}");
                }
            },
            onkeydown: move |e: Event<KeyboardData>| {
                if reading.on_key(&app, &keys, &e.data()) {
                    e.prevent_default();
                }
            },
            // A click outside the account menu closes it; the menu's own don't get here.
            onclick: move |_| {
                if reading.menu.peek().is_some() {
                    reading.menu.set(None);
                }
            },
            Sidebar {}
            ArticleList {}
            Reader {}
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

/// The sidebar's scopes from top to bottom, as the arrow keys step through them: all
/// feeds, then each folder with its feeds, then the feeds outside folders.
fn sidebar_scopes(lib: &Library) -> Vec<Scope> {
    let mut scopes = vec![Scope::All];
    for folder in &lib.folders {
        scopes.push(Scope::Folder {
            id: folder.id.clone(),
        });
        for feed in feeds_in(lib, Some(&folder.id)) {
            scopes.push(Scope::Feed {
                id: feed.id.clone(),
            });
        }
    }
    for feed in feeds_in(lib, None) {
        scopes.push(Scope::Feed {
            id: feed.id.clone(),
        });
    }
    scopes
}

#[component]
fn Sidebar() -> Element {
    let app = use_context::<Arc<App>>();
    let mut reading = use_context::<Reading>();
    let lib = (reading.library)();
    let unread_total: i64 = lib.counts.unread_by_feed.values().sum();
    let starred_total: i64 = lib.counts.starred_by_feed.values().sum();
    let current = (reading.selection)();
    let mut select = move |next: Selection| {
        reading.pane.set(Pane::Sidebar);
        reading.select(next);
    };
    let views = [
        (View::Unread, t("views.unread.label"), unread_total),
        (View::Starred, t("views.starred.label"), starred_total),
        (View::All, t("views.all.label"), 0),
    ];
    let counts_for = |view: View, id: &str| match view {
        View::Unread | View::All => lib.counts.unread_by_feed.get(id).copied().unwrap_or(0),
        View::Starred => lib.counts.starred_by_feed.get(id).copied().unwrap_or(0),
    };
    let status_line = match (reading.status)() {
        Some(s) if s.syncing => (t("sidebar.syncing"), false),
        Some(SyncStatus { error: Some(e), .. }) => {
            (format!("{}: {e}", t("sidebar.syncFailed")), true)
        }
        Some(SyncStatus {
            last_sync: Some(ms),
            ..
        }) => (article::format_date(ms), false),
        _ => (t("sidebar.notSynced"), false),
    };
    let class = if (reading.pane)() == Pane::Sidebar {
        "sidebar active"
    } else {
        "sidebar"
    };

    rsx! {
        nav { class, aria_label: t("sidebar.label"),
            div { class: "toolbar",
                if app.accounts().len() > 1 {
                    AccountSwitcher {}
                }
            }
            div { class: "feeds",
                for (view , label , count) in views {
                    div {
                        class: if current.view == view && current.scope == Scope::All { "item selected" } else { "item" },
                        onclick: move |_| select(Selection { view, scope: Scope::All }),
                        span { class: "name", "{label}" }
                        if count > 0 { span { class: "count", "{count}" } }
                    }
                }
                div { class: "heading", {t("sidebar.feeds")} }
                for folder in lib.folders.clone() {
                    {
                        let scope = Scope::Folder { id: folder.id.clone() };
                        let count: i64 = feeds_in(&lib, Some(&folder.id)).iter().map(|f| counts_for(current.view, &f.id)).sum();
                        let feeds: Vec<_> = feeds_in(&lib, Some(&folder.id)).into_iter().cloned().collect();
                        rsx! {
                            div {
                                key: "folder-{folder.id}",
                                class: if current.scope == scope { "item selected" } else { "item" },
                                onclick: move |_| {
                                    let view = reading.selection.peek().view;
                                    select(Selection { view, scope: scope.clone() });
                                },
                                span { class: "name", "{folder.title}" }
                                if count > 0 { span { class: "count", "{count}" } }
                            }
                            for feed in feeds {
                                FeedItem { key: "{feed.id}", count: counts_for(current.view, &feed.id), feed, nested: true }
                            }
                        }
                    }
                }
                for feed in feeds_in(&lib, None).into_iter().cloned() {
                    FeedItem { key: "{feed.id}", count: counts_for(current.view, &feed.id), feed, nested: false }
                }
            }
            div { class: if status_line.1 { "status error" } else { "status" }, "{status_line.0}" }
        }
    }
}

/// The account menu's button, and the menu while open: each account with its unread
/// count and shortcut, the shown one checked.
#[component]
fn AccountSwitcher() -> Element {
    let app = use_context::<Arc<App>>();
    let mut reading = use_context::<Reading>();
    let menu = (reading.menu)();
    let active = (reading.account)();
    // Changes made here show in the shown account's count at once.
    (reading.counts)();
    let accounts = app.accounts();
    let menu_app = Arc::clone(&app);
    let shortcut_key = if cfg!(target_os = "macos") {
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
                div { class: "menu", role: "menu", aria_label: t("accounts.menu"),
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
                }
            }
        }
    }
}

#[component]
fn FeedItem(feed: skvaller_core::Feed, count: i64, nested: bool) -> Element {
    let mut reading = use_context::<Reading>();
    let scope = Scope::Feed {
        id: feed.id.clone(),
    };
    let selected = reading.selection.read().scope == scope;
    let class = match (selected, nested) {
        (true, true) => "item nested selected",
        (true, false) => "item selected",
        (false, true) => "item nested",
        (false, false) => "item",
    };
    rsx! {
        div {
            class,
            onclick: move |_| {
                reading.pane.set(Pane::Sidebar);
                let view = reading.selection.peek().view;
                reading.select(Selection { view, scope: scope.clone() });
            },
            span { class: "name", "{feed.title}" }
            if count > 0 { span { class: "count", "{count}" } }
        }
    }
}

#[component]
fn ArticleList() -> Element {
    let app = use_context::<Arc<App>>();
    let mut reading = use_context::<Reading>();
    let unread = reading.unread_count();
    let heading = if unread == 0 {
        t("list.noUnread")
    } else {
        t_count("list.unread", i64::try_from(unread).unwrap_or(i64::MAX))
    };
    // A new list starts at the top: the viewport is keyed on what it lists, so a new
    // selection gets a new viewport, with its own scroll state.
    let key = format!("{:?}", reading.selection.read());
    let class = if (reading.pane)() == Pane::List {
        "list active"
    } else {
        "list"
    };
    let confirm_app = Arc::clone(&app);

    rsx! {
        section { class, aria_label: t("list.label"),
            div { class: "toolbar",
                h1 { "{heading}" }
                div { class: "mark-all",
                    button {
                        class: "icon-button",
                        aria_label: t("list.markAll"),
                        title: t("list.markAll"),
                        disabled: unread == 0,
                        onclick: move |_| reading.request_mark_all(&app),
                        svg { width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                            circle { cx: "12", cy: "12", r: "10" }
                            path { d: "m9 12 2 2 4-4" }
                        }
                    }
                    if (reading.confirming)() {
                        div { class: "confirm", role: "dialog",
                            p { {t_count("list.markAllQuestion", i64::try_from(unread).unwrap_or(i64::MAX))} }
                            div { class: "actions",
                                button { class: "button", onclick: move |_| reading.confirming.set(false), {t("common.cancel")} }
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
    let mut reading = use_context::<Reading>();
    let mut top = use_signal(|| 0.0_f64);
    let mut height = use_signal(|| INITIAL_VIEWPORT);
    let mut mounted = use_signal(|| None::<Rc<MountedData>>);

    // Brings a newly opened article's row into view, as when stepping with the keyboard.
    // Only a change of article does this: a reload must not pull the list back.
    use_effect(move || {
        let Some(id) = reading.article.read().as_ref().map(|a| a.id.clone()) else {
            return;
        };
        let Some(i) = reading.articles.peek().iter().position(|a| a.id == id) else {
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
            let (row_top, row_bottom) = (row_offset(i), row_offset(i + 1));
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

    let list = reading.articles.read();
    let (first, last) = visible_rows(top(), height(), list.len());
    let total = row_offset(list.len());
    let selected = reading.article.read().as_ref().map(|a| a.id.clone());
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
                p { class: "empty", {t("list.noUnread")} }
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
                        rsx! {
                            div {
                                key: "{a.id}",
                                class,
                                style: "top: {row_offset(i)}px; height: {ROW_HEIGHT}px",
                                onclick: move |_| {
                                    reading.pane.set(Pane::List);
                                    reading.open(&app, summary.clone());
                                },
                                div { class: "meta",
                                    span { class: "feed", {feed_title(&a.feed_id)} }
                                    if reading.is_starred(a) {
                                        svg { class: "star", width: "11", height: "11", view_box: "0 0 24 24", fill: "currentColor",
                                            path { d: STAR_PATH }
                                        }
                                    }
                                    span { {short_date(a.published)} }
                                }
                                div { class: "title", "{a.title}" }
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
    let theme = app.settings().theme;
    let doc = use_memo({
        let app = Arc::clone(&app);
        move || {
            let a = (reading.article)()?;
            let html =
                (reading.account)().and_then(|id| app.article_html(&id, &a.id).ok().flatten())?;
            let lib = reading.library.peek();
            let feed = lib.feeds.iter().find(|f| f.id == a.feed_id);
            Some(article::document(&a, feed, &html, theme))
        }
    });
    let open = (reading.article)();

    rsx! {
        section { class: "reader", aria_label: t("reader.label"),
            div { class: "toolbar",
                if let Some(a) = open {
                    ReaderButtons { article: a }
                }
            }
            match doc() {
                Some(doc) => rsx! {
                    iframe {
                        key: "{doc.len()}",
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
    let star_app = Arc::clone(&app);
    rsx! {
        div { class: "spacer" }
        button {
            class: if read { "icon-button" } else { "icon-button unread" },
            aria_label: if read { t("reader.markUnread") } else { t("reader.markRead") },
            title: if read { t("reader.markUnread") } else { t("reader.markRead") },
            onclick: move |_| reading.toggle_read(&app),
            // The unread mark: filled while unread, outlined once read.
            svg { width: "18", height: "18", view_box: "0 0 24 24", fill: if read { "none" } else { "currentColor" }, stroke: "currentColor", stroke_width: "1.6",
                circle { cx: "12", cy: "12", r: "7.5" }
            }
        }
        button {
            class: if starred { "icon-button starred" } else { "icon-button" },
            aria_label: if starred { t("reader.unstar") } else { t("reader.addStar") },
            title: if starred { t("reader.unstar") } else { t("reader.addStar") },
            "aria-pressed": "{starred}",
            onclick: move |_| reading.toggle_starred(&star_app),
            svg { width: "18", height: "18", view_box: "0 0 24 24", fill: if starred { "currentColor" } else { "none" }, stroke: "currentColor", stroke_width: "2", stroke_linejoin: "round",
                path { d: STAR_PATH }
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

/// The rows to render, `first..last`, for a viewport scrolled to `top` that is `height`
/// tall, with `OVERSCAN` rows either side.
fn visible_rows(top: f64, height: f64, len: usize) -> (usize, usize) {
    let rows = |px: f64| {
        let n = (px.max(0.0) / ROW_HEIGHT).floor();
        // Bounded by the list length below; a scroll position is never near usize::MAX.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "non-negative and small"
        )]
        let n = n as usize;
        n
    };
    let first = rows(top).saturating_sub(OVERSCAN);
    let last = (rows(top + height) + 1 + OVERSCAN).min(len);
    (first.min(last), last)
}

/// Where row `i` starts, in CSS pixels.
fn row_offset(i: usize) -> f64 {
    // Exact for any list a person scrolls: f64 holds integers up to 2^53.
    #[expect(clippy::cast_precision_loss, reason = "row counts are far below 2^53")]
    let i = i as f64;
    i * ROW_HEIGHT
}

/// The date part of [`article::format_date`], for the list.
fn short_date(ms: i64) -> String {
    let full = article::format_date(ms);
    full.split(',').next().unwrap_or_default().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_visible_rows_and_some_either_side_render() {
        assert_eq!(visible_rows(0.0, 870.0, 5_000), (0, 19));
        let (first, last) = visible_rows(87_000.0, 870.0, 5_000);
        assert_eq!((first, last), (992, 1019));
        assert_eq!(visible_rows(434_913.0, 870.0, 5_000), (4_991, 5_000));
        assert_eq!(visible_rows(0.0, 870.0, 3), (0, 3));
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

    #[test]
    fn the_sidebar_steps_all_then_folders_with_their_feeds_then_loose_feeds() {
        let feed = |id: &str, folder: Option<&str>| skvaller_core::Feed {
            id: id.into(),
            title: id.into(),
            folder_id: folder.map(Into::into),
            subscribed: true,
            icon: None,
            reader_view: false,
            url: None,
            site_url: None,
            fetch_error: None,
        };
        let lib = Library {
            folders: vec![skvaller_core::Folder {
                id: "d".into(),
                title: "D".into(),
            }],
            feeds: vec![feed("loose", None), feed("inner", Some("d"))],
            ..Default::default()
        };
        assert_eq!(
            sidebar_scopes(&lib),
            [
                Scope::All,
                Scope::Folder { id: "d".into() },
                Scope::Feed { id: "inner".into() },
                Scope::Feed { id: "loose".into() },
            ]
        );
    }
}
