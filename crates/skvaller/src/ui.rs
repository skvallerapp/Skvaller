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
use std::time::{Duration, Instant};

use dioxus_html::geometry::WheelDelta;
use dioxus_html::input_data::MouseButton;
use dioxus_native::prelude::*;
use dioxus_native::{
    BlitzScrollSource, BlitzWheelPhase, NativeWheelData, use_window, use_window_event, use_windows,
};
use skvaller_app::{
    App, Error as AppError, FontSize, LIST_WIDTH, Layout, Library, MarkReadOn, PaneWidth,
    SIDEBAR_WIDTH, SortSubscriptions, SyncStatus, ThumbnailSize,
};
use skvaller_core::{ArticleSummary, Counts, Scope, Selection, View};
use tokio::sync::broadcast::error::RecvError;

use crate::add_account::AddAccount;
use crate::article;
use crate::chrome::{self, DragArea, FRAMELESS, Frame, LINUX, MAC, WindowButtons};
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
use crate::shortcuts::{ACCOUNT_MODIFIER, Action, COMMAND_MODIFIER};
use crate::spin::Spin;
use crate::thumbs;

mod command;
mod listed;
mod manage;

pub(crate) use command::Command;
use command::{Overlay, leave};
use listed::{Listed, Load, Marks, Relist};

use manage::{AddFeed, ConfirmRemove, Manage, ManageMenu, RenameField, Target, is_local};

pub(crate) const APP_CSS: &str = include_str!("app.css");

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
/// The reader's least width (`.linux .reader`'s `min-width`): on Linux, room for the
/// window buttons at the end of its toolbar, the wider GNOME ones, with its padding.
/// The list gives up width before the reader does.
const READER_LEAST: f64 = if LINUX { 116.0 } else { 0.0 };
/// The reader's width that holds an open article's buttons too, on Linux: their four
/// and the window buttons, with the toolbar's gaps. A narrower reader leaves them
/// out, as they would run under the window buttons: Blitz clips what overflows but
/// still lets it take presses.
const READER_WITH_BUTTONS: f64 = 248.0;
/// The main window's least width: on Linux the widest sidebar, the narrowest list and
/// the reader's least, so the window buttons always show; never under 640 px.
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
/// feed's content. Long enough for the cache, which most
/// pages come from after a sync's prefetch; short enough not to feel stuck.
const READER_VIEW_HOLD: Duration = Duration::from_millis(400);

/// How much of the article's height reading on scrolls: a page, less some lines to keep
/// the place.
const READ_ON_PAGE: f64 = 0.9;

/// How long the sidebar takes to slide open or closed: app.css's `.sidebar-slot`
/// transition. Closing, the sidebar stays in the document this long.
const SIDEBAR_SLIDE: Duration = Duration::from_millis(180);

/// How long a folder's feeds take to slide open or closed: app.css's `.folder-feeds`
/// and `.highlight.folding` transitions. Closing, the rows stay in the document this long.
const FOLDER_SLIDE: Duration = Duration::from_millis(160);

