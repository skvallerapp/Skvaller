//! The sidebar: the view and account switchers, the feeds and folders, and their
//! resizer.

use super::*;
use crate::dates::Dates;
use crate::icons::RSS;

/// What the sidebar says of the account's sync, and whether it is a failure.
pub(super) fn status_line(status: Option<&SyncStatus>, dates: &Dates) -> (String, bool) {
    match status {
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
        }) => (dates.synced(*ms), false),
        _ => (t("sidebar.notSynced"), false),
    }
}

#[component]
pub(super) fn Sidebar() -> Element {
    let app = use_context::<Arc<App>>();
    let live = use_context::<Live>();
    let dates = (live.dates)();
    let reading = use_context::<Reading>();
    // Borrowed, not cloned: the render writes no signal, and the library may hold
    // hundreds of feeds.
    let lib = reading.library.read();
    let current = (reading.selection)();
    let counts = counts_by_feed(&lib.counts, current.view);
    let all_label = t(&format!("views.{}.all", view_key(current.view)));
    let all_count: i64 = counts.values().sum();
    let count_of = |id: &str| counts.get(id).copied().unwrap_or(0);
    let status = (reading.status)();
    let spinning = use_spin(reading.status, reading.syncs_asked);
    let sync_app = Arc::clone(&app);
    let settings_app = Arc::clone(&app);
    let settings_key = chord_label(COMMAND_MODIFIER, ",");
    let status_line = status_line(status.as_ref(), &dates);
    let class = if (reading.pane)() == Pane::Sidebar {
        "sidebar active"
    } else {
        "sidebar"
    };
    // The account's name and the count of accounts come from config.json.
    (live.revision)();
    let local = is_local(&app, reading.account.read().as_deref());
    let title = reading
        .account
        .read()
        .as_deref()
        .and_then(|id| app.account(id))
        .map_or_default(|a| a.description);
    let sort = live.settings.read().sort_subscriptions;
    let font_size = use_memo(move || live.settings.read().font_size);
    // Rows mark-all-read emptied stay while they close, but only in its account and
    // view.
    let leaving = reading.leaving.read().clone().filter(|l| {
        l.view == current.view && reading.account.read().as_deref() == Some(l.account.as_str())
    });
    let pinned = reading.pinned.read().clone();
    let shown = match &leaving {
        Some(l) => Rc::new(pinned.union(&l.feeds).cloned().collect()),
        None => pinned,
    };
    let entries = sidebar_tree(&lib, current.view, &shown, &current.scope, sort);
    let feed_leaves = |id: &str| leaving.as_ref().is_some_and(|l| l.feeds.contains(id));
    let folder_leaves = |id: &str| leaving.as_ref().is_some_and(|l| l.folders.contains(id));
    let row_leaves = |scope: &Scope| match scope {
        Scope::Feed { id } => feed_leaves(id),
        Scope::Folder { id } => folder_leaves(id),
        Scope::All => false,
    };
    let sidebar_width = use_memo(move || reading.layout.read().sidebar_width)();
    let feeds_pane = use_signal(|| None::<Rc<MountedData>>);
    // Pulled past their ends, the feeds stretch, with no refresh.
    let pull = use_hook(|| Pull::new(&live, feeds_pane, None));
    // Unlike the article list, the feeds don't know their scroll range: measured again
    // when rows come or go, as when leaving rows have closed, a folder opens or closes,
    // the rows grow or shrink with the font size, or the window resizes.
    use_effect(move || {
        reading.library.read();
        reading.leaving.read();
        reading.collapsed.read();
        reading.pinned.read();
        reading.window_size.read();
        font_size.read();
        spawn(async move {
            tokio::time::sleep(FOLDER_SLIDE + FIRST_FRAME).await;
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
    // Placed among the rows that stay, as they will be once the leaving ones closed.
    let row = sidebar_scopes(&entries, &collapsed)
        .iter()
        .filter(|s| !row_leaves(s))
        .position(|s| *s == row_scope);
    selected_row.set(row);
    let folded = *last_collapsed.borrow() != collapsed;
    last_collapsed.replace(collapsed);
    let highlight = row.map(|i| {
        let place = sidebar_place(i, font_size());
        let settles = settle
            .borrow_mut()
            .settles(current.scope.clone(), place.top);
        // A sync or a folder moving the same selection's row meanwhile still settles or
        // folds it.
        let class = if leaving.is_some() && !settles {
            "highlight leaving"
        } else if settles && folded {
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
                    Icon { size: 18, stroke: "2.25", paths: SETTINGS }
                }
                button {
                    class: if spinning { "icon-button sync spinning" } else { "icon-button sync" },
                    aria_label: t("sidebar.sync"),
                    title: t("sidebar.sync"),
                    onclick: move |_| reading.run(&sync_app, Action::Sync.into()),
                    Icon { size: 18, stroke: "2.25", paths: ROTATE_CW }
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
                ScopeItem { scope: Scope::All, title: all_label, count: all_count, nested: false, expanded: None, leaving: false }
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
                            let leaves = folder_leaves(&folder.id);
                            let feeds: Vec<_> = feeds
                                .into_iter()
                                .map(|f| (f.clone(), count_of(&f.id), feed_leaves(&f.id)))
                                .collect();
                            rsx! {
                                ScopeItem { key: "folder-{folder.id}", scope, title: folder.title.clone(), count, nested: false, expanded, leaving: leaves }
                                FolderFeeds { feeds, expanded, row: scale::sidebar_row(font_size()) }
                            }
                        }
                        Entry::Feed(feed) => rsx! {
                            FeedItem { key: "{feed.id}", count: count_of(&feed.id), feed: feed.clone(), nested: false, leaving: feed_leaves(&feed.id) }
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
pub(super) fn Feeds(pull: Pull, children: Element) -> Element {
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
pub(super) fn use_spin(status: Signal<Option<SyncStatus>>, asked: Signal<u64>) -> bool {
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
pub(super) fn SidebarSlot() -> Element {
    let reading = use_context::<Reading>();
    let open = use_memo(move || reading.layout.read().sidebar_open)();
    let width = use_memo(move || reading.layout.read().sidebar_width)();
    let shown = use_closing(open, SIDEBAR_SLIDE + FIRST_FRAME);
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

/// Whether something `open` is still in the document: while open, and for `stays` after
/// it closes, so its closing transition plays. With Settings → Animations off, only
/// while open.
pub(super) fn use_closing(open: bool, stays: Duration) -> bool {
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
            tokio::time::sleep(stays).await;
            if changes.get() == change {
                present.set(false);
            }
        });
    }));
    open || present()
}

/// The border after a pane, dragged or moved with ← and → to resize that pane.
#[component]
pub(super) fn Resizer(edge: Edge) -> Element {
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
pub(super) const VIEWS: [View; 3] = [View::Unread, View::Starred, View::All];

/// The view's name in its text keys: `views.<name>.label` and the like.
pub(super) fn view_key(view: View) -> &'static str {
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
pub(super) fn ViewSwitcher() -> Element {
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
                    focus(&button, "the view's button").await;
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
                            svg { width: "16", height: "16", view_box: "0 0 24 24", fill: "currentColor", stroke: "currentColor", stroke_width: "1.85", "aria-hidden": "true",
                                circle { cx: "12", cy: "12", r: "7.5" }
                            }
                        },
                        View::Starred => rsx! {
                            svg { width: "16", height: "16", view_box: "0 0 24 24", fill: "none", stroke: "currentColor", stroke_width: "2.25", stroke_linecap: "round", stroke_linejoin: "round", "aria-hidden": "true",
                                path { d: STAR_PATH }
                            }
                        },
                        View::All => rsx! { Icon { size: 16, stroke: "2.25", paths: LAYERS } },
                    }
                }
            }
        }
    }
}

/// The account menu's button, and the menu while open: each account with its unread
/// count and shortcut, the shown one checked.
#[component]
pub(super) fn AccountSwitcher() -> Element {
    let app = use_context::<Arc<App>>();
    let mut reading = use_context::<Reading>();
    let menu = use_memo(move || match *reading.overlay.read() {
        Overlay::AccountMenu { at } => Some(at),
        _ => None,
    })();
    let active = (reading.account)();
    // The counts change with changes made here and with syncs, as the library does.
    reading.library.read();
    let accounts = app.accounts();
    let menu_app = Arc::clone(&app);
    let app_for_add = Arc::clone(&app);
    let shortcut_key = chord_label(ACCOUNT_MODIFIER, "");

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
                Icon { size: 18, stroke: "2.25", paths: RSS }
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
                                            Icon { size: 14, stroke: "2.75", paths: CHECK }
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
/// closing, the rows stay in it until the slide ends. Each feed comes with its count and
/// whether it is leaving: a leaving feed's row closes, and the box shrinks with it to
/// the rows that stay.
#[component]
pub(super) fn FolderFeeds(
    feeds: Vec<(skvaller_core::Feed, i64, bool)>,
    expanded: bool,
    row: f64,
) -> Element {
    let shown = use_closing(expanded, FOLDER_SLIDE + FIRST_FRAME);
    // Whether the folder last opened or closed, and when: only that slides the box, not
    // a sync adding or removing a feed, which moves the rows and highlight at once.
    let last = use_hook(|| Rc::new(std::cell::Cell::new((expanded, None::<Instant>))));
    let (was, mut toggled) = last.get();
    if was != expanded {
        toggled = Some(Instant::now());
        last.set((expanded, toggled));
    }
    let staying = feeds.iter().filter(|(_, _, leaving)| !leaving).count();
    let class = if toggled.is_some_and(|t| t.elapsed() < FOLDER_SLIDE + FIRST_FRAME) {
        "folder-feeds sliding"
    } else if staying < feeds.len() {
        "folder-feeds leaving"
    } else {
        "folder-feeds"
    };
    let rows = if expanded { staying } else { 0 };
    let height = f64::from(u32::try_from(rows).unwrap_or(u32::MAX)) * row;
    let feeds = if shown { feeds } else { Vec::new() };
    rsx! {
        div { class, style: "height: {height}px",
            for (feed, count, leaving) in feeds {
                FeedItem { key: "{feed.id}", count, feed, nested: true, leaving }
            }
        }
    }
}

#[component]
pub(super) fn FeedItem(
    feed: skvaller_core::Feed,
    count: i64,
    nested: bool,
    leaving: bool,
) -> Element {
    let scope = Scope::Feed { id: feed.id };
    rsx! {
        ScopeItem { scope, title: feed.title, count, nested, expanded: None, leaving }
    }
}

/// Where the sidebar's row `index` is, counted as [`sidebar_scopes`] lists them: all
/// feeds, then after the Feeds heading each folder and feed shown, at `font_size`.
pub(super) fn sidebar_place(index: usize, font_size: FontSize) -> Place {
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
/// disclosure button that shows or hides its feeds. A `leaving` row closes and fades
/// after mark-all-read emptied it, and takes no clicks meanwhile.
#[component]
pub(super) fn ScopeItem(
    scope: Scope,
    title: String,
    count: i64,
    nested: bool,
    expanded: Option<bool>,
    leaving: bool,
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
    let class = if leaving {
        format!("{class} leaving")
    } else {
        class.to_owned()
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
                            Icon { size: 16, stroke: "2.25", paths: CHEVRON_RIGHT }
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

#[cfg(test)]
mod tests {
    use skvaller_app::{Clock, DateFormat};

    use super::*;

    #[test]
    fn the_status_line_says_a_damaged_cache_first_then_syncing_then_a_failure() {
        let dates = Dates::system(Clock::H24, DateFormat::Iso);
        let status = |s: SyncStatus| status_line(Some(&s), &dates);
        let failed = SyncStatus {
            error: Some("timeout".into()),
            ..SyncStatus::default()
        };
        assert_eq!(
            status(failed.clone()),
            (format!("{}: timeout", t("sidebar.syncFailed")), true)
        );
        let syncing = SyncStatus {
            syncing: true,
            ..failed.clone()
        };
        assert_eq!(status(syncing.clone()), (t("sidebar.syncing"), false));
        let broken = SyncStatus {
            cache_broken: true,
            ..syncing
        };
        assert_eq!(
            status(broken),
            (format!("{}: timeout", t("sidebar.cacheDamaged")), true)
        );
        assert_eq!(status_line(None, &dates), (t("sidebar.notSynced"), false));
    }
}
