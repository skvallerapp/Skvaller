//! The main window: sidebar, article list and reader.
//!
//! Everything shown is read from the local caches through [`App`]; nothing here waits
//! for the network. A sync that changes the cache bumps a revision, which reloads the
//! sidebar and the list.

// Rust guideline compliant 2026-10-09

use std::sync::Arc;

use dioxus_native::prelude::*;
use skvaller_app::{App, Library, SyncStatus};
use skvaller_core::{ArticleSummary, Scope, Selection, View};
use tokio::sync::broadcast::error::RecvError;

use crate::article;
use crate::i18n::{t, t_count};

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
            account()
                .and_then(|id| app.library(&id).ok())
                .unwrap_or_default()
        }
    });
    let theme = article::theme_name(app.settings().theme);

    rsx! {
        style { {APP_CSS} }
        div { class: "root", "data-theme": theme,
            Sidebar { library, selection, status, article }
            ArticleList { account, selection, revision, article, library }
            Reader { account, article, library }
        }
    }
}

#[component]
fn Sidebar(
    library: Memo<Library>,
    selection: Signal<Selection>,
    status: Signal<Option<SyncStatus>>,
    article: Signal<Option<ArticleSummary>>,
) -> Element {
    let lib = library();
    let unread_total: i64 = lib.counts.unread_by_feed.values().sum();
    let starred_total: i64 = lib.counts.starred_by_feed.values().sum();
    let mut select = move |next: Selection| {
        selection.set(next);
        article.set(None);
    };
    let current = selection();
    let views = [
        (View::Unread, t("views.unread.label"), unread_total),
        (View::Starred, t("views.starred.label"), starred_total),
        (View::All, t("views.all.label"), 0),
    ];
    let counts_for = |view: View, id: &str| match view {
        View::Unread | View::All => lib.counts.unread_by_feed.get(id).copied().unwrap_or(0),
        View::Starred => lib.counts.starred_by_feed.get(id).copied().unwrap_or(0),
    };
    let feeds_in = |folder: Option<&str>| -> Vec<skvaller_core::Feed> {
        lib.feeds
            .iter()
            .filter(|f| f.subscribed && f.folder_id.as_deref() == folder)
            .cloned()
            .collect()
    };
    let status_line = match status() {
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

    rsx! {
        nav { class: "sidebar", aria_label: t("sidebar.label"),
            div { class: "toolbar" }
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
                        let count: i64 = feeds_in(Some(&folder.id)).iter().map(|f| counts_for(current.view, &f.id)).sum();
                        rsx! {
                            div {
                                key: "folder-{folder.id}",
                                class: if current.scope == scope { "item selected" } else { "item" },
                                onclick: move |_| select(Selection { view: selection.peek().view, scope: scope.clone() }),
                                span { class: "name", "{folder.title}" }
                                if count > 0 { span { class: "count", "{count}" } }
                            }
                            for feed in feeds_in(Some(&folder.id)) {
                                FeedItem { key: "{feed.id}", feed: feed.clone(), count: counts_for(current.view, &feed.id), nested: true, selection, article }
                            }
                        }
                    }
                }
                for feed in feeds_in(None) {
                    FeedItem { key: "{feed.id}", feed: feed.clone(), count: counts_for(current.view, &feed.id), nested: false, selection, article }
                }
            }
            div { class: if status_line.1 { "status error" } else { "status" }, "{status_line.0}" }
        }
    }
}

#[component]
fn FeedItem(
    feed: skvaller_core::Feed,
    count: i64,
    nested: bool,
    selection: Signal<Selection>,
    article: Signal<Option<ArticleSummary>>,
) -> Element {
    let scope = Scope::Feed {
        id: feed.id.clone(),
    };
    let selected = selection.read().scope == scope;
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
                let view = selection.peek().view;
                selection.set(Selection { view, scope: scope.clone() });
                article.set(None);
            },
            span { class: "name", "{feed.title}" }
            if count > 0 { span { class: "count", "{count}" } }
        }
    }
}

#[component]
fn ArticleList(
    account: Signal<Option<String>>,
    selection: Signal<Selection>,
    revision: Signal<u64>,
    article: Signal<Option<ArticleSummary>>,
    library: Memo<Library>,
) -> Element {
    let app = use_context::<Arc<App>>();
    let articles = use_memo(move || {
        revision();
        let selection = selection();
        account()
            .and_then(|id| app.articles(&id, &selection).ok())
            .unwrap_or_default()
    });
    let mut top = use_signal(|| 0.0_f64);
    let mut height = use_signal(|| INITIAL_VIEWPORT);

    let list = articles.read();
    let unread = list.iter().filter(|a| !a.read).count();
    let heading = if unread == 0 {
        t("list.noUnread")
    } else {
        t_count("list.unread", i64::try_from(unread).unwrap_or(i64::MAX))
    };
    let (first, last) = visible_rows(top(), height(), list.len());
    let total = row_offset(list.len());
    let selected = article.read().as_ref().map(|a| a.id.clone());
    let lib = library.read();
    let feed_title = |id: &str| {
        lib.feeds
            .iter()
            .find(|f| f.id == id)
            .map(|f| f.title.clone())
            .unwrap_or_default()
    };
    // A new list starts at the top: a new viewport, keyed on what it lists.
    let key = format!("{:?}", selection.read());

    rsx! {
        section { class: "list", aria_label: t("list.label"),
            div { class: "toolbar", h1 { "{heading}" } }
            // A list of one, keyed on what it lists: a new selection gets a new viewport,
            // scrolled to the top.
            for key in std::iter::once(key) {
            div {
                key: "{key}",
                class: "viewport",
                onscroll: move |e: Event<ScrollData>| {
                    top.set(e.data().scroll_top());
                    height.set(f64::from(e.data().client_height()));
                },
                if list.is_empty() {
                    p { class: "empty", {t("list.noUnread")} }
                }
                div { class: "spacer", style: "height: {total}px",
                    for (i , a) in list.iter().enumerate().take(last).skip(first) {
                        {
                            let summary = a.clone();
                            let class = match (selected.as_deref() == Some(a.id.as_str()), a.read) {
                                (true, true) => "row read selected",
                                (true, false) => "row selected",
                                (false, true) => "row read",
                                (false, false) => "row",
                            };
                            rsx! {
                                div {
                                    key: "{a.id}",
                                    class,
                                    style: "top: {row_offset(i)}px; height: {ROW_HEIGHT}px",
                                    onclick: move |_| article.set(Some(summary.clone())),
                                    div { class: "meta",
                                        span { class: "feed", {feed_title(&a.feed_id)} }
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
    }
}

#[component]
fn Reader(
    account: Signal<Option<String>>,
    article: Signal<Option<ArticleSummary>>,
    library: Memo<Library>,
) -> Element {
    let app = use_context::<Arc<App>>();
    let theme = app.settings().theme;
    let doc = use_memo(move || {
        let a = article()?;
        let html = account().and_then(|id| app.article_html(&id, &a.id).ok().flatten())?;
        let lib = library.peek();
        let feed = lib.feeds.iter().find(|f| f.id == a.feed_id);
        Some(article::document(&a, feed, &html, theme))
    });

    rsx! {
        section { class: "reader", aria_label: t("reader.label"),
            div { class: "toolbar" }
            match doc() {
                Some(doc) => rsx! { iframe { key: "{doc.len()}", title: "", "srcdoc": doc } },
                None => rsx! { p { class: "empty", {t("reader.empty")} } },
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
}
