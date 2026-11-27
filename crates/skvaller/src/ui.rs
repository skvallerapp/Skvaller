//! The main window: sidebar, article list and reader.
//!
//! Everything shown is read from the local caches through [`App`]; nothing here waits
//! for the network. Every change to the shown account's cache, a sync's or one made
//! here, comes from [`App::changes`] and reloads the sidebar and the list as [`Cached`]
//! says. A change made here shows at once and is saved to the cache; the next sync
//! sends it.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dioxus_html::geometry::WheelDelta;
use dioxus_html::input_data::MouseButton;
use dioxus_native::prelude::*;
use dioxus_native::{
    BlitzScrollSource, BlitzWheelPhase, NativeWheelData, use_window, use_window_event, use_windows,
};
use skvaller_app::{
    App, Changed, Error as AppError, FontSize, LIST_WIDTH, Layout, Library, MarkReadOn, Notice,
    PaneWidth, SIDEBAR_WIDTH, SortSubscriptions, SyncStatus, ThumbnailSize,
};
use skvaller_core::{ArticleSummary, Counts, Scope, Selection, View};
use tokio::sync::broadcast::error::RecvError;

use crate::add_account::AddAccount;
use crate::article;
use crate::chrome::{
    self, DRAWS_FRAME, DragArea, FRAMELESS, Frame, LINUX, MAC, WINDOWS, WindowButtons,
};
use crate::fonts;
use crate::i18n::{t, t_count, t_with};
use crate::icons::{
    CHECK, CHEVRON_RIGHT, Icon, LAYERS, PANEL_LEFT, ROTATE_CW, SEARCH, SETTINGS, X,
};
use crate::lines::Lines;
use crate::live::{Live, use_live};
use crate::overscroll::{Edges, HOLD, Out, Overscroll, SPRING_EASE, Shown, WHEEL_IDLE};
use crate::place::{Keeper, ListView, Place, differs, reveal, reveal_row};
use crate::scale;
use crate::settings::SettingsWindow;
use crate::shortcuts::{ACCOUNT_MODIFIER, Action, COMMAND_MODIFIER, chord_label, field_key};
use crate::spin::Spin;
use crate::thumbs;

mod command;
mod list;
mod listed;
mod manage;
mod pull;
mod reader;
mod sidebar;
mod tree;

pub(crate) use command::Command;
use command::{Overlay, leave};
use listed::{Listed, Load, Marks, Relist};

use list::{ArticleList, list_key};
use manage::{AddFeed, ConfirmRemove, Manage, ManageMenu, RenameField, Target, is_local};
use pull::{ListPull, Pull, Scrolled, spring_style, stretch_style};
use reader::{Reader, STAR_PATH};
use sidebar::{AccountSwitcher, Resizer, SidebarSlot, use_closing, view_key};
use tree::{
    Cached, Entry, Leaving, Pinned, counts_by_feed, next_with_articles, row_scope, sidebar_scopes,
    sidebar_tree, step_scope,
};

pub(crate) const APP_CSS: &str = include_str!("app.css");

/// The window's background for `theme`: html's, which Blitz paints across the window.
///
/// Blitz rounds the root's size to whole CSS pixels, so at some sizes and scales (250%)
/// the last device pixel at the window's right or bottom edge is past the root, and
/// would be white. The theme's colors live on the root, where html can't read them, and
/// Blitz's CSS has no `:has()` to select by the root's theme, so each window adds this
/// beside app.css. The colors are app.css's `--reader-bg`, the right-hand pane's, and
/// must stay in step. Empty on Linux while `framed`: the window is transparent there,
/// so the root's rounded corners show what's behind it.
pub(crate) fn canvas_css(theme: skvaller_app::Theme, framed: bool) -> &'static str {
    use skvaller_app::Theme;
    if chrome::LINUX && framed {
        return "";
    }
    match theme {
        Theme::Light => "html { background: #fafaf9 }",
        Theme::Dark => "html { background: #202022 }",
        Theme::Espresso => "html { background: #2d211e }",
        Theme::System => {
            "html { background: #fafaf9 } \
             @media (prefers-color-scheme: dark) { html { background: #202022 } }"
        }
    }
}

// An article list row's parts, in CSS px, as app.css has them. Rows are placed from
// these heights and the headline's lines (`lines::Lines`), never by measuring them.

/// A row's padding above and below (`.row`).
const PAD_Y: f64 = 11.0;
/// A row's padding at either side (`.row`).
const PAD_X: f64 = 20.0;
/// The meta line, at a line height of 1.3, and the gap below it (`.title`), at
/// `font_size`.
fn meta(font_size: FontSize) -> f64 {
    scale::meta_px(font_size) * 1.3 + 3.0
}
/// A headline line: its font size at a line height of 1.3 (`.title`), at `font_size`.
fn title_line(font_size: FontSize) -> f64 {
    scale::headline_px(font_size) * 1.3
}
/// The most lines a headline shows: rows count it to at most this many, and its `.title`
/// is clamped to the count.
pub(crate) const TITLE_LINES: u32 = 6;
/// The space between a thumbnail beside the text and the text (`.row`'s `gap`).
const GAP: f64 = 12.0;
/// The margin above a thumbnail beside the text (`.thumb.small`, `.thumb.medium`).
const THUMB_MARGIN: f64 = 2.0;
/// What a pane's border takes of the window: 9 px, less 4 px of margin a side
/// (`.resizer`).
const RESIZER: f64 = 1.0;
/// The reader's least width (`.linux .reader`'s and `.windows .reader`'s `min-width`):
/// where the app draws the window buttons, room for them at the end of its toolbar,
/// after its padding: on Linux the wider GNOME ones; on Windows three 46px buttons
/// flush with the window's edge. The list gives up width before the reader does.
const READER_LEAST: f64 = if LINUX {
    116.0
} else if WINDOWS {
    156.0
} else {
    0.0
};
/// The reader's width that holds an open article's buttons too, where the app draws
/// the window buttons: [`READER_LEAST`] and 132 px more, for their four, 28 px each,
/// and the toolbar's five 4 px gaps. A narrower reader leaves them out, as they would
/// run under the window buttons: Blitz clips what overflows but still lets it take
/// presses.
const READER_WITH_BUTTONS: f64 = READER_LEAST + 132.0;
/// The main window's least width: where the app draws the window buttons, the widest
/// sidebar, the narrowest list and the reader's least, so they always show; never
/// under 640 px.
pub const MIN_WINDOW_WIDTH: f64 =
    (SIDEBAR_WIDTH.max + RESIZER + LIST_WIDTH.min + RESIZER + READER_LEAST).max(640.0);
/// The space above a Large thumbnail (`.text.below .title`).
const LARGE_GAP: f64 = 8.0;

/// The list's toolbar above its viewport: app.css's `--toolbar-height`, its border
/// inside (`border-box`). The list's pane is as tall as the window, so the viewport is
/// the window's height less this: taken from the CSS, never by measuring, as a resize
/// brings no scroll event to measure by and Blitz may answer before its layout.
const TOOLBAR: f64 = if MAC { 52.0 } else { 44.0 };

/// How long an article is open before it is marked read, with [`MarkReadOn::Delay`].
const MARK_READ_DELAY: Duration = Duration::from_secs(2);
/// How long an article of a Reader View feed waits for its text before showing the
/// feed's content. Long enough for the cache, which most pages come from after a
/// sync's prefetch; short enough not to feel stuck.
const READER_VIEW_HOLD: Duration = Duration::from_millis(400);

/// How much of the article's height reading on scrolls: a page, less some lines to keep
/// the place.
const READ_ON_PAGE: f64 = 0.9;

/// How long the sidebar takes to slide open or closed: app.css's `.sidebar-slot`
/// transition. Closing, the sidebar stays in the document this long and [`FIRST_FRAME`]
/// more.
const SIDEBAR_SLIDE: Duration = Duration::from_millis(180);

/// How long a folder's feeds take to slide open or closed: app.css's `.folder-feeds`
/// and `.highlight.folding` transitions. Closing, the rows stay in the document this
/// long and [`FIRST_FRAME`] more.
const FOLDER_SLIDE: Duration = Duration::from_millis(160);

