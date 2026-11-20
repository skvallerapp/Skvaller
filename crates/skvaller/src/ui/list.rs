//! The article list: its title and search bar, and the rows it draws as it scrolls.

use super::*;

/// What the list shows: the view's label for every feed, else the feed's or folder's
/// name. Empty for one gone from the library.
pub(super) fn list_title(lib: &Library, selection: &Selection) -> String {
    match &selection.scope {
        Scope::All => t(&format!("views.{}.all", view_key(selection.view))),
        Scope::Feed { id } => lib.feed(id).map_or_default(|f| f.title.clone()),
        Scope::Folder { id } => lib.folder(id).map_or_default(|f| f.title.clone()),
    }
}

#[component]
pub(super) fn ArticleList() -> Element {
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
    let listed = searched_within(&selection, searched == Some(true));
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
    let search_key = chord_label(COMMAND_MODIFIER, "f");

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
pub(super) fn SearchBar() -> Element {
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
                    focus(&e.data(), "the search field").await;
                },
                oninput: move |e| reading.set_query(e.value()),
                onkeydown: move |e: Event<KeyboardData>| {
                    let Some(key) = field_key(&e) else { return };
                    match key {
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
pub(super) fn list_key(reading: &Reading) -> String {
    format!(
        "{:?}/{:?}/{:?}",
        reading.account.read(),
        reading.selection.read(),
        reading.search.read().active()
    )
}

/// A row's thumbnail, meta line and headline, in the list or its ghost.
pub(super) fn row_content(row: &Row, beside: Option<&'static str>, below: Option<f64>) -> Element {
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
pub(super) fn Viewport() -> Element {
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
    let feed_title = |id: &str| lib.feed(id).map_or_default(|f| f.title.clone());
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
