//! The reader: the open article in its own document, and its buttons.

use super::*;
use crate::icons::{GLOBE, SQUARE_TEXT, SQUARE_TEXT_FILLED};

/// Lucide's star.
pub(super) const STAR_PATH: &str = "M11.525 2.295a.53.53 0 0 1 .95 0l2.31 4.679a2.123 2.123 0 0 0 1.595 1.16l5.166.756a.53.53 0 0 1 .294.904l-3.736 3.638a2.123 2.123 0 0 0-.611 1.878l.882 5.14a.53.53 0 0 1-.771.56l-4.618-2.428a2.122 2.122 0 0 0-1.973 0L6.396 21.01a.53.53 0 0 1-.77-.56l.881-5.139a2.122 2.122 0 0 0-.611-1.879L2.16 9.795a.53.53 0 0 1 .294-.906l5.165-.755a2.122 2.122 0 0 0 1.597-1.16z";

#[component]
pub(super) fn Reader() -> Element {
    let app = use_context::<Arc<App>>();
    let mut reading = use_context::<Reading>();
    let live = use_context::<Live>();
    // Only what the document shows remakes it: remade, the frame loads again, back at
    // the top, so a new look keeps the article's place (`Remake::keep_place`).
    let look = use_memo(move || article::Look::new(&live.settings.read(), (live.theme)()));
    // The Reader View text to show, apart: its loading states don't remake the document.
    let full_text = use_memo(move || {
        let rv = reading.reader_view.read();
        let key = Some(((reading.account)()?, (reading.article)()?.id));
        rv.html.clone().filter(|_| rv.on && rv.article == key)
    });
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
            spawn(remake.keep_place(reading.frame, reading.article, full_text, look, next));
        }
    });
    // Another article opening, or its text swapping, forgets the one remade: it fades
    // in, and so does the remade one shown again.
    use_effect(move || {
        let id = reading.article.read().as_ref().map(|a| a.id.clone());
        let shown = full_text.read().is_some();
        let mut remade = remake.article;
        if remade
            .peek()
            .as_ref()
            .is_some_and(|(r, text)| Some(r) != id.as_ref() || *text != shown)
        {
            remade.set(None);
        }
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
            let feed = lib.feed(&a.feed_id);
            let date = live.dates.read().full(a.published);
            let mut look = (remake.look)();
            // Remade for a look, it doesn't fade in again; swapped to or from Reader View
            // it does.
            let shown = (a.id.clone(), full_text.read().is_some());
            if (remake.article).read().as_ref() == Some(&shown) {
                look.animations = false;
            }
            Some(article::document(&a, feed, &html, &date, look))
        }
    });
    let open = reading.open_article();
    // The sidebar counts until its closing slide ends, as the reader widens only then:
    // without its first frame, as the buttons showing a frame early do no harm.
    let sidebar_open = use_memo(move || reading.layout.read().sidebar_open)();
    let sidebar_shown = use_closing(sidebar_open, SIDEBAR_SLIDE);
    // A memo, so a resize re-renders the reader only when this changes.
    let buttons_fit = use_memo(use_reactive!(|sidebar_shown| {
        // Where the system draws the window buttons they always fit, without following
        // the window's size.
        if !DRAWS_FRAME {
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
                        // A click in the article goes to its own document, so the
                        // window's click, which closes the open overlay, never comes.
                        // The press still reaches here. Not `close_overlay`: the frame
                        // takes the focus after this, and its refocus would take it back.
                        onpointerdown: move |e: PointerEvent| {
                            // As the window's click: the primary button only.
                            if e.trigger_button() != Some(MouseButton::Primary) {
                                return;
                            }
                            let overlay = reading.overlay.peek().after_click();
                            reading.set_overlay(overlay);
                        },
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

/// How many frames a new look waits at most for the remade article's layout to put it
/// back in its place: a long article takes a while to lay out.
pub(super) const PLACE_WAITS: u32 = 60;

/// The reader's document as a new look remakes it.
#[derive(Clone, Copy)]
pub(super) struct Remake {
    /// The look the document is made in.
    pub(super) look: Signal<article::Look>,
    /// Bumped for each look: it keys the frame, so the remade one mounts anew.
    pub(super) count: Signal<u64>,
    /// The article whose remade frame is to be put back at a place, and the place, while
    /// it waits for its layout.
    pub(super) kept: CopyValue<Option<(String, f64)>>,
    /// The frame last mounted, with the `count` it was made at: the one to put back is
    /// the one made for its look, not one a newer look is already replacing.
    pub(super) frame: Signal<Option<(u64, Rc<MountedData>)>>,
    /// The remade frame hides until it is in its place, so it doesn't show the top, nor
    /// its fade in, first.
    pub(super) hidden: Signal<bool>,
    /// The article the document was last remade for, and whether it showed Reader View's
    /// text: remade, it doesn't fade in again, as an article opening or its text swapping
    /// does.
    pub(super) article: Signal<Option<(String, bool)>>,
}

impl Remake {
    /// Shows the article in the look `next`, the newest of `look`, at the place it is
    /// scrolled to now in `frame`, while `article` stays open; `full_text` says whether
    /// it shows Reader View's text.
    ///
    /// A new look remakes the reader's document, and the frame loads it from the top. So
    /// the place, as a share of how far the frame scrolls, is read first, and the remade
    /// frame scrolls there as it lays out, until its height holds still. A look changed
    /// again meanwhile, as by ⌘+ held down, keeps the place `kept` is waiting to
    /// restore rather than reading it from a frame still loading.
    pub(super) async fn keep_place(
        mut self,
        frame: Signal<Option<Rc<MountedData>>>,
        article: Signal<Option<ArticleSummary>>,
        full_text: Memo<Option<String>>,
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
            self.article
                .set(Some((id.clone(), full_text.peek().is_some())));
        }
        let made = *self.count.peek();
        if let Some(place) = place {
            let mut last = None;
            // How many waits the remade frame has had no scroll range: at a smaller size
            // it may not scroll at all.
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

/// Which way the reader's toolbar icons pop: the unread mark, the star and Reader View,
/// when one turns on or off for the article shown, not when an article opens with it on.
#[derive(Debug, Default)]
pub(super) struct Pops {
    pub(super) id: String,
    pub(super) unread: Pop,
    pub(super) star: Pop,
    pub(super) reader_view: Pop,
}

/// How an icon of [`Pops`] last popped.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Motion {
    #[default]
    Still,
    /// Turned on: it pops.
    On,
    /// Turned off: it pops too, under another animation name, so the next turn restarts
    /// it.
    Off,
}

impl Motion {
    /// The class that plays it, after a space.
    fn class(self) -> &'static str {
        match self {
            Self::Still => "",
            Self::On => " pop",
            Self::Off => " pop-off",
        }
    }
}

/// One icon of [`Pops`]: whether it is on, and how it popped last.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Pop {
    pub(super) on: bool,
    pub(super) motion: Motion,
}

impl Pop {
    /// Takes in `on` for the same article: an icon pops when it turns on or off, and
    /// keeps that class until it turns again, so a re-render doesn't replay it.
    pub(super) fn update(&mut self, on: bool) -> Motion {
        if on != self.on {
            self.motion = if on { Motion::On } else { Motion::Off };
        }
        self.on = on;
        self.motion
    }

    fn opened(on: bool) -> Self {
        Self {
            on,
            motion: Motion::Still,
        }
    }
}

impl Pops {
    /// Takes in what the toolbar shows now, and returns how the unread mark, the star
    /// and Reader View pop.
    pub(super) fn update(
        &mut self,
        id: &str,
        read: bool,
        starred: bool,
        reader_view: bool,
    ) -> (Motion, Motion, Motion) {
        if self.id != id {
            id.clone_into(&mut self.id);
            self.unread = Pop::opened(!read);
            self.star = Pop::opened(starred);
            self.reader_view = Pop::opened(reader_view);
        }
        (
            self.unread.update(!read),
            self.star.update(starred),
            self.reader_view.update(reader_view),
        )
    }
}

/// The open article's read, star and browser buttons.
#[component]
pub(super) fn ReaderButtons(article: ArticleSummary) -> Element {
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
    let browser_app = Arc::clone(&app);
    let (rv_on, rv_loading, rv_failed) = {
        let rv = reading.reader_view.read();
        (
            rv.on,
            rv.on && matches!(rv.fetch, Fetch::Loading { .. }),
            rv.fetch == Fetch::Failed,
        )
    };
    let pops = use_hook(|| Rc::new(RefCell::new(Pops::default())));
    let (unread_motion, star_motion, rv_motion) =
        pops.borrow_mut().update(&article.id, read, starred, rv_on);
    let unread_class = format!(
        "icon-button{}{}",
        if read { "" } else { " unread" },
        unread_motion.class()
    );
    let star_class = format!(
        "icon-button{}{}",
        if starred { " starred" } else { "" },
        star_motion.class()
    );
    let rv_class = format!(
        "icon-button{}{}{}",
        if rv_on { " reader-view" } else { "" },
        if rv_loading { " loading" } else { "" },
        rv_motion.class()
    );
    rsx! {
        div { class: "spacer",
            // Announced, and shown until the next try or article.
            span { class: "notice", role: "status",
                if rv_failed { {t("reader.unavailable")} }
            }
        }
        button {
            class: unread_class,
            aria_label: read_label.clone(),
            title: read_label,
            onclick: move |_| reading.run(&app, Action::ToggleRead.into()),
            // The unread mark: filled while unread, outlined once read.
            svg { width: "18", height: "18", view_box: "0 0 24 24", fill: if read { "none" } else { "currentColor" }, stroke: "currentColor", stroke_width: "1.85",
                circle { cx: "12", cy: "12", r: "7.5" }
            }
        }
        button {
            class: star_class,
            aria_label: star_label.clone(),
            title: star_label,
            onclick: move |_| reading.run(&star_app, Action::ToggleStar.into()),
            svg { width: "18", height: "18", view_box: "0 0 24 24", fill: if starred { "currentColor" } else { "none" }, stroke: "currentColor", stroke_width: "2.25", stroke_linejoin: "round",
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
            // The pulse on its own element: the icon's pop doesn't restart when loading
            // ends. Filled while on, outlined while off, as the unread mark.
            span { class: "pulse",
                if rv_on {
                    svg { width: "18", height: "18", view_box: "0 0 24 24", fill: "currentColor", fill_rule: "evenodd", "aria-hidden": "true",
                        path { d: SQUARE_TEXT_FILLED }
                    }
                } else {
                    Icon { size: 18, stroke: "2.25", paths: SQUARE_TEXT }
                }
            }
        }
        button {
            class: "icon-button",
            aria_label: t("reader.openInBrowser"),
            title: t("reader.openInBrowser"),
            disabled: article.url.is_empty(),
            onclick: move |_| reading.run(&browser_app, Action::OpenInBrowser.into()),
            Icon { size: 18, stroke: "2.25", paths: GLOBE }
        }
    }
}