/// How long the list takes to slide on to the next one after mark-all-read: app.css's
/// `list-in` and `list-out`.
const LIST_SLIDE: Duration = Duration::from_millis(320);
/// How long the old list's ghost stays: the slide, and some frames for it to start,
/// which on a slow machine may be a few frames after the mark. Ending the slide early
/// would snap the next list the last pixels into place.
const GHOST_STAYS: Duration = LIST_SLIDE.saturating_add(Duration::from_millis(3 * 16));

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
    /// The open article, as it was when opened: [`Reading::open_article`] has its flags
    /// now.
    article: Signal<Option<ArticleSummary>>,
    /// Bumped when the cache changed under the lists: they load again, keeping what was
    /// read or unstarred here meanwhile. By a sync's report, and [`Self::feeds_changed`].
    revision: Signal<u64>,
    /// Bumped to load the list again without keeping those: [`Self::all_marked_read`].
    cleared: Signal<u64>,
    /// Bumped when a change made here changed the counts: [`Self::flags_changed`].
    counts: Signal<u64>,
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
        self.articles
            .read()
            .rows
            .iter()
            .filter(|a| !self.is_read(a))
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

    /// The shown account's feeds or folders changed here: added, moved, renamed or
    /// removed. The list loads again, as its rows show feeds' names and a folder's list
    /// gains or loses a feed's articles. The sidebar needs nothing from here: every feed
    /// change in [`App`] announces itself, which `live.revision` and the library follow.
    fn feeds_changed(mut self) {
        self.revision += 1;
    }

    /// A read or star change made here: the library loads again for its counts. The
    /// list keeps its rows, which show the change through `marks`.
    fn flags_changed(mut self) {
        self.counts += 1;
    }

    /// Every article of the list was marked read: in Unread it loads again without
    /// keeping the rows read here, so it empties.
    fn all_marked_read(mut self) {
        self.cleared += 1;
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
        if *self.overlay.peek() != left.overlay {
            self.overlay.set(left.overlay);
        }
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
        let overlay = self.overlay.peek().after_list_change();
        if *self.overlay.peek() != overlay {
            self.overlay.set(overlay);
        }
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
        self.flags_changed();
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
        self.flags_changed();
    }

    /// Marks all as read, asking first when the settings say so. Not while Add Feed is
    /// busy, which the question can't replace.
    fn request_mark_all(self, app: &App) {
        if self.unread_count() == 0 {
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
    /// and the open article closes. With All Unread already
    /// selected, the list empties at once, except for the article being read.
    fn mark_all_read(mut self, app: &App) {
        if *self.overlay.peek() == Overlay::MarkAll {
            self.overlay.set(Overlay::None);
        }
        // The question's focused button goes, or the one that asked keeps the focus.
        self.refocus();
        // Read, not peek: `is_read` reads the list too, and a recompute under a peek's
        // guard panics.
        let ids: Vec<String> = self
            .articles
            .read()
            .rows
            .iter()
            .filter(|a| !self.is_read(a))
            .map(|a| a.id.clone())
            .collect();
        self.set_read(app, &ids, true);
        let Selection { view, scope } = self.selection.peek().clone();
        if view != View::Unread {
            return;
        }
        self.all_marked_read();
        // Search results don't move on to another feed.
        if self.search.peek().active().is_some() {
            return;
        }
        // Before the library's guard: reading `pinned` may recompute it, which reads
        // the library. Read, not peek: a peek can return the library from before these
        // counts.
        let pinned = (self.pinned)();
        let sort = app.settings().sort_subscriptions;
        let next = {
            let lib = self.library.read();
            let collapsed = self.collapsed_folders();
            next_with_articles(&lib, view, &pinned, &collapsed, &scope, sort)
        };
        if let Some(scope) = next {
            let list = list_key(&self);
            self.select(Selection { view, scope });
            if app.settings().animations {
                self.slide_away(list);
            }
        }
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
        spawn(async move {
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
        // Their keys would keep the field's, such as a paste, from it. The field takes
        // the focus.
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
            if let Err(e) = field.set_focus(true).await {
                tracing::warn!("focusing the search field failed: {e}");
            }
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
    fn close_search(mut self) {
        if *self.search.peek() == Search::default() {
            return;
        }
        self.search.set(Search::default());
        self.refocus();
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
        if a.url.is_empty() {
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
            if *self.overlay.peek() != overlay {
                self.overlay.set(overlay);
            }
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
        if *self.overlay.peek() != Overlay::None {
            self.overlay.set(Overlay::None);
        }
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
    let search = use_signal(Search::default);
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
                            // The thumbnails' prefetch reports after each batch, most
                            // often with nothing new to show: no render then.
                            if status.peek().as_ref() != Some(&report) {
                                status.set(Some(report));
                            }
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
            // A sync changed the cache.
            revision();
            // Read or star changes made here changed the counts.
            counts();
            // The config changed: a feed added, moved, renamed or removed, its Reader
            // View turned on or off, here or in Settings.
            (live.revision)();
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
    let articles = use_memo({
        let app = Arc::clone(&app);
        move || {
            revision();
            let (selection, account, cleared) = (selection(), account(), cleared());
            let searched = searched();
            let rows = account
                .as_ref()
                .and_then(|id| match &searched {
                    Some((query, everywhere)) => {
                        let scope = if *everywhere {
                            Scope::All
                        } else {
                            selection.scope.clone()
                        };
                        let within = Selection {
                            view: selection.view,
                            scope,
                        };
                        app.search(id, &within, query).ok()
                    }
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
    });
    let window = use_window();
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
        search,
        search_field: Signal::new(None),
        restore: CopyValue::new(None),
    });
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
    #[cfg(target_os = "macos")]
    crate::menu::use_menu_bar(&app, reading.account, live.revision, {
        let app = Arc::clone(&app);
        move |command| reading.run(&app, command)
    });
    // Settings goes with the main window, as the app does, and the list follows the
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
    let motion = motion(settings_now.animations);
    drop(settings_now);

    rsx! {
        style { {APP_CSS} }
        // Focusable, and focused from the start: a key press goes to the focused element,
        // and a click keeps focus on the nearest focusable ancestor, so every key the
        // window gets passes through here. -1 keeps the root out of the Tab order.
        div {
            class: "root",
            class: if dragging { "resizing" },
            class: if FRAMELESS { "frameless" },
            class: if MAC { "mac" },
            class: if LINUX { "linux" },
            class: if framed { "framed" },
            "data-theme": theme,
            "data-motion": motion,
            style: root_style,
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
                if *reading.overlay.peek() != overlay {
                    reading.overlay.set(overlay);
                }
            },
            if (reading.account)().is_none() || signed_out {
                AddAccountScreen {}
            } else {
                SidebarSlot {}
                ArticleList {}
                Resizer { edge: Edge::List }
                Reader {}
            }
            Frame {}
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

/// The article counts per feed that `view` shows: starred for Starred, else unread.
///
/// Feeds without any are left out.
fn counts_by_feed(counts: &Counts, view: View) -> &HashMap<String, i64> {
    match view {
        View::Starred => &counts.starred_by_feed,
        View::Unread | View::All => &counts.unread_by_feed,
    }
}

/// What the account, view and selection were, and when the last sync finished.
type PinnedKey = (Option<String>, Selection, Option<i64>);

/// The feeds with articles when the key last changed.
#[derive(Debug, Default)]
struct Pinned {
    key: Option<PinnedKey>,
    feeds: Rc<HashSet<String>>,
}

impl Pinned {
    /// The feeds in `counts`, taken again only when `key` changed.
    fn update(&mut self, key: PinnedKey, counts: &HashMap<String, i64>) -> Rc<HashSet<String>> {
        if self.key.as_ref() != Some(&key) {
            self.key = Some(key);
            self.feeds = Rc::new(counts.keys().cloned().collect());
        }
        Rc::clone(&self.feeds)
    }
}

/// A row at the top level of the sidebar's feeds.
#[derive(Debug)]
enum Entry<'a> {
    /// A folder with its shown feeds, collapsed or not.
    Folder(&'a skvaller_core::Folder, Vec<&'a skvaller_core::Feed>),
    /// A feed in no folder.
    Feed(&'a skvaller_core::Feed),
}

impl Entry<'_> {
    fn title(&self) -> &str {
        match self {
            Self::Folder(folder, _) => &folder.title,
            Self::Feed(feed) => &feed.title,
        }
    }

    /// The row's key, so a row that moves keeps its state.
    fn key(&self) -> String {
        match self {
            Self::Folder(folder, _) => format!("folder-{}", folder.id),
            Self::Feed(feed) => feed.id.clone(),
        }
    }
}

/// Orders titles as SQLite's NOCASE does, as the cache sorts each list.
///
/// ASCII letters are folded, then the UTF-8 bytes compare.
fn nocase(a: &str, b: &str) -> std::cmp::Ordering {
    let a = a.bytes().map(|c| c.to_ascii_lowercase());
    a.cmp(b.bytes().map(|c| c.to_ascii_lowercase()))
}

/// What the sidebar shows of `lib` in `view`, in the `sort` order. All shows every
/// subscribed feed. Unread and Starred show the feeds with articles in the view or
/// `pinned`, the folders with any, and always the selected `scope`.
fn sidebar_tree<'a>(
    lib: &'a Library,
    view: View,
    pinned: &HashSet<String>,
    scope: &Scope,
    sort: SortSubscriptions,
) -> Vec<Entry<'a>> {
    let counts = counts_by_feed(&lib.counts, view);
    let mut by_folder: HashMap<Option<&str>, Vec<&skvaller_core::Feed>> = HashMap::new();
    for feed in lib.feeds.iter().filter(|f| f.subscribed) {
        let shown = view == View::All
            || counts.contains_key(&feed.id)
            || pinned.contains(&feed.id)
            || matches!(scope, Scope::Feed { id } if *id == feed.id);
        if shown {
            by_folder
                .entry(feed.folder_id.as_deref())
                .or_default()
                .push(feed);
        }
    }
    let folders = lib.folders.iter().filter_map(|folder| {
        let feeds = by_folder
            .remove(&Some(folder.id.as_str()))
            .unwrap_or_default();
        let shown = view == View::All
            || !feeds.is_empty()
            || matches!(scope, Scope::Folder { id } if *id == folder.id);
        shown.then_some(Entry::Folder(folder, feeds))
    });
    let mut entries: Vec<_> = folders.collect();
    let loose = by_folder.remove(&None).unwrap_or_default();
    entries.extend(loose.into_iter().map(Entry::Feed));
    // Each list is A–Z already. Stable, so a folder stays before a feed of its name.
    if sort == SortSubscriptions::Alphabetical {
        entries.sort_by(|a, b| nocase(a.title(), b.title()));
    }
    entries
}

/// The scope `delta` rows from `scope` in the sidebar, if there is a row there.
fn step_scope(
    lib: &Library,
    view: View,
    pinned: &HashSet<String>,
    collapsed: &HashSet<String>,
    scope: &Scope,
    delta: isize,
    sort: SortSubscriptions,
) -> Option<Scope> {
    let row = row_scope(lib, scope, collapsed);
    let scopes = sidebar_scopes(&sidebar_tree(lib, view, pinned, scope, sort), collapsed);
    scopes
        .iter()
        .position(|s| *s == row)
        .and_then(|i| i.checked_add_signed(delta))
        .and_then(|i| scopes.get(i))
        .cloned()
}

/// The sidebar row that stands for `scope`: a feed in a collapsed folder has no row of
/// its own, so its folder's stands for it, for the arrow keys and the highlight.
fn row_scope(lib: &Library, scope: &Scope, collapsed: &HashSet<String>) -> Scope {
    match scope {
        Scope::Feed { id } => lib
            .feeds
            .iter()
            .find(|f| &f.id == id)
            .and_then(|f| f.folder_id.clone())
            .filter(|folder| collapsed.contains(folder))
            .map_or_else(|| scope.clone(), |id| Scope::Folder { id }),
        _ => scope.clone(),
    }
}

/// The next row below `scope` with articles in `view`, else All.
/// `None` when `scope` has no row: All, or a feed in a collapsed folder.
fn next_with_articles(
    lib: &Library,
    view: View,
    pinned: &HashSet<String>,
    collapsed: &HashSet<String>,
    scope: &Scope,
    sort: SortSubscriptions,
) -> Option<Scope> {
    if *scope == Scope::All {
        return None;
    }
    let counts = counts_by_feed(&lib.counts, view);
    let entries = sidebar_tree(lib, view, pinned, scope, sort);
    // Every feed with articles is in the entries, collapsed or not.
    let folders: HashSet<&str> = entries
        .iter()
        .filter_map(|entry| match entry {
            Entry::Folder(folder, feeds) if feeds.iter().any(|f| counts.contains_key(&f.id)) => {
                Some(folder.id.as_str())
            }
            _ => None,
        })
        .collect();
    let has_articles = |row: &Scope| match row {
        Scope::Feed { id } => counts.contains_key(id),
        Scope::Folder { id } => folders.contains(id.as_str()),
        Scope::All => false,
    };
    let rows = sidebar_scopes(&entries, collapsed);
    let at = rows.iter().position(|row| row == scope)?;
    Some(
        rows[at + 1..]
            .iter()
            .find(|row| has_articles(row))
            .cloned()
            .unwrap_or(Scope::All),
    )
}

/// The sidebar's scopes from top to bottom, as the arrow keys step through them: all
/// feeds, then each entry, with a folder's feeds unless it is `collapsed`.
fn sidebar_scopes(entries: &[Entry<'_>], collapsed: &HashSet<String>) -> Vec<Scope> {
    let feed = |f: &skvaller_core::Feed| Scope::Feed { id: f.id.clone() };
    let mut scopes = vec![Scope::All];
    for entry in entries {
        match entry {
            Entry::Folder(folder, feeds) => {
                scopes.push(Scope::Folder {
                    id: folder.id.clone(),
                });
                if !collapsed.contains(&folder.id) {
                    scopes.extend(feeds.iter().map(|f| feed(f)));
                }
            }
            Entry::Feed(f) => scopes.push(feed(f)),
        }
    }
    scopes
}

#[component]
fn Sidebar() -> Element {
    let app = use_context::<Arc<App>>();
    let live = use_context::<Live>();
    let dates = (live.dates)();
    let reading = use_context::<Reading>();
    let lib = (reading.library)();
    let current = (reading.selection)();
    let counts = counts_by_feed(&lib.counts, current.view);
    let all_label = t(&format!("views.{}.all", view_key(current.view)));
    let all_count: i64 = counts.values().sum();
    let count_of = |id: &str| counts.get(id).copied().unwrap_or(0);
    let status = (reading.status)();
    let spinning = use_spin(reading.status, reading.syncs_asked);
    let sync_app = Arc::clone(&app);
    let settings_app = Arc::clone(&app);
    let settings_key = if COMMAND_MODIFIER == Modifiers::META {
        "⌘,".to_owned()
    } else {
        format!("{}+,", t("keys.ctrl"))
    };
    let status_line = match status {
        // The reason belongs in a hover title, but Blitz shows no titles yet.
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
    let sort = live.settings.read().sort_subscriptions;
    let font_size = use_memo(move || live.settings.read().font_size);
    let entries = sidebar_tree(
        &lib,
        current.view,
        &reading.pinned.read(),
        &current.scope,
        sort,
    );
    let sidebar_width = use_memo(move || reading.layout.read().sidebar_width)();
    let feeds_pane = use_signal(|| None::<Rc<MountedData>>);
    // Pulled past their ends, the feeds stretch, with no refresh.
    let pull = use_hook(|| Pull::new(&live, feeds_pane, None));
    // Unlike the article list, the feeds don't know their scroll range: measured again
    // when rows come or go, a folder opens or closes, the rows grow or shrink with the
    // font size, or the window resizes.
    use_effect(move || {
        reading.library.read();
        reading.collapsed.read();
        reading.pinned.read();
        reading.window_size.read();
        font_size.read();
        spawn(async move {
            tokio::time::sleep(FOLDER_SLIDE + FRAME).await;
            pull.measure().await;
        });
    });
    let settle = use_hook(|| Rc::new(RefCell::new(Settle::<Scope>::new())));
    // The selection's row, for the scroll after this render too: a feed in a collapsed
    // folder's is its folder's.
    let selected_row = use_hook(|| Rc::new(std::cell::Cell::new(None::<usize>)));
    // Counts the scrolls asked for: one still waiting for a layout must not land after a
    // newer one.
    let scrolls = use_hook(|| Rc::new(std::cell::Cell::new(0_u64)));
    // The collapsed folders the last render had: a folder opening or closing above the
    // selection slides its highlight along with the rows, instead of moving it at once.
    let last_collapsed = use_hook(|| Rc::new(RefCell::new(HashSet::new())));
    let collapsed = reading.collapsed_folders();
    let row_scope = row_scope(&lib, &current.scope, &collapsed);
    let row = sidebar_scopes(&entries, &collapsed)
        .iter()
        .position(|s| *s == row_scope);
    selected_row.set(row);
    let folded = *last_collapsed.borrow() != collapsed;
    last_collapsed.replace(collapsed);
    let highlight = row.map(|i| {
        let place = sidebar_place(i, font_size());
        let settles = settle
            .borrow_mut()
            .settles(current.scope.clone(), place.top);
        let class = if settles && folded {
            "highlight folding"
        } else {
            highlight_class(settles)
        };
        (class, place)
    });
    // The selection, as by the arrow keys, scrolls into view; every feed, at the top,
    // shows the top. Blitz scrolls only the window into view, not the feeds' own scroll:
    // done here, from the row's place as this render found it.
    use_effect(move || {
        reading.selection.read();
        let scroll = scrolls.get() + 1;
        scrolls.set(scroll);
        let (Some(index), Some(pane)) = (selected_row.get(), feeds_pane.peek().clone()) else {
            return;
        };
        let scrolls = Rc::clone(&scrolls);
        spawn(async move {
            // Every feed's row shows the top too.
            let place = sidebar_place(index, *font_size.peek());
            let top = if index == 0 { 0.0 } else { place.top };
            let row = Reveal::Span(top, place.top + place.height);
            scroll_into_view(&pane, row, || scrolls.get() == scroll).await;
            // Code-driven scrolls send no scroll event.
            pull.measure().await;
        });
    });
    rsx! {
        nav { class, aria_label: t("sidebar.label"), style: "width: {sidebar_width}px",
            div { class: "toolbar",
                DragArea {}
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
                    onclick: move |_| reading.run(&settings_app, Command::Settings),
                    Icon { size: 18, stroke: "2", paths: SETTINGS }
                }
                button {
                    class: if spinning { "icon-button sync spinning" } else { "icon-button sync" },
                    aria_label: t("sidebar.sync"),
                    title: t("sidebar.sync"),
                    onclick: move |_| reading.run(&sync_app, Action::Sync.into()),
                    Icon { size: 18, stroke: "2", paths: ROTATE_CW }
                }
            }
            header { class: "account",
                DragArea {}
                h1 { "{title}" }
                p { class: if status_line.1 { "failed" } else { "" }, "{status_line.0}" }
            }
            Feeds { pull,
                if let Some((class, place)) = highlight {
                    div { class, style: "transform: translateY({place.top}px); height: {place.height}px" }
                }
                ScopeItem { scope: Scope::All, title: all_label, count: all_count, nested: false, expanded: None }
                if !entries.is_empty() {
                    div { class: "heading", {t("sidebar.feeds")} }
                }
                for entry in entries {
                    // Keyed here: the match makes each row an expression, which keeps
                    // no key of its own.
                    Fragment { key: "{entry.key()}",
                    match entry {
                        Entry::Folder(folder, feeds) => {
                            let scope = Scope::Folder { id: folder.id.clone() };
                            let count: i64 = feeds.iter().map(|f| count_of(&f.id)).sum();
                            let expanded = !reading.is_collapsed(&folder.id);
                            let feeds: Vec<_> =
                                feeds.into_iter().map(|f| (f.clone(), count_of(&f.id))).collect();
                            rsx! {
                                ScopeItem { key: "folder-{folder.id}", scope, title: folder.title.clone(), count, nested: false, expanded }
                                FolderFeeds { feeds, expanded, row: scale::sidebar_row(font_size()) }
                            }
                        }
                        Entry::Feed(feed) => rsx! {
                            FeedItem { key: "{feed.id}", count: count_of(&feed.id), feed: feed.clone(), nested: false }
                        },
                    }
                    }
                }
            }
            ViewSwitcher {}
            ManageMenu {}
        }
    }
}

/// The sidebar's scrolling feeds, apart so that only they draw a pull, not the sidebar.
#[component]
fn Feeds(pull: Pull, children: Element) -> Element {
    let (mut mounted, mut at) = (pull.mounted, pull.at);
    let stretched = stretch_style((pull.shown)());
    let mut wheel_at = use_hook(|| CopyValue::new(None::<Instant>));
    rsx! {
        div { class: "feeds-clip",
        div {
            class: "feeds",
            style: "{stretched}",
            onmounted: move |e: MountedEvent| mounted.set(Some(e.data())),
            onscroll: move |e: Event<ScrollData>| at.write().top = e.data().scroll_top(),
            // Measured anew as a gesture begins, for its next events: `onscroll`
            // follows the scroll meanwhile.
            onwheel: move |e| {
                pull.on_wheel(&e);
                let now = Instant::now();
                let last = wheel_at.replace(Some(now));
                if last.is_none_or(|at| now.duration_since(at) > WHEEL_IDLE) {
                    spawn(pull.measure());
                }
            },
            ontouchstart: move |e| {
                pull.on_touch_start(&e);
                spawn(pull.measure());
            },
            ontouchmove: move |e| pull.on_touch_move(&e),
            ontouchend: move |e| pull.on_touch_end(&e),
            ontouchcancel: move |e| pull.on_touch_cancel(&e),
            {children}
        }
        }
    }
}

/// Whether the sync icon turns, as [`Spin`] says.
fn use_spin(status: Signal<Option<SyncStatus>>, asked: Signal<u64>) -> bool {
    let live = use_context::<Live>();
    let syncing = use_memo(move || status.read().as_ref().is_some_and(|s| s.syncing));
    let mut spin = use_hook(|| CopyValue::new(Spin::new(*asked.peek())));
    let mut turning = use_signal(|| false);
    use_effect(move || {
        let (syncing, asked) = (syncing(), asked());
        let animate = live.settings.peek().animations;
        let stop = spin.write().follow(syncing, asked, Instant::now(), animate);
        let turns = spin.peek().turning();
        let shown = *turning.peek();
        if turns != shown {
            turning.set(turns);
        }
        if let Some(stop) = stop {
            spawn(async move {
                tokio::time::sleep(stop.wait).await;
                let stopped = spin.write().stop(stop);
                if stopped {
                    turning.set(false);
                }
            });
        }
    });
    turning()
}

/// The sidebar and its border while it shows, sliding open and closed.
///
/// The slot stays in the document, at no width while closed, so opening changes its
/// width from one it had, which Blitz transitions. The sidebar in it is removed once
/// closed rather than hidden with CSS: Blitz's layout keeps stale children of nodes
/// removed under `display: none`. Its border goes at once.
#[component]
fn SidebarSlot() -> Element {
    let reading = use_context::<Reading>();
    let open = use_memo(move || reading.layout.read().sidebar_open)();
    let width = use_memo(move || reading.layout.read().sidebar_width)();
    let shown = use_closing(open, SIDEBAR_SLIDE);
    let slot_width = if open { width } else { 0.0 };

    rsx! {
        div { class: "sidebar-slot", style: "width: {slot_width}px",
            if shown {
                Sidebar {}
            }
        }
        if open {
            Resizer { edge: Edge::Sidebar }
        }
    }
}

/// Whether something `open` is still in the document: while open, and for `slide` after
/// it closes, so its closing transition plays. With Settings → Animations off, only
/// while open.
fn use_closing(open: bool, slide: Duration) -> bool {
    let live = use_context::<Live>();
    // Shown at the last change: true from an open until its close's slide ends.
    let mut present = use_signal(move || open);
    // Counts the changes, so a close's delayed removal knows a later one came.
    let changes = use_hook(|| Rc::new(std::cell::Cell::new(0_u64)));
    use_effect(use_reactive!(|open| {
        let change = changes.get() + 1;
        changes.set(change);
        let was = *present.peek();
        if open == was {
            return;
        }
        if open || !live.settings.peek().animations {
            present.set(open);
            return;
        }
        let changes = Rc::clone(&changes);
        spawn(async move {
            tokio::time::sleep(slide).await;
            if changes.get() == change {
                present.set(false);
            }
        });
    }));
    open || present()
}

/// The border after a pane, dragged or moved with ← and → to resize that pane.
#[component]
fn Resizer(edge: Edge) -> Element {
    let app = use_context::<Arc<App>>();
    let mut reading = use_context::<Reading>();
    let width = use_memo(move || edge.width(&reading.layout.read()))();
    let range = edge.range();
    let dragging = use_memo(move || reading.drag.read().is_some_and(|d| d.edge == edge))();
    let label = match edge {
        Edge::Sidebar => t("app.resizeSidebar"),
        Edge::List => t("app.resizeList"),
    };
    rsx! {
        div {
            class: if dragging { "resizer dragging" } else { "resizer" },
            role: "separator",
            tabindex: "0",
            aria_label: label,
            "aria-orientation": "vertical",
            "aria-valuenow": "{width}",
            "aria-valuemin": "{range.min}",
            "aria-valuemax": "{range.max}",
            onmousedown: move |e: Event<MouseData>| {
                if e.trigger_button() != Some(MouseButton::Primary) {
                    return;
                }
                // Not prevented: the press focuses the resizer, for the arrow keys.
                let from_width = edge.width(&reading.layout.peek());
                let from_x = e.client_coordinates().x;
                let saved_width = from_width;
                reading.drag.set(Some(Drag { edge, from_x, from_width, saved_width }));
                // A list the window shrank moves from the width it shows.
                spawn(async move {
                    let shown = reading.shown_width(edge).await;
                    let squeezed = |d: &Drag| d.edge == edge && differs(d.from_width, shown);
                    let drag = *reading.drag.peek();
                    if let Some(drag) = drag.filter(squeezed) {
                        reading.drag.set(Some(Drag { from_width: shown, ..drag }));
                    }
                });
            },
            onkeydown: move |e: Event<KeyboardData>| {
                let step = if e.modifiers().shift() { 40.0 } else { 10.0 };
                let by = match e.key() {
                    Key::Named(NamedKey::ArrowLeft) => -step,
                    Key::Named(NamedKey::ArrowRight) => step,
                    _ => return,
                };
                // The window's own arrow keys would move between the panes as well.
                e.stop_propagation();
                e.prevent_default();
                let app = Arc::clone(&app);
                spawn(async move {
                    // Narrowing a list the window shrank starts from the width it shows;
                    // widening starts from the saved width, which it never lowers.
                    let width = if by < 0.0 {
                        reading.shown_width(edge).await
                    } else {
                        edge.width(&reading.layout.peek())
                    };
                    reading.resize(edge, width + by);
                    reading.save_layout(&app);
                });
            },
            div { class: "line" }
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
    let index = VIEWS.iter().position(|v| *v == shown).unwrap_or(0);

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
            style: "--count: {VIEWS.len()}",
            // Under the checked button.
            span { class: "indicator", style: "--index: {index}", "aria-hidden": "true" }
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
                        // A clicked button lets the arrow keys go back to the sidebar and
                        // the list. Tab still reaches it.
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
    let menu = use_memo(move || match *reading.overlay.read() {
        Overlay::AccountMenu { at } => Some(at),
        _ => None,
    })();
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
                    if matches!(*reading.overlay.peek(), Overlay::AccountMenu { .. }) {
                        reading.overlay.set(Overlay::None);
                        return;
                    }
                    let at = reading.account.peek().as_ref().and_then(|id| app.accounts().iter().position(|a| &a.id == id));
                    reading.open_overlay(Overlay::AccountMenu { at: at.unwrap_or(0) });
                },
                // Lucide's rss.
                svg { width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                    path { d: "M4 11a9 9 0 0 1 9 9" }
                    path { d: "M4 4a16 16 0 0 1 16 16" }
                    circle { cx: "5", cy: "19", r: "1" }
                }
            }
            if let Some(at) = menu {
                div { class: "menu popover sidebar-popover", role: "menu", aria_label: t("accounts.menu"),
                    onclick: move |e| e.stop_propagation(),
                    for (i , account) in accounts.into_iter().enumerate() {
                        {
                            let app = Arc::clone(&menu_app);
                            let checked = active.as_deref() == Some(account.id.as_str());
                            let count = app.unread_total(&account.id).unwrap_or(0);
                            let id = account.id.clone();
                            let hover_app = Arc::clone(&app);
                            rsx! {
                                div {
                                    key: "{account.id}",
                                    class: if i == at { "menu-item highlighted" } else { "menu-item" },
                                    role: "menuitemradio",
                                    "aria-checked": "{checked}",
                                    onmouseenter: move |_| reading.run(&hover_app, Command::Highlight(i)),
                                    onclick: move |_| reading.run(&app, Command::Account(id.clone())),
                                    span { class: "check",
                                        if checked {
                                            Icon { size: 14, stroke: "2.5", paths: CHECK }
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
                        onclick: move |_| reading.run(&app_for_add, Command::AddAccount),
                        span { class: "check" }
                        span { class: "name", {t("accounts.add")} }
                    }
                }
            }
        }
    }
}

/// A folder's feeds, with their unread counts, under its row. They slide open and
/// closed: the box's height goes from 0 to its rows', each `row` high, or back, and
/// closing, the rows stay in it until the slide ends.
#[component]
fn FolderFeeds(feeds: Vec<(skvaller_core::Feed, i64)>, expanded: bool, row: f64) -> Element {
    let shown = use_closing(expanded, FOLDER_SLIDE);
    // Whether the folder last opened or closed, and when: only that slides the box, not
    // a sync adding or removing a feed, which moves the rows and highlight at once.
    let last = use_hook(|| Rc::new(std::cell::Cell::new((expanded, None::<Instant>))));
    let (was, mut toggled) = last.get();
    if was != expanded {
        toggled = Some(Instant::now());
        last.set((expanded, toggled));
    }
    let class = if toggled.is_some_and(|t| t.elapsed() < FOLDER_SLIDE) {
        "folder-feeds sliding"
    } else {
        "folder-feeds"
    };
    let rows = if expanded { feeds.len() } else { 0 };
    let height = f64::from(u32::try_from(rows).unwrap_or(u32::MAX)) * row;
    let feeds = if shown { feeds } else { Vec::new() };
    rsx! {
        div { class, style: "height: {height}px",
            for (feed, count) in feeds {
                FeedItem { key: "{feed.id}", count, feed, nested: true }
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

/// How far toward `target` the list `el` can scroll once its rows are laid out.
///
/// This runs before Blitz lays out the rows' new heights, and a scroll past the old
/// range would stop at its end: it waits for the range, a few frames at most, and else
/// returns as far as it reaches.
async fn reachable(el: &MountedData, target: f64) -> f64 {
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

/// Where the sidebar's row `index` is, counted as [`sidebar_scopes`] lists them: all
/// feeds, then after the Feeds heading each folder and feed shown, at `font_size`.
fn sidebar_place(index: usize, font_size: FontSize) -> Place {
    let rows = f64::from(u32::try_from(index).unwrap_or(u32::MAX));
    let row = scale::sidebar_row(font_size);
    let heading = if index > 0 {
        scale::heading_height(font_size) + SIDEBAR_HEADING_MARGINS
    } else {
        0.0
    };
    Place {
        top: SIDEBAR_TOP + heading + rows * row,
        height: row,
    }
}

/// A folder or feed in the sidebar, or every feed. A folder, with `expanded`, has a
/// disclosure button that shows or hides its feeds.
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
    // Not every overlay change: each row re-renders only when renaming starts or ends.
    let renamed = use_memo(move || match &*reading.overlay.read() {
        Overlay::Manage(Manage::Rename(target)) => Some(target.clone()),
        _ => None,
    });
    let renaming = target
        .clone()
        .filter(|t| renamed.read().as_ref() == Some(t));
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
                            class: if expanded { "disclosure open" } else { "disclosure" },
                            aria_label: t_with(key, &[("folder", title.as_str())]),
                            "aria-expanded": "{expanded}",
                            onclick: move |e| {
                                // Not a click on the folder: its selection stays.
                                e.stop_propagation();
                                reading.set_collapsed(&id, expanded);
                            },
                            // Turned down by app.css while open.
                            Icon { size: 16, stroke: "2", paths: CHEVRON_RIGHT }
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
    let mut list_pane = reading.list_pane;
    let unread = i64::try_from(reading.unread_count()).unwrap_or(i64::MAX);
    let selection = (reading.selection)();
    let searched = reading
        .search
        .read()
        .active()
        .map(|(_, everywhere)| everywhere);
    // A search of every feed lists the view's articles from all of them.
    let everywhere = searched == Some(true);
    let listed = if everywhere {
        Selection {
            view: selection.view,
            scope: Scope::All,
        }
    } else {
        selection.clone()
    };
    let title = list_title(&(reading.library)(), &listed);
    // The results' count, not the feed's unread one, which the title would suggest.
    let unread_label = if searched.is_some() {
        let found = reading.articles.read().rows.len();
        t_count("list.results", i64::try_from(found).unwrap_or(i64::MAX))
    } else if unread == 0 {
        t("list.noUnread")
    } else {
        t_count("list.unread", unread)
    };
    let key = list_key(&reading);
    let ghost = (reading.ghost)();
    let body_class = if ghost.is_some() {
        "list-body sliding"
    } else {
        "list-body"
    };
    let confirming = use_memo(move || *reading.overlay.read() == Overlay::MarkAll)() && unread > 0;
    // Shared by the list's viewports, one for each selection, so each character is
    // shaped, and each headline cut, once.
    use_context_provider(|| Rc::new(RefCell::new(Lines::new(fonts::font_context()))));
    let sidebar_open = use_memo(move || reading.layout.read().sidebar_open)();
    let list_width = use_memo(move || reading.layout.read().list_width)();
    let active = use_memo(move || reading.active_pane())() == Pane::List;
    let confirm_app = Arc::clone(&app);
    let cancel_app = Arc::clone(&app);
    let search_app = Arc::clone(&app);
    let toggle_app = Arc::clone(&app);
    let toggle_label = if sidebar_open {
        t("list.hideSidebar")
    } else {
        t("list.showSidebar")
    };
    let searching = reading.search.read().open;
    let search_key = if COMMAND_MODIFIER == Modifiers::META {
        "⌘F".to_owned()
    } else {
        format!("{}+F", t("keys.ctrl"))
    };

    rsx! {
        section {
            class: "list",
            class: if active { "active" },
            // Without the sidebar, the toolbar makes room for macOS's window buttons.
            class: if MAC && !sidebar_open { "no-sidebar" },
            aria_label: t("list.label"),
            style: "width: {list_width}px",
            onmounted: move |e: MountedEvent| list_pane.set(Some(e.data())),
            div { class: "toolbar",
                DragArea {}
                button {
                    class: "icon-button",
                    aria_label: toggle_label.clone(),
                    title: toggle_label,
                    "aria-pressed": "{sidebar_open}",
                    onclick: move |_| reading.run(&toggle_app, Command::ToggleSidebar),
                    Icon { size: 18, stroke: "2", paths: PANEL_LEFT }
                }
                div { class: "list-title",
                    h1 { "{title}" }
                    p { "{unread_label}" }
                }
                button {
                    class: "icon-button",
                    aria_label: t("list.search"),
                    title: t_with("list.searchWithKey", &[("key", search_key.as_str())]),
                    onclick: move |_| reading.run(&search_app, Command::Search),
                    Icon { size: 18, stroke: "2", paths: SEARCH }
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
                        onclick: move |_| reading.run(&app, Action::MarkAllRead.into()),
                        svg { width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                            circle { cx: "12", cy: "12", r: "10" }
                            path { d: "m9 12 2 2 4-4" }
                        }
                    }
                    if confirming {
                        div { class: "confirm popover", role: "dialog", "aria-labelledby": "mark-all-question",
                            p { id: "mark-all-question", {t_count("list.markAllQuestion", unread)} }
                            div { class: "actions",
                                button { class: "button", onclick: move |_| reading.run(&cancel_app, Command::CloseOverlay), {t("common.cancel")} }
                                button { class: "button primary", onclick: move |_| reading.run(&confirm_app, Command::Accept), {t("list.markAsRead")} }
                            }
                        }
                    }
                }
            }
            if searching {
                SearchBar {}
            }
            div { class: body_class,
                for key in std::iter::once(key) {
                    Viewport { key: "{key}" }
                }
                for Ghost { list, drawn } in ghost {
                    div { key: "{list}", class: "ghost", "aria-hidden": "true",
                        div { class: "spacer", style: "transform: translateY({-drawn.top}px)",
                            if let Some(place) = drawn.highlight {
                                div { class: "highlight", style: "transform: translateY({place.top}px); height: {place.height}px" }
                            }
                            for row in &drawn.rows {
                                div {
                                    key: "{row.id}",
                                    class: "{row.class}",
                                    style: "top: {row.place.top}px; height: {row.place.height}px",
                                    {row_content(row, drawn.beside, drawn.below)}
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The search field over the article list, and the choice to search every feed when
/// the sidebar has one feed or folder selected.
#[component]
fn SearchBar() -> Element {
    let app = use_context::<Arc<App>>();
    let mut reading = use_context::<Reading>();
    let search = (reading.search)();
    let scoped = (reading.selection)().scope != Scope::All;

    rsx! {
        div { class: "search-bar", role: "search",
            input {
                class: "search-field",
                aria_label: t("list.search"),
                placeholder: t("list.search"),
                value: "{search.query}",
                spellcheck: "false",
                onmounted: move |e: MountedEvent| async move {
                    reading.search_field.set(Some(e.data()));
                    if let Err(err) = e.data().set_focus(true).await {
                        tracing::warn!("focusing the search field failed: {err}");
                    }
                },
                oninput: move |e| reading.set_query(e.value()),
                onkeydown: move |e: Event<KeyboardData>| {
                    if e.modifiers().intersects(COMMAND_MODIFIER | ACCOUNT_MODIFIER) {
                        return;
                    }
                    // Other keys typed here are the field's, not the window's.
                    e.stop_propagation();
                    match e.key() {
                        Key::Named(NamedKey::Escape) => reading.close_search(),
                        Key::Named(NamedKey::Enter | NamedKey::ArrowDown) => {
                            e.prevent_default();
                            reading.leave_search_field(&app);
                        }
                        _ => {}
                    }
                },
            }
            if scoped {
                button {
                    class: "search-everywhere",
                    "aria-pressed": "{search.everywhere}",
                    onclick: move |_| {
                        reading.toggle_search_everywhere();
                        // Back to the field, so typing goes on there.
                        reading.open_search();
                    },
                    {t("list.searchEverywhere")}
                }
            }
            button {
                class: "icon-button",
                aria_label: t("list.closeSearch"),
                title: t("list.closeSearch"),
                onclick: move |_| reading.close_search(),
                Icon { size: 16, stroke: "2", paths: X }
            }
        }
    }
}

/// What the article list lists, which keys its viewport. A new list starts at the top: a
/// new selection or account gets a new viewport, with its own scroll state.
fn list_key(reading: &Reading) -> String {
    format!(
        "{:?}/{:?}/{:?}",
        reading.account.read(),
        reading.selection.read(),
        reading.search.read().active()
    )
}

/// A row's thumbnail, meta line and headline, in the list or its ghost.
fn row_content(row: &Row, beside: Option<&'static str>, below: Option<f64>) -> Element {
    let thumb_below = below.zip(row.thumb.clone());
    let text_class = if thumb_below.is_some() {
        "text below"
    } else {
        "text"
    };
    rsx! {
        if let (Some(class), Some(src)) = (beside, row.thumb.clone()) {
            img { class, src, alt: "" }
        }
        div { class: text_class,
            div { class: "meta",
                span { class: "feed", "{row.feed}" }
                if row.starred {
                    svg { class: "star", width: "11", height: "11", view_box: "0 0 24 24", fill: "currentColor",
                        path { d: STAR_PATH }
                    }
                }
                span { "{row.date}" }
            }
            div { class: "title", style: "max-height: {row.title_height}px", "{row.title}" }
            if let Some((height, src)) = thumb_below {
                img { class: "thumb large", style: "height: {height}px", src, alt: "" }
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
    let settle = use_hook(|| Rc::new(RefCell::new(Settle::<String>::new())));
    let cutter = use_context::<Rc<RefCell<Lines>>>();
    // The open article's row, found again only when the article or the list changes, not
    // at every scroll.
    let selected_row = use_memo(move || {
        let article = reading.article.read();
        let id = &article.as_ref()?.id;
        reading
            .articles
            .read()
            .rows
            .iter()
            .position(|a| &a.id == id)
    });
    // What sizes the rows, apart from the rows themselves.
    let size = use_memo(move || live.settings.read().thumbnail_size);
    let font = use_memo(move || live.settings.read().ui_font);
    let font_size = use_memo(move || live.settings.read().font_size);
    let shown_width = use_memo(move || reading.list_shown_width());
    // Each row's headline cut where it may wrap: again only for new rows or a new font
    // or size.
    let headlines = use_memo(move || {
        let stack = fonts::stack(font());
        let px = scale::headline_px(font_size());
        let rows = &reading.articles.read().rows;
        cutter
            .borrow_mut()
            .headlines(&stack, px, rows.iter().map(|a| a.title.as_str()))
    });
    // How many lines each row's headline takes, and where each row starts, at the width
    // the list shows, as during a drag.
    let placed = use_memo(move || {
        let (size, width, font_size) = (size(), shown_width(), font_size());
        let headlines = headlines();
        let rows = &reading.articles.read().rows;
        debug_assert_eq!(rows.len(), headlines.len(), "headlines cut from other rows");
        let counts: Rc<[u32]> = rows
            .iter()
            .zip(headlines.iter())
            .map(|(a, h)| h.lines(title_width(size, width, a.thumbnail), TITLE_LINES))
            .collect();
        let offsets = row_offsets(rows, size, font_size, width, &counts);
        (Rc::<[f64]>::from(offsets), counts)
    });
    let mut keeper = use_hook(|| CopyValue::new(Keeper::default()));
    // The list's scroll to where it holds, while it waits for the layout, and where to.
    let mut pending = use_hook(|| CopyValue::new(None::<(f64, dioxus_core::Task)>));
    let mut top = use_signal(|| 0.0_f64);
    let mut mounted = use_signal(|| None::<Rc<MountedData>>);
    let pull = use_hook(|| {
        Pull::new(
            &live,
            mounted,
            Some(&ListPull {
                top,
                app: CopyValue::new(Arc::clone(&app)),
                reading,
            }),
        )
    });
    let mut scrolled = pull.at;
    let waiting = move || pending.peek().as_ref().map(|&(target, _)| target);

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
            let height = viewport_height(reading.window_size.peek().1);
            // Found now, not when the effect ran: a reload meanwhile may have moved the
            // row, or taken it away.
            let target = {
                let (list, placed) = (reading.articles.peek(), placed.peek());
                reveal_row(&placed.0, &list.rows, &id, *top.peek(), waiting(), height)
            };
            // A list just mounted, as search results with the article open, isn't laid
            // out yet: the scroll would stop short while the rows drawn followed `top`.
            // It waits as a hold, which the user's scroll or a newer one replaces.
            if let Some(target) = target {
                scroll_list(el, target, pending, top, "scrolling the list");
            }
        });
    });

    let size = size();
    let font_size = font_size();
    let width = shown_width();
    let listed = reading.articles.read();
    let list = &listed.rows;
    let (offsets, counts) = placed();
    let total = offsets[list.len()];
    let height = viewport_height((reading.window_size)().1);
    let selected = reading.article.read().as_ref().map(|a| a.id.clone());
    let place = keeper.write().place(&ListView {
        offsets: &offsets,
        rows: list,
        top: top(),
        waiting: waiting(),
        height,
        selected: selected_row(),
    });
    scrolled.set(Scrolled {
        top: *top.peek(),
        end: place.end,
        height,
    });
    let view_top = place.top;
    let shown = place.shown;
    // `top` stays where the viewport is until the scroll: the renders until then draw
    // the rows there too. Whatever else scrolls the list cancels this.
    if let Some(target) = place.scroll
        && let Some(el) = mounted.peek().clone()
    {
        scroll_list(el, target, pending, top, "keeping the list in place");
    }
    let account = reading.account.read().clone().unwrap_or_default();
    let (beside, below, fit) = match size {
        ThumbnailSize::Small => (Some("thumb small"), None, thumbs::Fit::Square),
        ThumbnailSize::Medium => (Some("thumb medium"), None, thumbs::Fit::Square),
        // The box follows the width shown; the image is served for the saved one, so
        // a drag loads none.
        ThumbnailSize::Large => (
            None,
            Some(large_height(width)),
            thumbs::Fit::wide((reading.saved_list_width)() - 2.0 * PAD_X),
        ),
        ThumbnailSize::None => (None, None, thumbs::Fit::Square),
    };
    let highlight = selected.as_ref().zip(selected_row()).map(|(id, i)| {
        let place = Place {
            top: offsets[i],
            height: offsets[i + 1] - offsets[i],
        };
        let settles = settle.borrow_mut().settles(id.clone(), place.top);
        (highlight_class(settles), place)
    });
    let empty = if reading.search.read().active().is_some() {
        t("list.noMatches")
    } else {
        t(&format!(
            "views.{}.empty",
            view_key(reading.selection.read().view)
        ))
    };
    let lib = reading.library.read();
    let feed_title = |id: &str| {
        lib.feeds
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.title.clone())
            .unwrap_or_default()
    };
    let rows: Vec<Row> = shown
        .iter()
        .map(|&i| {
            let a = &list[i];
            let mut class = String::from("row");
            if !reading.is_read(a) {
                class.push_str(" unread");
            }
            if selected.as_deref() == Some(a.id.as_str()) {
                class.push_str(" selected");
            }
            Row {
                id: a.id.clone(),
                class,
                place: Place {
                    top: offsets[i],
                    height: offsets[i + 1] - offsets[i],
                },
                feed: feed_title(&a.feed_id),
                starred: reading.is_starred(a),
                date: dates.list(a.published),
                title: a.title.clone(),
                title_height: f64::from(counts[i]) * title_line(font_size),
                thumb: a.thumbnail.then(|| thumbs::url(&account, &a.id, fit)),
            }
        })
        .collect();
    let drawn = Rc::new(Drawn {
        rows,
        top: view_top,
        highlight: highlight.map(|(_, place)| place),
        beside,
        below,
    });
    // Kept for mark-all-read, which slides away the list as drawn.
    reading.drawn.set(Some(Rc::clone(&drawn)));
    let pulled = (pull.shown)();
    let spring = spring_style(pulled);
    let stretched = stretch_style(pulled);
    let opacity = if pulled.armed() {
        1.0
    } else {
        pulled.progress()
    };
    let icon_y = (pulled.offset - HOLD) / 2.0;
    let turn = pulled.progress() * 360.0;

    rsx! {
        // Shows in the gap above a list pulled down, centered in it. Decorative: the
        // sidebar's sync button says the same.
        if pulled.indicator() {
            div {
                class: "refresh",
                class: if pulled.armed() { "armed" },
                class: if pulled.refreshing { "spinning" },
                "aria-hidden": "true",
                style: "height: {HOLD}px; opacity: {opacity}; transform: translateY({icon_y}px); {spring}",
                // A full turn when armed, so the spin starts where the pull left it.
                span {
                    class: "turn",
                    style: if pulled.refreshing { "" } else { "transform: rotate({turn}deg); {spring}" },
                    Icon { size: 18, stroke: "2", paths: ROTATE_CW }
                }
            }
        }
        div {
            class: "viewport",
            style: "{stretched}",
            onmounted: move |e: MountedEvent| {
                let el = e.data();
                mounted.set(Some(Rc::clone(&el)));
                // Back from a search: where this list was before it.
                let back = reading
                    .restore
                    .peek()
                    .as_ref()
                    .is_some_and(|r| r.list == list_key(&reading));
                if back && let Some(restore) = reading.restore.take() {
                    let target = {
                        let (list, placed) = (reading.articles.peek(), placed.peek());
                        restore.target(&list.rows, &placed.0)
                    };
                    // A hold, as the list's own: bringing the open article into view
                    // takes its place.
                    scroll_list(el, target, pending, top, "scrolling the list back");
                }
            },
            onwheel: move |e| pull.on_wheel(&e),
            ontouchstart: move |e| pull.on_touch_start(&e),
            ontouchmove: move |e| pull.on_touch_move(&e),
            ontouchend: move |e| pull.on_touch_end(&e),
            ontouchcancel: move |e| pull.on_touch_cancel(&e),
            onscroll: move |e: Event<ScrollData>| {
                // Setting a signal re-renders even to the same value; a scroll event
                // that follows every layout would loop.
                let t = e.data().scroll_top();
                scrolled.write().top = t;
                if (*top.peek() - t).abs() >= 1.0 {
                    top.set(t);
                    // The user's scroll: the list is where they took it.
                    if let Some((_, task)) = pending.take() {
                        task.cancel();
                    }
                }
            },
            if list.is_empty() {
                p { class: "empty", "{empty}" }
            }
            div { class: "spacer", style: "height: {total}px",
                if let Some((class, place)) = highlight {
                    div { class, style: "transform: translateY({place.top}px); height: {place.height}px" }
                }
                for (row , &i) in drawn.rows.iter().zip(&shown) {
                    {
                        let summary = list[i].clone();
                        let app = Arc::clone(&app);
                        rsx! {
                            div {
                                key: "{row.id}",
                                class: "{row.class}",
                                style: "top: {row.place.top}px; height: {row.place.height}px",
                                onclick: move |_| {
                                    reading.pane.set(Pane::List);
                                    reading.open(&app, summary.clone());
                                },
                                {row_content(row, beside, below)}
                            }
                        }
                    }
                }
            }
        }
    }
}

/// While a list springs, CSS moves it, and the refresh icon along.
fn spring_style(pulled: Shown) -> String {
    pulled.spring.map_or(String::new(), |s| {
        let ms = s.duration.as_millis();
        format!("transition: transform {ms}ms {SPRING_EASE}, opacity {ms}ms {SPRING_EASE};")
    })
}

/// A list's style as it is stretched.
fn stretch_style(pulled: Shown) -> String {
    if pulled.offset == 0.0 && pulled.spring.is_none() {
        String::new()
    } else {
        format!(
            "transform: translateY({}px); {}",
            pulled.offset,
            spring_style(pulled)
        )
    }
}

/// Where a scrolling list is.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Scrolled {
    /// Where it is scrolled.
    top: f64,
    /// The furthest it scrolls.
    end: f64,
    /// How tall it shows.
    height: f64,
}

/// What pulling the article list acts on besides the list: its rows follow its scroll,
/// and a pull refreshes.
#[derive(Clone, Copy, PartialEq)]
struct ListPull {
    /// Where the list is scrolled, which its rows are drawn from.
    top: Signal<f64>,
    app: CopyValue<Arc<App>>,
    reading: Reading,
}

/// A list's [`Overscroll`], with what it acts on.
#[derive(Clone, Copy, PartialEq)]
struct Pull {
    state: CopyValue<Overscroll>,
    /// What the list draws of it.
    shown: Signal<Shown>,
    /// Where the scrolling element is, as last seen.
    at: CopyValue<Scrolled>,
    /// The scrolling element.
    mounted: Signal<Option<Rc<MountedData>>>,
    live: Live,
    /// The article list's, which refreshes; `None` for the sidebar's feeds.
    list: Option<ListPull>,
}

impl Pull {
    fn new(live: &Live, mounted: Signal<Option<Rc<MountedData>>>, list: Option<&ListPull>) -> Self {
        let animate = live.settings.peek().animations;
        Self {
            state: CopyValue::new(Overscroll::new(animate, list.is_some())),
            shown: Signal::new(Shown::default()),
            // Until measured, at the top of a long list.
            at: CopyValue::new(Scrolled {
                top: 0.0,
                end: f64::INFINITY,
                height: 600.0,
            }),
            mounted,
            live: *live,
            list: list.copied(),
        }
    }

    /// Where the list is scrolled now.
    fn edges(self) -> Edges {
        let at = *self.at.peek();
        Edges {
            top: at.top < 1.0,
            bottom: at.top >= at.end - END_SLACK,
            height: at.height,
        }
    }

    /// Reads where the scrolling element is, for a list that doesn't know it itself.
    async fn measure(mut self) {
        let Some(el) = self.mounted.peek().clone() else {
            return;
        };
        if let (Ok(view), Ok(offset), Ok(range)) = (
            el.get_client_rect().await,
            el.get_scroll_offset().await,
            el.get_scroll_size().await,
        ) && view.height() > 0.0
        {
            self.at.set(Scrolled {
                top: offset.y,
                end: range.height,
                height: view.height(),
            });
        }
    }

    fn on_wheel(self, e: &Event<WheelData>) {
        let (pixels, (dx, dy)) = match e.delta() {
            WheelDelta::Pixels(d) => (true, (d.x, d.y)),
            WheelDelta::Lines(d) => (false, (d.x, d.y)),
            WheelDelta::Pages(d) => (false, (d.x, d.y)),
        };
        // Dioxus's wheel event has no phase or source: Blitz's has.
        let (phase, source) = e
            .data()
            .downcast::<NativeWheelData>()
            .map_or((BlitzWheelPhase::Moved, BlitzScrollSource::Unknown), |d| {
                (d.phase(), d.source())
            });
        // A wheel that turns in lines is a wheel, whatever the platform says.
        let source = if pixels {
            source
        } else {
            BlitzScrollSource::Wheel
        };
        let zoom = e.modifiers().ctrl();
        if self.run(|o, now, edges| o.wheel(now, (dx, dy), zoom, (phase, source), edges)) {
            e.prevent_default();
        }
    }

    fn on_touch_start(self, e: &Event<TouchData>) {
        let y = only_touch(e);
        self.run(|o, _, edges| o.touch_start(y, edges));
    }

    fn on_touch_move(self, e: &Event<TouchData>) {
        let y = only_touch(e);
        if self.run(|o, now, edges| o.touch_move(now, y, edges)) {
            e.prevent_default();
        }
    }

    fn on_touch_end(self, e: &Event<TouchData>) {
        let left = e.touches().len();
        self.run(|o, now, edges| o.touch_end(now, left, edges));
    }

    fn on_touch_cancel(self, e: &Event<TouchData>) {
        let left = e.touches().len();
        self.run(|o, now, edges| o.touch_cancel(now, left, edges));
    }

    /// Feeds an event to the overscroll with `f` and does what it says. Returns whether
    /// to keep the event from scrolling the list.
    fn run(mut self, f: impl FnOnce(&mut Overscroll, Instant, Edges) -> Out) -> bool {
        let edges = self.edges();
        let (out, shown) = {
            let mut state = self.state.write();
            state.animate = self.live.settings.peek().animations;
            let out = f(&mut state, Instant::now(), edges);
            (out, state.shown())
        };
        if *self.shown.peek() != shown {
            self.shown.set(shown);
        }
        self.act(out)
    }

    /// Does what the overscroll said, apart from drawing it: not generic, as the tasks
    /// it starts feed it again.
    fn act(mut self, out: Out) -> bool {
        if let Some(wheel) = out.idle {
            spawn(async move {
                tokio::time::sleep(WHEEL_IDLE).await;
                self.run(|o, now, edges| o.idle(now, wheel, edges));
            });
        }
        if let Some((id, duration)) = out.spring {
            spawn(async move {
                tokio::time::sleep(duration).await;
                self.run(|o, now, _| o.sprung(now, id));
            });
        }
        if out.refresh
            && let Some(list) = self.list
        {
            spawn(async move {
                list.refresh().await;
                self.run(|o, now, _| o.refreshed(now));
            });
        }
        if out.scroll_by != 0.0
            && let Some(el) = self.mounted.peek().clone()
        {
            // Code-driven scrolls send no scroll event: the list follows here.
            let at = *self.at.peek();
            let to = (at.top + out.scroll_by).clamp(0.0, at.end);
            self.at.write().top = to;
            if let Some(mut list) = self.list {
                list.top.set(to);
            }
            spawn(async move {
                let to = dioxus_html::geometry::PixelsVector2D::new(0.0, to);
                if let Err(e) = el.scroll(to, ScrollBehavior::Instant).await {
                    tracing::warn!("scrolling the list failed: {e}");
                }
            });
        }
        out.prevent
    }
}

impl ListPull {
    /// Syncs the shown account and waits until no sync of it runs: one that was already
    /// running ends the wait when it does.
    async fn refresh(self) {
        let app = self.app.peek().clone();
        if let Some(synced) = self.reading.sync(&app) {
            synced.await;
        }
    }
}

/// Where a touch is, if it's the only one.
fn only_touch(e: &Event<TouchData>) -> Option<f64> {
    match e.touches().as_slice() {
        [touch] => Some(touch.client_coordinates().y),
        _ => None,
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
    // the top, so a new look keeps the article's place (`Remake::keep_place`).
    let look = use_memo(move || article::Look::new(&live.settings.read(), (live.theme)()));
    let remake = use_hook(|| Remake {
        look: Signal::new(*look.peek()),
        count: Signal::new(0),
        article: Signal::new(None),
        kept: CopyValue::new(None),
        frame: Signal::new(None),
        hidden: Signal::new(false),
    });
    use_effect(move || {
        let next = look();
        if next != *remake.look.peek() {
            spawn(remake.keep_place(reading.frame, reading.article, look, next));
        }
    });
    // Another article opening forgets the one remade: it fades in, and so does the remade
    // one opened again.
    use_effect(move || {
        let id = reading.article.read().as_ref().map(|a| a.id.clone());
        let mut remade = remake.article;
        if *remade.peek() != id && remade.peek().is_some() {
            remade.set(None);
        }
    });
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
            let mut look = (remake.look)();
            if (remake.article).read().as_ref() == Some(&a.id) {
                look.animations = false;
            }
            Some(article::document(&a, feed, &html, &date, look))
        }
    });
    let open = reading.open_article();
    // The sidebar counts until its closing slide ends, as the reader widens only then.
    let sidebar_open = use_memo(move || reading.layout.read().sidebar_open)();
    let sidebar_shown = use_closing(sidebar_open, SIDEBAR_SLIDE);
    // A memo, so a resize re-renders the reader only when this changes.
    let buttons_fit = use_memo(use_reactive!(|sidebar_shown| {
        // Off Linux they always fit, without following the window's size.
        if !LINUX {
            return true;
        }
        let layout = Layout {
            sidebar_open: sidebar_shown,
            ..*reading.layout.read()
        };
        reader_width(&layout, (reading.window_size)().0) >= READER_WITH_BUTTONS
    }))();
    // Reader View on its way, just after opening: no feed content to swap out yet.
    let holding = {
        let rv = reading.reader_view.read();
        rv.on && rv.fetch == (Fetch::Loading { awaiting: true })
    };

    rsx! {
        section { class: "reader", aria_label: t("reader.label"),
            div { class: "toolbar",
                DragArea {}
                if let Some(a) = open.filter(|_| buttons_fit) {
                    ReaderButtons { article: a }
                }
                MainWindowButtons {}
            }
            match doc() {
                Some(_) if holding => rsx! {},
                Some(doc) => rsx! {
                    iframe {
                        // Remade for new content, which then loads from the top, and for
                        // a new look, which keeps its place.
                        key: "{full_text.read().is_some()}-{doc.len()}-{remake.count}",
                        title: "",
                        style: if (remake.hidden)() { "visibility: hidden" },
                        "srcdoc": doc,
                        onmounted: {
                            let (made, mut mounted) = (*remake.count.peek(), remake.frame);
                            move |e: MountedEvent| {
                                reading.frame.set(Some(e.data()));
                                mounted.set(Some((made, e.data())));
                            }
                        },
                    }
                },
                None => rsx! { p { class: "empty", {t("reader.empty")} } },
            }
        }
    }
}

/// How long a new look waits at most for the remade article's layout to put it back in
/// its place: a long article takes a while to lay out.
const PLACE_WAITS: u32 = 60;

/// The reader's document as a new look remakes it.
#[derive(Clone, Copy)]
struct Remake {
    /// The look the document is made in.
    look: Signal<article::Look>,
    /// Bumped for each look: it keys the frame, so the remade one mounts anew.
    count: Signal<u64>,
    /// The article whose remade frame is to be put back at a place, and the place, while
    /// it waits for its layout.
    kept: CopyValue<Option<(String, f64)>>,
    /// The frame last mounted, with the `count` it was made at: the one to put back is
    /// the one made for its look, not one a newer look is already replacing.
    frame: Signal<Option<(u64, Rc<MountedData>)>>,
    /// The remade frame hides until it is in its place, so it doesn't show the top, nor
    /// its fade in, first.
    hidden: Signal<bool>,
    /// The article the document was last remade for: remade, it doesn't fade in again
    /// as an article opening does.
    article: Signal<Option<String>>,
}

impl Remake {
    /// Shows the article in the look `next`, the newest of `look`, at the place it is
    /// scrolled to now in `frame`, while `article` stays open.
    ///
    /// A new look remakes the reader's document, and the frame loads it from the top. So
    /// the place, as a share of how far the frame scrolls, is read first, and the remade
    /// frame scrolls there as it lays out, until its height holds still. A look changed
    /// again meanwhile, as by ⌘+ held down, keeps the place `kept` is waiting to
    /// restore rather than reading it from a frame still loading.
    async fn keep_place(
        mut self,
        frame: Signal<Option<Rc<MountedData>>>,
        article: Signal<Option<ArticleSummary>>,
        look: Memo<article::Look>,
        next: article::Look,
    ) {
        let Some(id) = article.peek().as_ref().map(|a| a.id.clone()) else {
            self.look.set(next);
            return;
        };
        let kept = self.kept.peek().clone().filter(|(kept, _)| *kept == id);
        let old = frame.peek().clone();
        let place = match (kept, &old) {
            (Some((_, place)), _) => Some(place),
            (None, Some(frame)) => match (
                frame.get_scroll_offset().await,
                frame.get_scroll_size().await,
            ) {
                (Ok(offset), Ok(range)) if range.height > 0.0 => Some(offset.y / range.height),
                _ => None,
            },
            (None, None) => None,
        };
        // A newer look came meanwhile: its own call shows it.
        if *look.peek() != next {
            return;
        }
        let place = place.filter(|&p| p > 0.0);
        self.kept.set(place.map(|p| (id.clone(), p)));
        self.hidden.set(place.is_some());
        self.look.set(next);
        self.count += 1;
        if article.peek().as_ref().map(|a| &a.id) == Some(&id) {
            self.article.set(Some(id.clone()));
        }
        let made = *self.count.peek();
        if let Some(place) = place {
            let mut last = None;
            // Frames the new frame has had no scroll range: at a smaller size it may no
            // longer scroll at all.
            let mut unscrolled = 0;
            for _ in 0..PLACE_WAITS {
                tokio::time::sleep(FRAME).await;
                // A newer look's call has the place and the frame now.
                if *self.count.peek() != made {
                    return;
                }
                if article.peek().as_ref().map(|a| &a.id) != Some(&id) {
                    break;
                }
                let now = self.frame.peek().clone();
                let Some((_, now)) = now.filter(|&(at, _)| at == made) else {
                    continue;
                };
                let Ok(range) = now.get_scroll_size().await else {
                    break;
                };
                if range.height <= 0.0 {
                    unscrolled += 1;
                    if unscrolled >= LAYOUT_WAITS {
                        break;
                    }
                    continue;
                }
                let to = dioxus_html::geometry::PixelsVector2D::new(0.0, place * range.height);
                if let Err(e) = now.scroll(to, ScrollBehavior::Instant).await {
                    tracing::warn!("keeping the article's place failed: {e}");
                    break;
                }
                if last == Some(range.height) {
                    break;
                }
                last = Some(range.height);
            }
        }
        if *self.count.peek() == made {
            self.kept.set(None);
            self.hidden.set(false);
        }
    }
}

/// Which of the reader's toolbar icons pop: the unread mark or the star, when it turns
/// on for the article shown, not when an article opens with it on.
#[derive(Debug, Default)]
struct Pops {
    id: String,
    unread: Pop,
    star: Pop,
}

/// One icon of [`Pops`]: whether it is on, and whether it popped turning on.
#[derive(Clone, Copy, Debug, Default)]
struct Pop {
    on: bool,
    pops: bool,
}

impl Pop {
    /// Takes in `on` for the same article: an icon pops when it turns on, and stays
    /// popped until it turns off, so a re-render doesn't replay it.
    fn update(&mut self, on: bool) -> bool {
        if on != self.on {
            self.pops = on;
        }
        self.on = on;
        self.pops
    }
}

impl Pops {
    /// Takes in what the toolbar shows now, and returns whether the unread mark and the
    /// star pop.
    fn update(&mut self, id: &str, read: bool, starred: bool) -> (bool, bool) {
        if self.id != id {
            id.clone_into(&mut self.id);
            self.unread = Pop {
                on: !read,
                pops: false,
            };
            self.star = Pop {
                on: starred,
                pops: false,
            };
        }
        (self.unread.update(!read), self.star.update(starred))
    }
}

/// The open article's read, star and browser buttons.
#[component]
fn ReaderButtons(article: ArticleSummary) -> Element {
    let app = use_context::<Arc<App>>();
    let reading = use_context::<Reading>();
    let read = reading.is_read(&article);
    let starred = reading.is_starred(&article);
    let pops = use_hook(|| Rc::new(RefCell::new(Pops::default())));
    let (unread_pops, star_pops) = pops.borrow_mut().update(&article.id, read, starred);
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
    let browser_app = Arc::clone(&app);
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
            class: match (read, unread_pops) {
                (true, _) => "icon-button",
                (false, false) => "icon-button unread",
                (false, true) => "icon-button unread pop",
            },
            aria_label: read_label.clone(),
            title: read_label,
            onclick: move |_| reading.run(&app, Action::ToggleRead.into()),
            // The unread mark: filled while unread, outlined once read.
            svg { width: "18", height: "18", view_box: "0 0 24 24", fill: if read { "none" } else { "currentColor" }, stroke: "currentColor", stroke_width: "1.6",
                circle { cx: "12", cy: "12", r: "7.5" }
            }
        }
        button {
            class: match (starred, star_pops) {
                (false, _) => "icon-button",
                (true, false) => "icon-button starred",
                (true, true) => "icon-button starred pop",
            },
            aria_label: star_label.clone(),
            title: star_label,
            onclick: move |_| reading.run(&star_app, Action::ToggleStar.into()),
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
            onclick: move |_| reading.run(&reader_app, Action::ToggleReaderView.into()),
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
            onclick: move |_| reading.run(&browser_app, Action::OpenInBrowser.into()),
            svg { width: "18", height: "18", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
                circle { cx: "12", cy: "12", r: "10" }
                path { d: "M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20" }
                path { d: "M2 12h20" }
            }
        }
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
        let entries = sidebar_tree(
            &lib,
            View::All,
            &HashSet::new(),
            &Scope::All,
            SortSubscriptions::FoldersFirst,
        );
        assert_eq!(
            sidebar_scopes(&entries, &HashSet::new()),
            [
                Scope::All,
                folder_scope("d"),
                feed_scope("inner"),
                feed_scope("loose")
            ]
        );
        // A collapsed folder's feeds have no rows.
        assert_eq!(
            sidebar_scopes(&entries, &HashSet::from(["d".to_owned()])),
            [Scope::All, folder_scope("d"), feed_scope("loose")]
        );
    }

    #[test]
    fn unread_and_starred_hide_feeds_without_articles() {
        let mut lib = Library {
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
        // "b" was read to zero but is still pinned; "y" has articles in both views, and
        // "x" only unread ones.
        let pinned = HashSet::from(["b".to_owned()]);
        lib.counts.unread_by_feed.insert("x".into(), 2);
        lib.counts.unread_by_feed.insert("y".into(), 3);
        lib.counts.starred_by_feed.insert("y".into(), 1);
        let scopes = |view, selected: &Scope| {
            sidebar_scopes(
                &sidebar_tree(
                    &lib,
                    view,
                    &pinned,
                    selected,
                    SortSubscriptions::FoldersFirst,
                ),
                &HashSet::new(),
            )
        };
        assert_eq!(
            scopes(View::Unread, &Scope::All),
            [
                Scope::All,
                folder_scope("d"),
                feed_scope("b"),
                feed_scope("x"),
                feed_scope("y")
            ]
        );
        assert_eq!(
            scopes(View::Starred, &Scope::All),
            [
                Scope::All,
                folder_scope("d"),
                feed_scope("b"),
                feed_scope("y")
            ]
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
                feed_scope("x"),
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
                feed_scope("x"),
                feed_scope("y")
            ]
        );
        assert_eq!(
            scopes(View::All, &Scope::All),
            [
                Scope::All,
                folder_scope("d"),
                feed_scope("a"),
                feed_scope("b"),
                folder_scope("e"),
                feed_scope("c"),
                folder_scope("f"),
                feed_scope("x"),
                feed_scope("y"),
            ]
        );
    }

    #[test]
    fn pinned_feeds_are_taken_again_only_when_the_key_changes() {
        let key = |view, last_sync| {
            let selection = Selection {
                view,
                scope: Scope::All,
            };
            (Some("1".to_owned()), selection, last_sync)
        };
        let ids = |set: Rc<HashSet<String>>| {
            let mut ids: Vec<_> = set.iter().cloned().collect();
            ids.sort();
            ids
        };
        let mut pinned = Pinned::default();
        let mut counts = HashMap::from([("a".to_owned(), 2), ("b".to_owned(), 1)]);
        assert_eq!(
            ids(pinned.update(key(View::Unread, None), &counts)),
            ["a", "b"]
        );
        // Read to zero, "b" stays while the key is the same.
        counts.remove("b");
        assert_eq!(
            ids(pinned.update(key(View::Unread, None), &counts)),
            ["a", "b"]
        );
        // A finished sync takes them again, as a new view or selection does.
        assert_eq!(
            ids(pinned.update(key(View::Unread, Some(5)), &counts)),
            ["a"]
        );
        let starred = ids(pinned.update(key(View::Starred, Some(5)), &HashMap::new()));
        assert_eq!(starred, Vec::<String>::new());
    }

    #[test]
    fn the_arrow_keys_step_through_the_shown_rows_only() {
        let lib = Library {
            folders: vec![folder("d")],
            feeds: vec![
                feed("a", Some("d")),
                feed("b", Some("d")),
                feed("x", None),
                feed("y", None),
            ],
            ..Default::default()
        };
        let pinned = HashSet::from(["a".to_owned(), "y".to_owned()]);
        let none = HashSet::new();
        let step = |collapsed: &HashSet<String>, from: &Scope, delta| {
            step_scope(
                &lib,
                View::Unread,
                &pinned,
                collapsed,
                from,
                delta,
                SortSubscriptions::FoldersFirst,
            )
        };
        // "x" has no articles: "a" steps over it to "y".
        assert_eq!(step(&none, &feed_scope("a"), 1), Some(feed_scope("y")));
        assert_eq!(step(&none, &feed_scope("y"), -1), Some(feed_scope("a")));
        // A selected feed shows, so it has a row to step from.
        assert_eq!(step(&none, &feed_scope("x"), 1), Some(feed_scope("y")));
        assert_eq!(step(&none, &feed_scope("b"), -1), Some(feed_scope("a")));
        // Past either end, nothing.
        assert_eq!(step(&none, &Scope::All, -1), None);
        assert_eq!(step(&none, &feed_scope("y"), 1), None);
        // From a feed in a collapsed folder, the folder's row.
        let collapsed = HashSet::from(["d".to_owned()]);
        assert_eq!(step(&collapsed, &feed_scope("a"), 1), Some(feed_scope("y")));
        assert_eq!(step(&collapsed, &feed_scope("a"), -1), Some(Scope::All));
    }

    #[test]
    fn mark_all_read_moves_on_to_the_next_row_below_with_articles() {
        let mut lib = Library {
            folders: vec![folder("d"), folder("e")],
            feeds: vec![
                feed("a", Some("d")),
                feed("b", Some("d")),
                feed("c", Some("e")),
                feed("x", None),
                feed("y", None),
            ],
            ..Default::default()
        };
        // "a" and "b" were just marked read; both are still pinned.
        lib.counts.unread_by_feed.insert("c".into(), 1);
        lib.counts.unread_by_feed.insert("y".into(), 2);
        let pinned = HashSet::from(["a", "b", "c", "x", "y"].map(String::from));
        let none = HashSet::new();
        let next = |collapsed: &HashSet<String>, from: &Scope| {
            next_with_articles(
                &lib,
                View::Unread,
                &pinned,
                collapsed,
                from,
                SortSubscriptions::FoldersFirst,
            )
        };
        // Past "b", which has none: folder "e" comes first, as its feed has some.
        assert_eq!(next(&none, &feed_scope("a")), Some(folder_scope("e")));
        assert_eq!(next(&none, &folder_scope("d")), Some(folder_scope("e")));
        // Only rows below count: "x" has none, "y" has some.
        assert_eq!(next(&none, &feed_scope("c")), Some(feed_scope("y")));
        // None left below: All.
        assert_eq!(next(&none, &feed_scope("y")), Some(Scope::All));
        let collapsed = HashSet::from(["d".to_owned()]);
        // No row to move on from.
        assert_eq!(next(&none, &Scope::All), None);
        assert_eq!(next(&collapsed, &feed_scope("a")), None);
        // A collapsed folder's feeds have no rows: the folder below is next.
        assert_eq!(
            next(&collapsed, &folder_scope("d")),
            Some(folder_scope("e"))
        );
    }

    #[test]
    fn alphabetical_mixes_folders_and_loose_feeds() {
        // As the cache lists them: each list A–Z in NOCASE order. Folder titles are
        // their ids in capitals.
        let lib = Library {
            folders: vec![folder("b"), folder("news")],
            feeds: vec![
                feed("a", None),
                feed("inner", Some("news")),
                feed("News", None),
                feed("Zed", None),
                feed("été", None),
            ],
            ..Default::default()
        };
        let sort = SortSubscriptions::Alphabetical;
        let entries = sidebar_tree(&lib, View::All, &HashSet::new(), &Scope::All, sort);
        // The folder "NEWS" and the feed "News" tie: the folder comes first. Letters
        // outside ASCII aren't folded and sort after Z, as in SQLite.
        assert_eq!(
            sidebar_scopes(&entries, &HashSet::new()),
            [
                Scope::All,
                feed_scope("a"),
                folder_scope("b"),
                folder_scope("news"),
                feed_scope("inner"),
                feed_scope("News"),
                feed_scope("Zed"),
                feed_scope("été"),
            ]
        );
        // The arrow keys follow the shown order.
        let step = |from: &Scope| {
            let none = HashSet::new();
            step_scope(&lib, View::All, &none, &none, from, 1, sort)
        };
        assert_eq!(step(&feed_scope("a")), Some(folder_scope("b")));
        assert_eq!(step(&feed_scope("inner")), Some(feed_scope("News")));
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
    fn icons_pop_when_turned_on_not_when_opened_on() {
        let mut pops = Pops::default();
        // Opened unread and starred: nothing pops.
        assert_eq!(pops.update("a", false, true), (false, false));
        // Marked read, then unread again: the mark pops, and stays popped.
        assert_eq!(pops.update("a", true, true), (false, false));
        assert_eq!(pops.update("a", false, true), (true, false));
        assert_eq!(pops.update("a", false, true), (true, false));
        // Unstarred, then starred.
        assert_eq!(pops.update("a", false, false), (true, false));
        assert_eq!(pops.update("a", false, true), (true, true));
        // Another article opens unread and starred: nothing pops.
        assert_eq!(pops.update("b", false, true), (false, false));
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