/// How long the list takes to slide on to the next one after mark-all-read: app.css's
/// `list-in` and `list-out`.
const LIST_SLIDE: Duration = Duration::from_millis(320);
/// How long the old list's ghost stays: the slide, and its first frame. Ending the slide
/// early would snap the next list the last pixels into place.
const GHOST_STAYS: Duration = LIST_SLIDE.saturating_add(FIRST_FRAME);

/// How long the sidebar rows that mark-all-read emptied take to close and fade:
/// app.css's `leave`, `.folder-feeds.leaving` and `.highlight.leaving`.
const LEAVE: Duration = Duration::from_millis(220);
/// How long the leaving rows stay: [`LEAVE`], and its first frame.
const LEAVING_STAYS: Duration = LEAVE.saturating_add(FIRST_FRAME);

/// How long the frame an animation or transition starts in may take: Blitz starts it as
/// that frame ends, as a browser does, and it may be long, as the first time a view is
/// built or on a slow machine. What stays in the document for a closing animation stays
/// this much longer, by then unseen.
const FIRST_FRAME: Duration = Duration::from_millis(250);

/// A frame, at 60 Hz.
pub(crate) const FRAME: Duration = Duration::from_millis(16);
/// How many frames to wait at most for a layout Blitz hasn't run yet, as for an element
/// just mounted: its size and scroll range are 0 until then.
pub(crate) const LAYOUT_WAITS: u32 = 10;

/// The Feeds heading's top and bottom margins: app.css's `.heading`. The selection's
/// highlight is placed from them, the heading's height and the rows'
/// (`scale::heading_height`, `scale::sidebar_row`), never by measuring: Blitz may
/// answer a measure before its layout.
const SIDEBAR_HEADING_MARGINS: f64 = 14.0 + 4.0;
/// The feeds' top padding: app.css's `.feeds`.
const SIDEBAR_TOP: f64 = 4.0;

/// The `data-motion` of a window's root or the article's document: `off` stops their
/// CSS animations and transitions, as Settings → Animations asks.
pub(crate) fn motion(animations: bool) -> &'static str {
    if animations { "on" } else { "off" }
}

/// How near the end, in CSS pixels, counts as the end: scroll offsets can fall short of
/// it by a rounded fraction.
const END_SLACK: f64 = 1.0;

/// The column the arrow keys move in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pane {
    Sidebar,
    List,
}

/// The search over the article list, while its field is open.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Search {
    open: bool,
    /// As typed.
    query: String,
    /// Search every feed rather than the sidebar's selection.
    everywhere: bool,
}

impl Search {
    /// What the list searches for: the text typed, while the field is open and has any.
    fn active(&self) -> Option<(&str, bool)> {
        let query = self.query.trim();
        (self.open && !query.is_empty()).then_some((query, self.everywhere))
    }
}

/// Focuses `el`, logging why if it fails: nothing more can be done about it.
pub(crate) async fn focus(el: &MountedData, what: &str) {
    if let Err(e) = el.set_focus(true).await {
        tracing::warn!("focusing {what} failed: {e}");
    }
}

/// What a search of `selection` looks through: the view's articles in every feed when
/// `everywhere`, else the selection.
fn searched_within(selection: &Selection, everywhere: bool) -> Selection {
    if everywhere {
        Selection {
            view: selection.view,
            scope: Scope::All,
        }
    } else {
        selection.clone()
    }
}

/// Where a list was when a search replaced it, to scroll back to when the search ends:
/// the row at its top and how far into it, since rows above it may be gone by then,
/// such as those read in Unread.
#[derive(Clone, Debug, PartialEq)]
struct Restore {
    /// The list's [`list_key`].
    list: String,
    /// The rows from the top of the view down, each with how far below the view's top
    /// it started (negative: above). The first still listed places the list.
    rows: Vec<(String, f64)>,
    /// Where it was scrolled, should none be.
    top: f64,
}

impl Restore {
    /// The place of `list` as last `drawn`.
    fn at(list: String, drawn: &Drawn) -> Self {
        let mut rows: Vec<&Row> = drawn
            .rows
            .iter()
            .filter(|r| r.place.top + r.place.height > drawn.top)
            .collect();
        rows.sort_by(|a, b| a.place.top.total_cmp(&b.place.top));
        Self {
            list,
            rows: rows
                .into_iter()
                .map(|r| (r.id.clone(), r.place.top - drawn.top))
                .collect(),
            top: drawn.top,
        }
    }

    /// Where to scroll the list `rows`, placed at `offsets`, to.
    fn target(&self, rows: &[ArticleSummary], offsets: &[f64]) -> f64 {
        self.rows
            .iter()
            .find_map(|(id, below)| {
                let i = rows.iter().position(|a| &a.id == id)?;
                Some(offsets.get(i)? - below)
            })
            .map_or(self.top, |top| top.max(0.0))
    }
}

/// A row of the article list as drawn, which the list's ghost draws again.
#[derive(Clone, Debug)]
struct Row {
    id: String,
    class: String,
    place: Place,
    feed: String,
    starred: bool,
    date: String,
    title: String,
    /// The headline's height as counted, which clamps it: one counted short loses its
    /// last line rather than showing half of it.
    title_height: f64,
    thumb: Option<String>,
}

/// What the article list last drew: the rows in the document and where it was scrolled.
/// After mark-all-read, it slides away as a ghost while the next list slides in.
#[derive(Clone, Debug)]
struct Drawn {
    rows: Vec<Row>,
    top: f64,
    highlight: Option<Place>,
    /// The class of a thumbnail beside the text, or the height of a Large one below it.
    beside: Option<&'static str>,
    below: Option<f64>,
}

/// The old article list sliding away after mark-all-read.
#[derive(Clone, Debug)]
struct Ghost {
    /// The list's [`list_key`], which keys the ghost: one replacing another starts its
    /// slide over.
    list: String,
    drawn: Rc<Drawn>,
}

/// The open article's Reader View.
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
    /// The open article, as it was when opened: [`Reading::open_article`] has its
    /// current flags.
    article: Signal<Option<ArticleSummary>>,
    /// The shown account's cache changes, as [`App::changes`] told them.
    cache: Signal<Cached>,
    marks: Signal<Marks>,
    library: Memo<Library>,
    /// The feeds Unread and Starred keep showing after they are read to zero.
    pinned: Memo<Rc<HashSet<String>>>,
    articles: Memo<Listed>,
    pane: Signal<Pane>,
    /// The popover, question or form open over the window.
    overlay: Signal<Overlay>,
    /// Bumped by every open and read change, so a delayed mark knows it is stale.
    mark_generation: Signal<u64>,
    /// The open article's frame, which "read on" scrolls.
    frame: Signal<Option<Rc<MountedData>>>,
    /// The shown account's sync status.
    status: Signal<Option<SyncStatus>>,
    /// Bumped by each sync asked for here, so the sync icon turns even for one that
    /// ends before its status says it runs.
    syncs_asked: Signal<u64>,
    /// The window's root, which takes focus back when the focused element goes away.
    root: Signal<Option<Rc<MountedData>>>,
    reader_view: Signal<ReaderView>,
    /// The ids of the folders whose feeds the sidebar hides, by account, for this
    /// session.
    collapsed: Signal<HashMap<String, HashSet<String>>>,
    /// The panes' widths and whether the sidebar shows, as saved in config.json.
    layout: Signal<Layout>,
    /// The border being dragged.
    drag: Signal<Option<Drag>>,
    /// The list's width as last saved, which Large thumbnails are served for: a drag
    /// doesn't load new ones at every step.
    saved_list_width: Signal<f64>,
    /// The window's width and height in CSS px: the width may squeeze the list, and
    /// either may change what the list shows.
    window_size: Signal<(f64, f64)>,
    /// The article list's element, measured for the width it shows.
    list_pane: Signal<Option<Rc<MountedData>>>,
    /// What the article list last drew. Kept at every render, so not a signal.
    drawn: CopyValue<Option<Rc<Drawn>>>,
    /// The old list sliding away after mark-all-read, until the slide ends.
    ghost: Signal<Option<Ghost>>,
    /// The sidebar's feeds that mark-all-read emptied, closing until [`LEAVE`] ends.
    leaving: Signal<Option<Rc<Leaving>>>,
    search: Signal<Search>,
    /// The search field, focused again by the search key while it is open.
    search_field: Signal<Option<Rc<MountedData>>>,
    /// The list a search replaced and where it was: it scrolls back there when the
    /// search ends.
    restore: CopyValue<Option<Restore>>,
}

/// A border the user drags to resize the pane before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Sidebar,
    List,
}

impl Edge {
    fn range(self) -> PaneWidth {
        match self {
            Self::Sidebar => SIDEBAR_WIDTH,
            Self::List => LIST_WIDTH,
        }
    }

    fn width(self, layout: &Layout) -> f64 {
        match self {
            Self::Sidebar => layout.sidebar_width,
            Self::List => layout.list_width,
        }
    }

    fn width_mut(self, layout: &mut Layout) -> &mut f64 {
        match self {
            Self::Sidebar => &mut layout.sidebar_width,
            Self::List => &mut layout.list_width,
        }
    }
}

/// Whether a selection highlight moves at once rather than sliding.
///
/// It slides to a new selection, and moves at once when the same selection's row moved,
/// as when a sync adds or removes rows above it. A folder opening or closing above it
/// slides it along with the rows instead (app.css's `.highlight.folding`).
#[derive(Debug)]
struct Settle<K> {
    last: Option<(K, f64)>,
}

impl<K: PartialEq> Settle<K> {
    const fn new() -> Self {
        Self { last: None }
    }

    /// Takes in the selection `key` with its highlight at `top`, and returns whether the
    /// highlight moves there at once (app.css's `.highlight.settling`).
    fn settles(&mut self, key: K, top: f64) -> bool {
        let settles = self
            .last
            .as_ref()
            .is_some_and(|(k, t)| *k == key && differs(*t, top));
        self.last = Some((key, top));
        settles
    }
}

/// The class of a selection highlight, as [`Settle::settles`] has it.
const fn highlight_class(settles: bool) -> &'static str {
    if settles {
        "highlight settling"
    } else {
        "highlight"
    }
}

/// A drag of an [`Edge`]: where the pointer and the pane were at its start.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Drag {
    edge: Edge,
    from_x: f64,
    /// The width the pane showed, which the drag moves from.
    from_width: f64,
    /// The width saved before the drag, which a move to widen never lowers.
    saved_width: f64,
}

impl Drag {
    /// The pane's width with the pointer at `x`. A list the window squeezes moves from
    /// the width it shows, but widening it doesn't lower the width it gets back.
    fn width_at(self, x: f64) -> f64 {
        let moved = x - self.from_x;
        let width = self.from_width + moved;
        if moved > 0.0 {
            width.max(self.saved_width)
        } else {
            width
        }
    }
}

impl Reading {
    /// Whether `a`, a row of the list, is read, with the changes made here.
    fn is_read(&self, a: &ArticleSummary) -> bool {
        self.marks.read().is_read(self.articles.read().load, a)
    }

    fn is_starred(&self, a: &ArticleSummary) -> bool {
        self.marks.read().is_starred(self.articles.read().load, a)
    }

    fn unread_count(&self) -> usize {
        // Borrowed once, not per row: the list may hold thousands.
        let (marks, articles) = (self.marks.read(), self.articles.read());
        articles
            .rows
            .iter()
            .filter(|a| !marks.is_read(articles.load, a))
            .count()
    }

    /// The open article as the list has it now, which a sync may have changed since it
    /// opened. The list has it, or keeps it apart while search results leave it out.
    fn open_article(&self) -> Option<ArticleSummary> {
        let open = self.article.read().clone()?;
        let list = self.articles.read();
        Some(
            list.rows
                .iter()
                .chain(&list.open)
                .find(|a| a.id == open.id)
                .cloned()
                .unwrap_or(open),
        )
    }

    /// Records changes made here on top of the list as loaded now.
    fn mark(mut self, ids: &[String], read: Option<bool>, starred: Option<bool>) {
        let load = self.articles.peek().load;
        self.marks.write().record(load, ids, read, starred);
    }

    /// Shows `selection`'s list, resetting what [`command::Leave`] says.
    fn select(mut self, selection: Selection) {
        // Copied out: a borrow held into the writes would make them panic.
        let left = leave(
            &self.selection.peek(),
            &selection,
            &self.overlay.peek(),
            &self.search.peek(),
        );
        if left.everywhere != self.search.peek().everywhere {
            self.search.write().everywhere = left.everywhere;
        }
        self.set_overlay(left.overlay);
        self.selection.set(selection);
        self.restore.set(None);
        self.article.set(None);
        // Reopening the article doesn't find its Reader View stuck loading.
        self.reset_reader_view(None);
        self.mark_generation += 1;
        // A list sliding away goes at once: the one it slid to is left too.
        if self.ghost.peek().is_some() {
            self.ghost.set(None);
        }
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
        // Not the calling component's task: a field that closes its own overlay, as the
        // rename field does on Enter, goes away in the same render, and its tasks with it.
        dioxus_core::spawn_forever(async move {
            focus(&root, "the window's root").await;
        });
    }

    /// Opens `a`, marking it read as the settings say.
    fn open(mut self, app: &Arc<App>, a: ArticleSummary) {
        let id = a.id.clone();
        let unread = !self.is_read(&a);
        self.follow_reader_view(app, &a);
        self.article.set(Some(a));
        let overlay = self.overlay.peek().after_list_change();
        self.set_overlay(overlay);
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
                // Not the calling component's task: a row's list or the search bar can go
                // away within the delay, as a search is typed or closed.
                dioxus_core::spawn_forever(async move {
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
                .feed(&a.feed_id)
                .is_some_and(|f| f.reader_view);
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
    }

    fn toggle_read(self, app: &App) {
        if let Some(a) = self.open_article() {
            self.set_read(app, std::slice::from_ref(&a.id), !self.is_read(&a));
        }
    }

    fn toggle_starred(self, app: &App) {
        let (Some(a), Some(account)) = (self.open_article(), self.account.peek().clone()) else {
            return;
        };
        let starred = !self.is_starred(&a);
        self.mark(std::slice::from_ref(&a.id), None, Some(starred));
        if let Err(e) = app.set_starred(&account, &[a.id], starred) {
            tracing::error!("saving a star failed: {e}");
        }
    }

    /// Whether mark-all-read moves on to the next row: in Unread, from a feed or
    /// folder, outside a search.
    fn mark_all_moves_on(&self) -> bool {
        let selection = self.selection.read();
        selection.view == View::Unread
            && selection.scope != Scope::All
            && self.search.read().active().is_none()
    }

    /// Marks all as read, asking first when the settings say so. The question doesn't
    /// open while Add Feed is busy, which it can't replace. With nothing unread, it
    /// only moves on, without asking.
    fn request_mark_all(self, app: &App) {
        if self.unread_count() == 0 {
            if self.mark_all_moves_on() {
                self.mark_all_read(app);
            }
            return;
        }
        if app.settings().confirm_mark_all_read {
            self.open_overlay(Overlay::MarkAll);
        } else {
            self.mark_all_read(app);
        }
    }

    /// Marks every article in the list read.
    ///
    /// In Unread, the next row with unread articles is then selected, or All Unread,
    /// and the open article closes. With All Unread already selected, the list empties
    /// at once, except for the article being read.
    fn mark_all_read(mut self, app: &App) {
        if *self.overlay.peek() == Overlay::MarkAll {
            self.overlay.set(Overlay::None);
        }
        // The question's focused button goes, or the one that asked keeps the focus.
        self.refocus();
        // Read, not peek: a recompute under a peek's guard panics. Borrowed once, not
        // per row, as in `unread_count`; both guards go before `mark` writes the marks.
        let ids: Vec<String> = {
            let (marks, articles) = (self.marks.read(), self.articles.read());
            articles
                .rows
                .iter()
                .filter(|a| !marks.is_read(articles.load, a))
                .map(|a| a.id.clone())
                .collect()
        };
        let Some(account) = self.account.peek().clone() else {
            return;
        };
        if !ids.is_empty() {
            self.mark_generation += 1;
            self.mark(&ids, Some(true), None);
            // `App` tells the window, which then loads the list again without the rows
            // read here: in Unread it empties.
            if let Err(e) = app.mark_all_read(&account, &ids) {
                tracing::error!("marking all read failed: {e}");
            }
        }
        if !self.mark_all_moves_on() {
            return;
        }
        let Selection { view, scope } = self.selection.peek().clone();
        let pinned = (self.pinned)();
        let sort = app.settings().sort_subscriptions;
        // From the cache, not `library`: that loads again only once `App`'s change
        // arrives, and until then has the counts from before.
        let lib = app.library(&account).unwrap_or_default();
        let collapsed = self.collapsed_folders();
        let next = next_with_articles(&lib, view, &pinned, &collapsed, &scope, sort);
        // With no next row, as with All selected, the selection stays, and so do the
        // emptied rows: they are pinned until it changes.
        if let Some(next) = next {
            let list = list_key(&self);
            let animate = app.settings().animations;
            // From the library the sidebar shows now, before the select.
            let leaving = animate.then(|| {
                let shown = self.library.peek();
                Leaving::after_mark(&shown, &lib, &account, view, &pinned, &scope, &next, sort)
            });
            self.select(Selection { view, scope: next });
            if animate {
                self.slide_away(list);
            }
            if let Some(leaving) = leaving.filter(|l| !l.feeds.is_empty()) {
                self.leave(leaving);
            }
        }
    }

    /// Closes and fades the sidebar rows of `leaving`'s feeds, then lets them go.
    fn leave(mut self, leaving: Leaving) {
        let leaving = Rc::new(leaving);
        self.leaving.set(Some(Rc::clone(&leaving)));
        let mut current = self.leaving;
        let cache = self.cache;
        let heard = cache.peek().library;
        // Not the calling component's task, which may go away first: the rows would
        // stay.
        dioxus_core::spawn_forever(async move {
            tokio::time::sleep(LEAVING_STAYS).await;
            // Let go before the library loads again without them, they would show again
            // until it does: wait for it, a few frames at most.
            for _ in 0..LAYOUT_WAITS {
                if cache.peek().library != heard {
                    break;
                }
                tokio::time::sleep(FRAME).await;
            }
            // Another mark-all meanwhile already replaced it.
            let ours = current
                .peek()
                .as_ref()
                .is_some_and(|l| Rc::ptr_eq(l, &leaving));
            if ours {
                current.set(None);
            }
        });
    }

    /// Slides the list as last drawn up out of sight, as the next one slides in from
    /// below. `list` is the old list's [`list_key`].
    fn slide_away(mut self, list: String) {
        // Taken, so a list not drawn since never slides away twice.
        let Some(drawn) = self.drawn.write().take() else {
            return;
        };
        self.ghost.set(Some(Ghost {
            list,
            drawn: Rc::clone(&drawn),
        }));
        let mut ghost = self.ghost;
        // Not the calling component's task, which may go away first: the ghost would
        // stay.
        dioxus_core::spawn_forever(async move {
            tokio::time::sleep(GHOST_STAYS).await;
            // A selection or another mark-all meanwhile already replaced it.
            let current = ghost
                .peek()
                .as_ref()
                .is_some_and(|g| Rc::ptr_eq(&g.drawn, &drawn));
            if current {
                ghost.set(None);
            }
        });
    }

    /// Opens the search field, or focuses it if it is open.
    fn open_search(mut self) {
        // An open overlay would take keys meant for the field, such as a paste. The
        // field takes the focus.
        self.open_overlay(Overlay::None);
        if !self.search.peek().open {
            // The field focuses itself as it mounts.
            self.search.write().open = true;
            return;
        }
        let Some(field) = self.search_field.peek().clone() else {
            return;
        };
        spawn(async move {
            focus(&field, "the search field").await;
        });
    }

    /// Searches for `query`, as typed. The first search keeps the list's place, so it
    /// scrolls back there when the search ends.
    fn set_query(mut self, query: String) {
        if self.search.peek().query == query {
            return;
        }
        let starts = self.search.peek().active().is_none() && !query.trim().is_empty();
        if starts && let Some(drawn) = self.drawn.peek().as_ref() {
            self.restore.set(Some(Restore::at(list_key(&self), drawn)));
        }
        self.search.write().query = query;
    }

    fn toggle_search_everywhere(mut self) {
        let mut search = self.search.write();
        search.everywhere = !search.everywhere;
    }

    /// Closes the search field and shows the list it searched, where it was.
    fn close_search(self) {
        if self.end_search() {
            // The field, or the bar's close button, may have had the focus.
            self.refocus();
        }
    }

    /// Closes the search field, leaving the focus where it is: on the search button
    /// that closed it. Returns whether it was open.
    fn end_search(mut self) -> bool {
        if *self.search.peek() == Search::default() {
            return false;
        }
        self.search.set(Search::default());
        true
    }

    /// Moves from the search field to the results, opening the first if none is open.
    fn leave_search_field(mut self, app: &Arc<App>) {
        self.pane.set(Pane::List);
        self.refocus();
        // Also when the open article isn't among the results.
        let listed = self
            .article
            .peek()
            .as_ref()
            .is_some_and(|open| self.articles.peek().rows.iter().any(|a| a.id == open.id));
        if !listed {
            self.step(app, 1);
        }
    }

    fn open_in_browser(self) {
        let Some(a) = self.article.peek().clone() else {
            return;
        };
        // The server's link, which may name any scheme: only a web page opens.
        let web = ["http://", "https://"].iter().any(|scheme| {
            a.url
                .get(..scheme.len())
                .is_some_and(|start| start.eq_ignore_ascii_case(scheme))
        });
        if !web {
            return;
        }
        if let Err(e) = webbrowser::open(&a.url) {
            tracing::error!("opening {} in the browser failed: {e}", a.url);
        }
    }

    /// Syncs the shown account. Returns a wait that ends with its sync, as
    /// [`App::sync_now`] says.
    fn sync(mut self, app: &Arc<App>) -> Option<impl Future<Output = ()> + use<>> {
        let account = self.account.peek().clone()?;
        self.syncs_asked += 1;
        app.sync_now(&account)
            .inspect_err(|e| tracing::error!("syncing failed: {e}"))
            .ok()
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

    /// Sets the width of the pane before `edge`, within its range, in whole pixels.
    fn resize(mut self, edge: Edge, width: f64) {
        let width = edge.range().clamp(width).round();
        if differs(edge.width(&self.layout.peek()), width) {
            *edge.width_mut(&mut self.layout.write()) = width;
        }
    }

    /// Saves the layout to config.json. Only the next start misses one not saved.
    fn save_layout(mut self, app: &App) {
        let layout = *self.layout.peek();
        if let Err(e) = app.set_layout(layout) {
            tracing::error!("saving the layout failed: {e}");
        }
        if differs(*self.saved_list_width.peek(), layout.list_width) {
            self.saved_list_width.set(layout.list_width);
        }
    }

    /// Ends a drag and saves the layout. A drag that resized gives the keys back to the
    /// window, as the press focused the resizer.
    fn end_drag(mut self, app: &App) {
        let Some(drag) = *self.drag.peek() else {
            return;
        };
        self.drag.set(None);
        self.save_layout(app);
        // Not when a sign-in screen took the panes' place meanwhile: it has the focus.
        if self.panes_shown() && differs(drag.edge.width(&self.layout.peek()), drag.saved_width) {
            self.refocus();
        }
    }

    /// Whether the window shows the panes, not a screen to add an account or sign in.
    fn panes_shown(&self) -> bool {
        self.account.peek().is_some()
            && !self.status.peek().as_ref().is_some_and(|s| s.unauthorized)
    }

    /// The width the pane before `edge` shows: less than its saved width when the window
    /// is too narrow for both panes, which shrinks the list.
    async fn shown_width(self, edge: Edge) -> f64 {
        let pane = match edge {
            Edge::List => self.list_pane.peek().clone(),
            Edge::Sidebar => None,
        };
        let shown = match pane {
            Some(pane) => pane.get_client_rect().await.ok().map(|rect| rect.width()),
            None => None,
        };
        // Read after the measure, so a step made meanwhile counts.
        let saved = edge.width(&self.layout.peek());
        shown.map_or(saved, |shown| shown.min(saved))
    }

    /// The width the list shows at, as CSS lays it out: its own, or less in a window
    /// too narrow for it beside the sidebar and the reader's least, but never less
    /// than its own least.
    fn list_shown_width(&self) -> f64 {
        list_shown_width(&self.layout.read(), (self.window_size)().0)
    }

    /// The pane the arrow keys move in. The list has them while the sidebar is
    /// hidden, whatever last selected a feed.
    fn active_pane(&self) -> Pane {
        if self.layout.read().sidebar_open {
            *self.pane.read()
        } else {
            Pane::List
        }
    }

    /// Hides or shows the sidebar. Hidden, it gives the list the arrow keys and closes
    /// its menus and questions, and the focus, which may have been in it, goes back to
    /// the window. It stays while a feed is being added, which hiding would drop.
    fn toggle_sidebar(mut self, app: &App) {
        let open = !self.layout.peek().sidebar_open;
        if !open && self.overlay.peek().busy() {
            return;
        }
        self.layout.write().sidebar_open = open;
        if !open {
            self.pane.set(Pane::List);
            let overlay = self.overlay.peek().after_sidebar_hidden();
            self.set_overlay(overlay);
            self.refocus();
        }
        self.save_layout(app);
    }

    /// Moves the sidebar's selection to the next (1) or previous (-1) scope, keeping
    /// the view. `sort` is the sidebar's order.
    fn step_sidebar(self, delta: isize, sort: SortSubscriptions) {
        // Before the library's guard: reading `pinned` may recompute it, which reads
        // the library.
        let pinned = (self.pinned)();
        let next = {
            let lib = self.library.peek();
            let Selection { view, scope } = self.selection.peek().clone();
            let collapsed = self.collapsed_folders();
            step_scope(&lib, view, &pinned, &collapsed, &scope, delta, sort)
        };
        if let Some(scope) = next {
            self.select_scope(scope);
        }
    }

    /// Shows the account and makes it the active one.
    fn switch_account(mut self, app: &App, id: &str) {
        if matches!(*self.overlay.peek(), Overlay::AccountMenu { .. }) {
            self.overlay.set(Overlay::None);
        }
        if self.account.peek().as_deref() == Some(id) {
            return;
        }
        // Only the next start misses a choice that wasn't saved.
        match app.set_active(id) {
            // Removed since the pick was offered, as from a menu not yet updated.
            Err(AppError::NoAccount) => return,
            Err(e) => tracing::error!("saving account {id} as the one shown failed: {e}"),
            Ok(()) => {}
        }
        self.show_account(app, Some(id.to_owned()));
    }

    /// Shows the account, keeping the view: its feeds, articles and status. `None` once
    /// there is no account.
    fn show_account(mut self, app: &App, id: Option<String>) {
        // Another account's feeds are its own, and so are its articles: its overlays go,
        // even a busy Add Feed, which then adds to the account it was for.
        self.set_overlay(Overlay::None);
        self.close_search();
        let view = self.selection.peek().view;
        self.select(Selection {
            view,
            scope: Scope::All,
        });
        self.status
            .set(id.as_deref().and_then(|id| app.status(id).ok()));
        self.account.set(id);
    }
}

/// Follows the shown `account`'s sync reports into `status`, and its cache's changes,
/// made here, in Settings or by a sync, into `cache`.
fn use_account_events(
    app: &Arc<App>,
    account: Signal<Option<String>>,
    mut status: Signal<Option<SyncStatus>>,
    mut cache: Signal<Cached>,
) {
    // Sync reports, for the sidebar's status.
    use_hook({
        let app = Arc::clone(app);
        move || {
            spawn(async move {
                let mut events = app.subscribe();
                // Catches a report sent before this subscribed, as by the first sync at
                // start.
                let shown = account.peek().clone();
                if let Some(id) = shown {
                    status.set(app.status(&id).ok());
                }
                loop {
                    match events.recv().await {
                        Ok(report) => {
                            let shown = Some(&report.account_id) == account.peek().as_ref();
                            if shown && status.peek().as_ref() != Some(&report) {
                                status.set(Some(report));
                            }
                        }
                        // Missed some: the latest status follows.
                        Err(RecvError::Lagged(_)) => {}
                        Err(RecvError::Closed) => break,
                    }
                }
            });
        }
    });
    // The shown account's cache changes, made here, in Settings or by a sync.
    crate::live::use_cache_changes(app, move |change| {
        let shown = change.is_none_or(|c| Some(&c.account_id) == account.peek().as_ref());
        if shown {
            cache.write().heard(change.map(|c| c.what));
        }
    });
}

/// The rows of the article list, loaded again as the account, the selection, the search
/// or the cache changes. A sync's reload keeps the rows read or unstarred here.
fn use_articles(
    app: &Arc<App>,
    account: Signal<Option<String>>,
    selection: Signal<Selection>,
    article: Signal<Option<ArticleSummary>>,
    cache: Signal<Cached>,
    marks: Signal<Marks>,
    search: Signal<Search>,
) -> Memo<Listed> {
    // The last load, what it listed and what it was for: a sync's reload keeps rows.
    let relist = use_hook(|| Rc::new(RefCell::new(Relist::default())));
    // What the list searches for: only a change here loads it again, not opening the
    // field or typing a space.
    let searched = use_memo(move || {
        search
            .read()
            .active()
            .map(|(query, everywhere)| (query.to_owned(), everywhere))
    });
    // Only the changes the list loads again for: not a read or star change.
    let relisted = use_memo(move || {
        let cache = cache.read();
        (cache.list, cache.cleared)
    });
    use_memo({
        let app = Arc::clone(app);
        move || {
            let (selection, account, (_, cleared)) = (selection(), account(), relisted());
            let searched = searched();
            let rows = account
                .as_ref()
                .and_then(|id| match &searched {
                    Some((query, everywhere)) => app
                        .search(id, &searched_within(&selection, *everywhere), query)
                        .ok(),
                    None => app.articles(id, &selection).ok(),
                })
                .unwrap_or_default();
            let fetch = |id: &str| {
                let account = account.as_deref()?;
                app.article(account, id).ok().flatten()
            };
            let load = Load {
                account: account.clone(),
                selection,
                searched,
            };
            relist.borrow_mut().next(
                load,
                rows,
                cleared,
                &marks.peek(),
                article.peek().as_ref(),
                fetch,
            )
        }
    })
}

/// The main window's state, its signals and what is worked out from them, given to its
/// parts as context.
fn use_reading(app: &Arc<App>) -> Reading {
    let account = use_signal(|| app.active());
    let selection = use_signal(|| Selection {
        view: View::Unread,
        scope: Scope::All,
    });
    let article = use_signal(|| None::<ArticleSummary>);
    let cache = use_signal(Cached::default);
    let marks = use_signal(Marks::default);
    let search = use_signal(Search::default);
    let status = use_signal(|| account.peek().as_deref().and_then(|id| app.status(id).ok()));

    use_account_events(app, account, status, cache);

    let library = use_memo({
        let app = Arc::clone(app);
        move || {
            cache.read();
            account()
                .and_then(|id| app.library(&id).ok())
                .unwrap_or_default()
        }
    });
    // A feed read to zero keeps its row until the account, view or selection changes
    // or a sync finishes, so rows don't jump while you read.
    let pinned_state = use_hook(|| Rc::new(RefCell::new(Pinned::default())));
    let pinned = use_memo(move || {
        let selection = selection();
        let last_sync = status.read().as_ref().and_then(|s| s.last_sync);
        let lib = library.read();
        let counts = counts_by_feed(&lib.counts, selection.view);
        pinned_state
            .borrow_mut()
            .update((account(), selection, last_sync), counts)
    });
    let articles = use_articles(app, account, selection, article, cache, marks, search);
    let window = use_window();
    use_context_provider(|| Reading {
        account,
        selection,
        article,
        cache,
        marks,
        library,
        pinned,
        articles,
        pane: Signal::new(Pane::List),
        overlay: Signal::new(Overlay::None),
        mark_generation: Signal::new(0),
        frame: Signal::new(None),
        status,
        syncs_asked: Signal::new(0),
        root: Signal::new(None),
        reader_view: Signal::new(ReaderView::default()),
        collapsed: Signal::new(HashMap::new()),
        layout: Signal::new(app.layout()),
        drag: Signal::new(None),
        saved_list_width: Signal::new(app.layout().list_width),
        window_size: Signal::new(window_size(&*window)),
        list_pane: Signal::new(None),
        drawn: CopyValue::new(None),
        ghost: Signal::new(None),
        leaving: Signal::new(None),
        search,
        search_field: Signal::new(None),
        restore: CopyValue::new(None),
    })
}

/// The window's root component.
#[component]
pub fn Root() -> Element {
    let app = use_context::<Arc<App>>();
    let live = use_live(&app);
    let mut reading = use_reading(&app);
    let window = use_window();
    use_context_provider(|| live);
    let window_state = chrome::use_window_state_provider();
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
    crate::badge::use_unread_badge(&app, &live);
    #[cfg(target_os = "macos")]
    crate::menu::use_menu_bar(&app, reading.account, live.revision, {
        let app = Arc::clone(&app);
        move |command| reading.run(&app, command)
    });
    // Settings closes with the main window, as the app does. The list follows the
    // window's size: its width may squeeze the list, its height the list's viewport.
    use_window_event({
        let (settings, windows) = (settings.clone(), windows.clone());
        let window = Arc::clone(&window);
        move |event, _| {
            use dioxus_native::winit::event::WindowEvent;
            match event {
                WindowEvent::CloseRequested => settings.close(&windows),
                // A new scale factor, which may keep the surface's size.
                WindowEvent::SurfaceResized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                    let size = window_size(&*window);
                    let was = *reading.window_size.peek();
                    // Minimized, as Windows has it: nothing shows to lay out.
                    let shown = size.0 > 0.0 && size.1 > 0.0;
                    if shown && (differs(was.0, size.0) || differs(was.1, size.1)) {
                        reading.window_size.set(size);
                    }
                }
                _ => {}
            }
        }
    });
    // Shown over whatever the window shows until dismissed: after a reset, the
    // add-account screen.
    let mut notice = use_signal(|| app.notice().cloned());
    let signed_out = (reading.status)().is_some_and(|s| s.unauthorized);
    let adding = !reading.panes_shown();
    // Each part reads only what it shows, so a drag re-renders only what it resizes.
    let dragging = use_memo(move || reading.drag.read().is_some())();
    let drag_app = Arc::clone(&app);
    let settings_now = live.settings.read();
    let theme = article::theme_name((live.theme)());
    let root_style = chrome::root_style(&fonts::stack(settings_now.ui_font), (live.frame)())
        + &scale::root_style(settings_now.font_size);
    let framed = (window_state.framed)();
    let canvas = canvas_css((live.theme)(), framed);
    let motion = motion(settings_now.animations);
    drop(settings_now);

    rsx! {
        style { {APP_CSS} }
        style { {canvas} }
        // Focusable, and focused from the start: a key press goes to the focused element,
        // and a click keeps focus on the nearest focusable ancestor, so every key the
        // window gets passes through here. -1 keeps the root out of the Tab order.
        div {
            class: "root",
            class: if dragging { "resizing" },
            class: if FRAMELESS { "frameless" },
            class: if MAC { "mac" },
            class: if LINUX { "linux" },
            class: if WINDOWS { "windows" },
            class: if framed { "framed" },
            "data-theme": theme,
            "data-motion": motion,
            style: root_style,
            tabindex: "-1",
            onmounted: move |e: MountedEvent| {
                // Without focus the app hears no keys, but works otherwise.
                reading.root.set(Some(e.data()));
                // The add-account screen focuses its first field instead, and the
                // config notice its button.
                if !adding && notice.peek().is_none() {
                    reading.refocus();
                }
            },
            onkeydown: move |e: Event<KeyboardData>| {
                if reading.on_key(&app, &live.keys.peek(), &e.data()) {
                    e.prevent_default();
                }
            },
            // A drag of a pane's border follows the pointer over the whole window.
            onmousemove: {
                let app = Arc::clone(&drag_app);
                move |e: Event<MouseData>| {
                    let Some(drag) = *reading.drag.peek() else {
                        return;
                    };
                    // Released outside the window, where no mouse-up reaches here.
                    if !e.held_buttons().contains(MouseButton::Primary) {
                        reading.end_drag(&app);
                        return;
                    }
                    let x = e.client_coordinates().x;
                    reading.resize(drag.edge, drag.width_at(x));
                }
            },
            onmouseup: move |_| reading.end_drag(&drag_app),
            // A click outside the open overlay closes it; its own clicks don't get here.
            // The click has moved the focus already.
            onclick: move |_| {
                let overlay = reading.overlay.peek().after_click();
                reading.set_overlay(overlay);
            },
            if (reading.account)().is_none() || signed_out {
                AddAccountScreen {}
            } else {
                SidebarSlot {}
                ArticleList {}
                Resizer { edge: Edge::List }
                Reader {}
            }
            if let Some(shown) = notice() {
                ConfigNotice {
                    notice: shown,
                    ondismiss: move |()| {
                        notice.set(None);
                        // The focused button goes with it. The add-account screen
                        // is left as a start without the notice leaves it.
                        if reading.panes_shown() {
                            reading.refocus();
                        }
                    },
                }
            }
            Frame {}
        }
    }
}

/// Tells the user that `config.json` couldn't be fully read, and where the old file is
/// kept, until they dismiss it. It takes the window's keys and clicks meanwhile: a click
/// in it focuses its backdrop, so keys still come here.
#[component]
fn ConfigNotice(notice: Notice, ondismiss: EventHandler<()>) -> Element {
    let (title, message, backup) = match &notice {
        Notice::SettingsReset { backup } => {
            ("configNotice.resetTitle", "configNotice.reset", backup)
        }
        Notice::AccountsLeftOut { backup } => {
            ("configNotice.leftOutTitle", "configNotice.leftOut", backup)
        }
    };
    let path = backup.display().to_string();
    rsx! {
        div {
            class: "notice-backdrop",
            tabindex: "-1",
            // Clicks behind it reach nothing, and close no overlay.
            onclick: move |e| e.stop_propagation(),
            // No key reaches the window behind it. Tab stays on its one button.
            onkeydown: move |e: Event<KeyboardData>| {
                e.stop_propagation();
                match e.key() {
                    Key::Named(NamedKey::Enter | NamedKey::Escape) => {
                        e.prevent_default();
                        ondismiss.call(());
                    }
                    Key::Named(NamedKey::Tab) => e.prevent_default(),
                    _ => {}
                }
            },
            div {
                class: "popover config-notice",
                role: "alertdialog",
                "aria-modal": "true",
                "aria-labelledby": "config-notice-title",
                "aria-describedby": "config-notice-message",
                p { id: "config-notice-title", class: "title", {t(title)} }
                p { id: "config-notice-message", class: "message", {t_with(message, &[("path", &path)])} }
                div { class: "actions",
                    button {
                        class: "button primary",
                        onmounted: move |e: MountedEvent| async move {
                            focus(&e.data(), "the notice's OK button").await;
                        },
                        onclick: move |_| ondismiss.call(()),
                        {t("common.ok")}
                    }
                }
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
        .and_then(|id| app.account(&id));
    let error = again
        .as_ref()
        .map(|a| t_with("app.signInAgain", &[("account", &a.description)]));
    rsx! {
        div { class: "add-account-screen",
            AddAccount { embedded: false, again, initial_error: error, on_added: |_| {} }
            // Over the drag strip, in a row, so the switcher leaves the buttons room.
            div { class: "screen-corner",
                if app.accounts().len() > 1 {
                    div { class: "other-accounts", AccountSwitcher {} }
                }
                MainWindowButtons {}
            }
        }
    }
}

/// The main window's [`WindowButtons`]: Settings closes with it, as on
/// `CloseRequested`.
#[component]
fn MainWindowButtons() -> Element {
    let settings = use_context::<SettingsWindow>();
    let windows = use_windows();
    rsx! {
        WindowButtons { on_close: move |()| settings.close(&windows) }
    }
}

/// How far toward `target` the list `el` can scroll once its rows are laid out.
///
/// This runs before Blitz lays out the rows' new heights, and a scroll past the old
/// range would stop at its end: it waits for the range, a few frames at most, and else
/// returns as far as it reaches.
pub(crate) async fn reachable(el: &MountedData, target: f64) -> f64 {
    let mut to = target;
    for _ in 0..LAYOUT_WAITS {
        match el.get_scroll_size().await {
            Ok(range) if range.height >= target - 0.5 => return target,
            Ok(range) => {
                to = range.height;
                tokio::time::sleep(FRAME).await;
            }
            Err(_) => break,
        }
    }
    to
}

/// Scrolls the article list `el` to `target` once its layout reaches it, as the scroll
/// waiting in `pending`, which it replaces; the user's scroll cancels it. `top` follows
/// the scroll, before the next frame, as the rows drawn do. `what` names it in a
/// warning if it fails.
fn scroll_list(
    el: Rc<MountedData>,
    target: f64,
    mut pending: CopyValue<Option<(f64, dioxus_core::Task)>>,
    mut top: Signal<f64>,
    what: &'static str,
) {
    if let Some((_, task)) = pending.take() {
        task.cancel();
    }
    let task = spawn(async move {
        let to = reachable(&el, target).await;
        pending.set(None);
        top.set(to);
        let to = dioxus_html::geometry::PixelsVector2D::new(0.0, to);
        if let Err(e) = el.scroll(to, ScrollBehavior::Instant).await {
            tracing::warn!("{what} failed: {e}");
        }
    });
    pending.set(Some((target, task)));
}

/// What [`scroll_into_view`] shows in a scrolling pane.
pub(crate) enum Reveal<'a> {
    /// This span of the pane's content, `top..bottom`, from its top.
    Span(f64, f64),
    /// This element in the pane, as laid out, with this much room either side.
    Element(&'a MountedData, f64),
}

/// Scrolls `pane` the least that shows `what`.
///
/// The effect or mount that asks runs before Blitz lays out the render that changed the
/// content, and a pane just shown has no size at all: a scroll then would stop at the
/// old range. This waits, a few frames at most, for the pane's size and a range that
/// reaches, and scrolls only if `latest` still says no newer scroll was asked for
/// meanwhile. Blitz's `scroll_to` scrolls only the window, never a pane.
pub(crate) async fn scroll_into_view(
    pane: &MountedData,
    what: Reveal<'_>,
    latest: impl Fn() -> bool,
) {
    for wait in 1..=LAYOUT_WAITS {
        tokio::time::sleep(FRAME).await;
        let (Ok(view), Ok(offset), Ok(range)) = (
            pane.get_client_rect().await,
            pane.get_scroll_offset().await,
            pane.get_scroll_size().await,
        ) else {
            return;
        };
        if view.height() <= 0.0 {
            continue;
        }
        let (top, bottom) = match what {
            Reveal::Span(top, bottom) => (top, bottom),
            Reveal::Element(el, room) => {
                let Ok(rect) = el.get_client_rect().await else {
                    return;
                };
                if rect.height() <= 0.0 {
                    continue;
                }
                // Blitz moves a pane's own rect by its scroll, as it does the element's:
                // the difference is where the element is in the pane's content.
                let top = rect.min_y() - view.min_y();
                (top - room, top + rect.height() + room)
            }
        };
        let Some(to) = reveal(offset.y, view.height(), top, bottom) else {
            return;
        };
        // A range short of the target may still be the old layout's; the last wait
        // scrolls as far as it goes.
        if to > range.height + 0.5 && wait < LAYOUT_WAITS {
            continue;
        }
        if !latest() {
            return;
        }
        let to = dioxus_html::geometry::PixelsVector2D::new(offset.x, to);
        if let Err(e) = pane.scroll(to, ScrollBehavior::Instant).await {
            tracing::warn!("scrolling a pane failed: {e}");
        }
        return;
    }
}

/// The width a list laid out as `layout` says shows at in a window `window_width` CSS px
/// wide: its own, or less in a window too narrow for it beside the sidebar and the
/// reader's least, but never less than its own least (`.list`'s `min-width`).
fn list_shown_width(layout: &Layout, window_width: f64) -> f64 {
    let room = window_width - sidebar_taken(layout) - RESIZER - READER_LEAST;
    layout.list_width.min(room).max(LIST_WIDTH.min)
}

/// The reader's width in a window `window_width` CSS px wide, beside the sidebar and
/// the list as `layout` lays them out.
fn reader_width(layout: &Layout, window_width: f64) -> f64 {
    window_width - sidebar_taken(layout) - list_shown_width(layout, window_width) - RESIZER
}

/// What the sidebar and its border take of the window's width, as `layout` has them.
fn sidebar_taken(layout: &Layout) -> f64 {
    if layout.sidebar_open {
        layout.sidebar_width + RESIZER
    } else {
        0.0
    }
}

/// The list's viewport's height in a window `window_height` CSS px high.
fn viewport_height(window_height: f64) -> f64 {
    (window_height - TOOLBAR).max(0.0)
}

/// `window`'s width and height in CSS px.
fn window_size(window: &dyn dioxus_native::winit::window::Window) -> (f64, f64) {
    let size = window
        .surface_size()
        .to_logical::<f64>(window.scale_factor());
    (size.width, size.height)
}

/// The height of a Large thumbnail, 16:9 at the width a list `list_width` wide gives it.
fn large_height(list_width: f64) -> f64 {
    ((list_width - 2.0 * PAD_X).max(0.0) * 9.0 / 16.0).round()
}

/// The size of a thumbnail beside the text, in CSS px, if `size` puts one there.
fn thumb_beside(size: ThumbnailSize) -> Option<f64> {
    match size {
        ThumbnailSize::Small => Some(40.0),
        ThumbnailSize::Medium => Some(64.0),
        ThumbnailSize::Large | ThumbnailSize::None => None,
    }
}

/// The width a headline wraps at in a list `list_width` wide, beside a thumbnail if
/// `size` puts one there and the article has one.
fn title_width(size: ThumbnailSize, list_width: f64, thumbnail: bool) -> f64 {
    let full = list_width - 2.0 * PAD_X;
    match thumb_beside(size).filter(|_| thumbnail) {
        Some(thumb) => full - thumb - GAP,
        None => full,
    }
}

/// Where each of `rows` starts, in CSS pixels, and then where the last one ends.
///
/// A row fits its headline's lines, `counts` for each row at its width (`title_width`)
/// and `font_size`, and its thumbnail: one beside the text at least as tall as
/// it, a Large one below. Each height is rounded up to a whole pixel, so rows never
/// start between pixels.
fn row_offsets(
    rows: &[ArticleSummary],
    size: ThumbnailSize,
    font_size: FontSize,
    list_width: f64,
    counts: &[u32],
) -> Vec<f64> {
    let (meta, title_line) = (meta(font_size), title_line(font_size));
    let large = (size == ThumbnailSize::Large).then(|| LARGE_GAP + large_height(list_width));
    let mut offsets = Vec::with_capacity(rows.len() + 1);
    let mut at = 0.0;
    offsets.push(at);
    for (a, &lines) in rows.iter().zip(counts) {
        let mut content = meta + f64::from(lines) * title_line;
        if let Some(thumb) = thumb_beside(size).filter(|_| a.thumbnail) {
            content = content.max(THUMB_MARGIN + thumb);
        } else if let Some(large) = large.filter(|_| a.thumbnail) {
            content += large;
        }
        at += (2.0 * PAD_Y + content).ceil();
        offsets.push(at);
    }
    offsets
}

#[cfg(test)]
mod tests {
    use super::reader::{
        Motion::{Off, On, Still},
        Pops,
    };
    use super::sidebar::sidebar_place;
    use super::*;
    use crate::place::visible_rows;

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
    fn search_restore_finds_the_first_row_still_listed() {
        let restore = Restore {
            list: String::new(),
            rows: vec![("1".into(), -20.0), ("2".into(), 60.0), ("3".into(), 140.0)],
            top: 500.0,
        };
        let rows = articles(&[false; 4]);
        let offsets = [0.0, 80.0, 160.0, 240.0, 320.0];
        assert_eq!(restore.target(&rows, &offsets), 100.0);
        // Row 1 gone, as read in Unread: row 2 keeps its place in the view.
        let rows: Vec<_> = rows.into_iter().filter(|a| a.id != "1").collect();
        assert_eq!(restore.target(&rows, &offsets), 20.0);
        // Above the list's start: its top.
        let rows: Vec<_> = rows.into_iter().filter(|a| a.id != "0").collect();
        assert_eq!(restore.target(&rows, &offsets), 0.0);
        assert_eq!(restore.target(&[], &[0.0]), 500.0);
    }

    /// A headline's lines as the tests count them: every character 10 px wide, wrapped
    /// anywhere at `width`.
    fn count_lines(title: &str, width: f64) -> u32 {
        #[expect(clippy::cast_precision_loss, reason = "test headlines are short")]
        let px = title.len() as f64 * 10.0;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "a count of lines, small and not negative"
        )]
        let lines = (px / width).ceil().max(1.0) as u32;
        lines.min(TITLE_LINES)
    }

    /// Where each of `rows` starts, its headline's lines counted by [`count_lines`].
    fn place(rows: &[ArticleSummary], size: ThumbnailSize, width: f64) -> Vec<f64> {
        let counts: Vec<u32> = rows
            .iter()
            .map(|a| count_lines(&a.title, title_width(size, width, a.thumbnail)))
            .collect();
        row_offsets(rows, size, FontSize::M, width, &counts)
    }

    #[test]
    fn only_the_visible_rows_and_some_either_side_render() {
        let offsets = place(&articles(&[false; 5_000]), ThumbnailSize::Large, 340.0);
        let row = offsets[1];
        assert_eq!(visible_rows(&offsets, 0.0, 10.0 * row), (0, 19));
        assert_eq!(
            visible_rows(&offsets, 1_000.0 * row, 10.0 * row),
            (992, 1019)
        );
        assert_eq!(
            visible_rows(&offsets, 4_999.0 * row, 10.0 * row),
            (4_991, 5_000)
        );
        let three = place(&articles(&[false; 3]), ThumbnailSize::Medium, 340.0);
        assert_eq!(visible_rows(&three, 0.0, 870.0), (0, 3));
        assert_eq!(visible_rows(&[0.0], 0.0, 870.0), (0, 0));
    }

    #[test]
    fn a_row_fits_its_headline_and_thumbnail() {
        let mut list = articles(&[true, false, true, false]);
        list[2].title = "x".repeat(45);
        list[3].title = "x".repeat(250);
        let heights = |size, width: f64| {
            let offsets = place(&list, size, width);
            offsets.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>()
        };
        let (meta, title_line) = (meta(FontSize::M), title_line(FontSize::M));
        let line = |n: f64| (2.0 * PAD_Y + meta + n * title_line).ceil();
        // At 340 px, the text is 300 px wide: the 450 px headline takes two lines, the
        // longest six, however long.
        assert_eq!(
            heights(ThumbnailSize::None, 340.0),
            [line(1.0), line(1.0), line(2.0), line(6.0)]
        );
        // Beside a 64 px thumbnail, the text is 224 px wide, which the 450 px headline
        // takes three lines of; an empty headline's row grows to fit the thumbnail.
        let medium = (2.0 * PAD_Y + THUMB_MARGIN + 64.0).ceil();
        assert!(line(2.0) < medium && medium < line(3.0));
        assert_eq!(
            heights(ThumbnailSize::Medium, 340.0),
            [medium, line(1.0), line(3.0), line(6.0)]
        );
        // A Large thumbnail is 16:9 at the text's width: 300 × 169, then 600 × 338.
        let large = |n: f64, h: f64| (2.0 * PAD_Y + meta + n * title_line + LARGE_GAP + h).ceil();
        assert_eq!(
            heights(ThumbnailSize::Large, 340.0),
            [large(1.0, 169.0), line(1.0), large(2.0, 169.0), line(6.0)]
        );
        assert_eq!(heights(ThumbnailSize::Large, 640.0)[0], large(1.0, 338.0));
        // Past the end, the last rows render.
        let offsets = place(&list, ThumbnailSize::Large, 340.0);
        assert_eq!(visible_rows(&offsets, 10_000.0, 500.0), (0, 4));
    }

    #[test]
    fn a_window_too_narrow_squeezes_the_list_down_to_its_least() {
        let layout = Layout {
            sidebar_open: true,
            sidebar_width: 230.0,
            list_width: 340.0,
        };
        // The sidebar, the list and their borders fit: the list has its width.
        let fit = 230.0 + 340.0 + 2.0 + READER_LEAST;
        assert_eq!(list_shown_width(&layout, 1200.0), 340.0);
        assert_eq!(list_shown_width(&layout, fit), 340.0);
        // One less, and the list gives it up, down to its least.
        assert_eq!(list_shown_width(&layout, fit - 1.0), 339.0);
        assert_eq!(list_shown_width(&layout, 400.0), LIST_WIDTH.min);
        // A hidden sidebar leaves the list its room.
        let hidden = Layout {
            sidebar_open: false,
            ..layout
        };
        assert_eq!(list_shown_width(&hidden, 300.0 + READER_LEAST), 299.0);
        // The reader takes the rest, and never less than its least.
        assert_eq!(reader_width(&layout, 1200.0), 1200.0 - 231.0 - 340.0 - 1.0);
        assert_eq!(reader_width(&layout, fit - 1.0), READER_LEAST);
    }

    #[test]
    fn a_drag_of_a_squeezed_list_narrows_from_its_shown_width_and_widens_from_its_saved() {
        let drag = Drag {
            edge: Edge::List,
            from_x: 600.0,
            from_width: 300.0,
            saved_width: 560.0,
        };
        assert_eq!(drag.width_at(550.0), 250.0);
        // Widening a list with no room doesn't lower the width it gets back.
        assert_eq!(drag.width_at(610.0), 560.0);
        assert_eq!(drag.width_at(900.0), 600.0);
        let roomy = Drag {
            from_width: 340.0,
            saved_width: 340.0,
            ..drag
        };
        assert_eq!(roomy.width_at(620.0), 360.0);
    }

    #[test]
    fn icons_pop_when_turned_not_when_opened() {
        let mut pops = Pops::default();
        // Opened unread, starred, in Reader View: nothing pops.
        assert_eq!(pops.update("a", false, true, true), (Still, Still, Still));
        // Marked read, then unread again: each way pops, and a re-render doesn't replay it.
        assert_eq!(pops.update("a", true, true, true), (Off, Still, Still));
        assert_eq!(pops.update("a", false, true, true), (On, Still, Still));
        assert_eq!(pops.update("a", false, true, true), (On, Still, Still));
        // Unstarred, then starred.
        assert_eq!(pops.update("a", false, false, true), (On, Off, Still));
        assert_eq!(pops.update("a", false, true, true), (On, On, Still));
        // Reader View off, then on from what it loaded before.
        assert_eq!(pops.update("a", false, true, false), (On, On, Off));
        assert_eq!(pops.update("a", false, true, true), (On, On, On));
        // Another article opens unread and starred: nothing pops.
        assert_eq!(pops.update("b", false, true, false), (Still, Still, Still));
    }

    #[test]
    fn sidebar_rows_are_placed_after_the_heading() {
        assert_eq!(
            sidebar_place(0, FontSize::M),
            Place {
                top: 4.0,
                height: 28.0
            }
        );
        // Feeds start after all feeds' row and the heading: 14 px high, 18 px of margins.
        assert_eq!(
            sidebar_place(1, FontSize::M),
            Place {
                top: 64.0,
                height: 28.0
            }
        );
        assert_eq!(
            sidebar_place(3, FontSize::M),
            Place {
                top: 120.0,
                height: 28.0
            }
        );
    }
}
