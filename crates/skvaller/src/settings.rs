//! The Settings window, a window of its own as in the Tauri version.
//!
//! It runs its own `VirtualDom`, so it shares nothing with the main window but what it
//! is given: the [`App`], and the [`SettingsWindow`] slot that says whether it is open.
//! A change is saved at once, and every window follows it through the app's config
//! revision. Settings for what the native app doesn't have yet are left out until it
//! does.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Weak};

use dioxus_native::prelude::*;
use dioxus_native::winit::window::Window;
use dioxus_native::{Config, LogicalSize, WindowAttributes, Windows, use_window};
use skvaller_app::{
    Account, AccountChange, AccountKind, App, Clock, DateFormat, Font, MarkReadOn, Settings,
    SortSubscriptions, SyncSchedule, Theme, ThumbnailSize,
};
use skvaller_core::ImportReport;
use tokio::sync::Notify;

use crate::add_account::AddAccount;
use crate::article;
use crate::chrome::{self, DragArea, MAC};
use crate::dates::Dates;
use crate::dropdown::Dropdown;
use crate::fonts;
use crate::i18n::{sentence, t, t_count, t_with};
use crate::icons::{
    BOOK_OPEN, CHEVRON_LEFT, CHEVRON_RIGHT, CIRCLE_USER, Icon, KEYBOARD, PALETTE, PLUS,
};
use crate::live::use_live;
use crate::shortcuts::{self, Action, Capture, Refusal};
use crate::ui::APP_CSS;

const SETTINGS_CSS: &str = include_str!("settings.css");
/// The smallest window in which every pane still lays out.
const MIN_WIDTH: f64 = 560.0;
const MIN_HEIGHT: f64 = 400.0;

/// The open Settings window, if any, shared by the windows on the UI thread.
#[derive(Clone, Default)]
pub struct SettingsWindow {
    window: Rc<RefCell<Option<Weak<dyn Window>>>>,
    /// The pane another window asked to show, until the Settings window takes it.
    pane: Rc<RefCell<Option<Pane>>>,
    /// Tells an open Settings window that a pane was asked for.
    asked: Rc<Notify>,
}

impl SettingsWindow {
    fn get(&self) -> Option<Arc<dyn Window>> {
        self.window.borrow().as_ref().and_then(Weak::upgrade)
    }

    /// Opens the Settings window, or brings it to the front when it is open.
    pub fn open(&self, windows: &Windows, app: &Arc<App>) {
        if let Some(window) = self.get() {
            window.set_minimized(false);
            window.focus_window();
            return;
        }
        let attributes = chrome::frame(
            WindowAttributes::default()
                .with_title(t("settings.label"))
                .with_surface_size(LogicalSize::new(720.0, 520.0))
                .with_min_surface_size(LogicalSize::new(MIN_WIDTH, MIN_HEIGHT)),
        );
        let vdom = VirtualDom::new(SettingsRoot)
            .with_root_context(Arc::clone(app))
            .with_root_context(self.clone());
        let config = Config::new()
            .with_font_ctx(fonts::font_context())
            .with_window_attributes(attributes);
        windows.new_window(vdom, config);
    }

    /// Opens the Settings window at `pane`.
    pub fn open_at(&self, windows: &Windows, app: &Arc<App>, pane: Pane) {
        *self.pane.borrow_mut() = Some(pane);
        self.asked.notify_one();
        self.open(windows, app);
    }

    /// Closes the Settings window, if open.
    pub fn close(&self, windows: &Windows) {
        if let Some(window) = self.get() {
            windows.close_window(window.id());
        }
    }
}

/// A section of the window.
#[derive(Clone, Debug, PartialEq)]
pub enum Pane {
    Appearance,
    Reading,
    Shortcuts,
    /// The account with this id.
    Account(String),
    /// The Reader View page of the account with this id: its feeds' setting.
    ReaderView(String),
    AddAccount,
}

/// The Settings window's root component.
#[component]
fn SettingsRoot() -> Element {
    let app = use_context::<Arc<App>>();
    let slot = use_context::<SettingsWindow>();
    let windows = use_context::<Windows>();
    let window = use_window();
    // The main window finds this window through the slot, to focus or close it.
    use_hook(|| *slot.window.borrow_mut() = Some(Arc::downgrade(&window)));
    let id = window.id();
    let live = use_live(&app);
    let mut pane = use_signal(|| slot.pane.borrow_mut().take().unwrap_or(Pane::Appearance));
    // A pane asked for while the window is open, as by the main window's Add Account.
    use_hook(|| {
        let slot = slot.clone();
        spawn(async move {
            loop {
                slot.asked.notified().await;
                let asked = slot.pane.borrow_mut().take();
                if let Some(asked) = asked {
                    pane.set(asked);
                }
            }
        });
    });
    // A removed account's pane gives way to the next account's, or the first section.
    use_effect({
        let app = Arc::clone(&app);
        move || {
            (live.revision)();
            let accounts = app.accounts();
            let gone = matches!(&*pane.read(), Pane::Account(id) | Pane::ReaderView(id) if !accounts.iter().any(|a| &a.id == id));
            if gone {
                pane.set(
                    accounts
                        .first()
                        .map_or(Pane::Appearance, |a| Pane::Account(a.id.clone())),
                );
            }
        }
    });
    // Back from an account's Reader View page: its row takes the focus again.
    let returned = use_signal(|| false);
    let feedback = Feedback {
        error: use_signal(|| None),
        failures: use_signal(|| 0),
    };

    let settings = live.settings.read().clone();
    // The accounts change with the config too, as when one is renamed.
    (live.revision)();
    let accounts = app.accounts();
    let current = pane.read().clone();
    let account = match &current {
        Pane::Account(id) | Pane::ReaderView(id) => accounts.iter().find(|a| &a.id == id).cloned(),
        _ => None,
    };
    let title = match (&current, &account) {
        (Pane::Appearance, _) => t("settings.appearance"),
        (Pane::Reading, _) => t("settings.reading"),
        (Pane::Shortcuts, _) => t("settings.shortcuts"),
        (Pane::Account(_), Some(a)) => a.description.clone(),
        (Pane::ReaderView(_), _) => t("settings.readerView"),
        (Pane::Account(_), None) => String::new(),
        (Pane::AddAccount, _) => t("addAccount.title"),
    };
    let username = match (&current, account.as_ref().map(|a| &a.kind)) {
        (Pane::Account(_), Some(AccountKind::Greader { username, .. })) => Some(username.clone()),
        _ => None,
    };
    // The account's subscribed feeds, for its Reader View page. Read again with every
    // config revision, which a change of a feed's setting bumps.
    let feeds = match &current {
        Pane::ReaderView(id) => app
            .library(id)
            .map(|lib| {
                lib.feeds
                    .into_iter()
                    .filter(|f| f.subscribed)
                    .collect::<Vec<_>>()
            })
            .map_err(|e| sentence(&e.to_string())),
        _ => Ok(Vec::new()),
    };
    let ui_font = format!("--ui-font: {}", fonts::stack(settings.ui_font));
    let theme = article::theme_name(settings.theme);
    let motion = crate::ui::motion(settings.animations);

    rsx! {
        style { {APP_CSS} }
        style { {SETTINGS_CSS} }
        div {
            class: "root settings",
            class: if MAC { "mac" },
            "data-theme": theme,
            "data-motion": motion,
            style: ui_font,
            tabindex: "-1",
            onmounted: move |e: MountedEvent| async move {
                // Without focus the window hears no keys, so Escape wouldn't close it.
                if let Err(err) = e.data().set_focus(true).await {
                    tracing::warn!("focusing the Settings window failed: {err}");
                }
            },
            onkeydown: move |e: Event<KeyboardData>| {
                if e.key() == Key::Named(NamedKey::Escape) {
                    windows.close_window(id);
                }
            },
            nav { aria_label: t("settings.label"),
                if MAC {
                    // Where the title bar was: room for the window buttons, to drag by.
                    div { class: "drag-strip", DragArea {} }
                }
                ul {
                    NavRow { pane: Pane::Appearance, current: pane, label: t("settings.appearance"), icon: PALETTE }
                    NavRow { pane: Pane::Reading, current: pane, label: t("settings.reading"), icon: BOOK_OPEN }
                    NavRow { pane: Pane::Shortcuts, current: pane, label: t("settings.shortcuts"), icon: KEYBOARD }
                }
                ul { class: "accounts", aria_label: t("settings.accounts"),
                    for a in accounts.iter() {
                        AccountRow { key: "{a.id}", account: a.clone(), current: pane }
                    }
                    NavRow { pane: Pane::AddAccount, current: pane, label: t("accounts.add"), icon: PLUS }
                }
            }
            main {
                header {
                    DragArea {}
                    if let (Pane::ReaderView(id), Some(a)) = (&current, &account) {
                        button {
                            class: "back",
                            aria_label: t_with("settings.back", &[("account", &a.description)]),
                            // The row that opened the page is gone: focus moves to the way
                            // back, as in the Tauri version.
                            onmounted: move |e: MountedEvent| async move {
                                if let Err(err) = e.data().set_focus(true).await {
                                    tracing::warn!("focusing Back failed: {err}");
                                }
                            },
                            onclick: {
                                let id = id.clone();
                                let mut returned = returned;
                                move |_| {
                                    returned.set(true);
                                    pane.set(Pane::Account(id.clone()));
                                }
                            },
                            Icon { size: 16, stroke: "2", paths: CHEVRON_LEFT }
                            "{a.description}"
                        }
                    }
                    h1 { "{title}" }
                    if let Some(username) = username {
                        p { "{username}" }
                    }
                }
                if let Some(e) = (feedback.error)() {
                    p { class: "error", role: "alert", "{e}" }
                }
                // Made again after a failed change, so its controls show what is saved
                // rather than what was picked.
                for failures in [(feedback.failures)()] {
                    div { key: "{failures}", class: "content",
                        match (current.clone(), account.clone()) {
                            (Pane::Appearance, _) => rsx! { AppearancePane { settings: settings.clone(), feedback } },
                            (Pane::Reading, _) => rsx! { ReadingPane { settings: settings.clone(), feedback } },
                            (Pane::Shortcuts, _) => rsx! { ShortcutsPane { overrides: settings.shortcuts.clone(), feedback } },
                            (Pane::Account(_), Some(account)) => {
                                let id = account.id.clone();
                                rsx! {
                                    AccountPane {
                                        key: "{account.id}",
                                        account,
                                        feedback,
                                        returned,
                                        on_reader_view: move |()| pane.set(Pane::ReaderView(id.clone())),
                                    }
                                }
                            }
                            (Pane::ReaderView(_), Some(account)) => rsx! {
                                ReaderViewPane { account_id: account.id, feeds: feeds.clone(), feedback }
                            },
                            (Pane::Account(_) | Pane::ReaderView(_), None) => rsx! {},
                            (Pane::AddAccount, _) => rsx! {
                                AddAccount {
                                    embedded: true,
                                    on_added: move |id| pane.set(Pane::Account(id)),
                                }
                            },
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn NavRow(
    pane: Pane,
    current: Signal<Pane>,
    label: String,
    icon: &'static [&'static str],
) -> Element {
    let selected = *current.read() == pane;
    rsx! {
        li {
            button {
                class: if selected { "nav-row selected" } else { "nav-row" },
                "aria-current": if selected { "page" } else { "false" },
                onclick: move |_| current.set(pane.clone()),
                Icon { size: 18, stroke: "2", paths: icon }
                "{label}"
            }
        }
    }
}

/// An account in the pane list: its name, and its user name or that it is local.
#[component]
fn AccountRow(account: Account, current: Signal<Pane>) -> Element {
    let pane = Pane::Account(account.id.clone());
    // Its Reader View page is part of its pane.
    let selected =
        matches!(&*current.read(), Pane::Account(id) | Pane::ReaderView(id) if *id == account.id);
    let detail = match &account.kind {
        AccountKind::Greader { username, .. } => username.clone(),
        AccountKind::Local => t("settings.localAccount"),
    };
    rsx! {
        li {
            button {
                class: if selected { "nav-row account selected" } else { "nav-row account" },
                "aria-current": if selected { "page" } else { "false" },
                onclick: move |_| current.set(pane.clone()),
                Icon { size: 28, stroke: "1.5", paths: CIRCLE_USER }
                span {
                    strong { "{account.description}" }
                    small { "{detail}" }
                }
            }
        }
    }
}

/// What the last change came to, shown over the pane.
#[derive(Clone, Copy, PartialEq)]
struct Feedback {
    /// Why the last change failed.
    error: Signal<Option<String>>,
    /// Counts the failed changes; the pane is made again on each.
    failures: Signal<u32>,
}

impl Feedback {
    fn report(self, result: skvaller_app::Result<()>) {
        match result {
            Ok(()) => self.clear(),
            Err(e) => self.fail(&e.to_string()),
        }
    }

    fn clear(mut self) {
        self.error.set(None);
    }

    fn fail(mut self, why: &str) {
        self.error.set(Some(sentence(why)));
        self.failures += 1;
    }
}

/// A change to the settings, for a control to make.
type SettingsChange = Box<dyn FnOnce(&mut Settings)>;

#[component]
fn AppearancePane(settings: Settings, feedback: Feedback) -> Element {
    let app = use_context::<Arc<App>>();
    let clocks = [Clock::System, Clock::H24, Clock::H12];
    let clock_labels = vec![
        t("settings.clocks.system"),
        t("settings.clocks.h24"),
        t("settings.clocks.h12"),
    ];
    // Shown as today's date in each order.
    let orders = [
        DateFormat::System,
        DateFormat::DayMonth,
        DateFormat::MonthDay,
        DateFormat::Iso,
    ];
    let order_labels = orders
        .iter()
        .map(|&order| match order {
            DateFormat::System => t("settings.dateFormats.system"),
            _ => Dates::system(settings.clock, order).today(),
        })
        .collect();
    let themes = [Theme::System, Theme::Light, Theme::Dark, Theme::Espresso];
    let theme_labels = vec![
        t("settings.themes.system"),
        t("settings.themes.light"),
        t("settings.themes.dark"),
        t("settings.themes.espresso"),
    ];
    let sizes = [
        ThumbnailSize::None,
        ThumbnailSize::Small,
        ThumbnailSize::Medium,
        ThumbnailSize::Large,
    ];
    let size_labels = vec![
        t("settings.thumbnailSizes.none"),
        t("settings.thumbnailSizes.small"),
        t("settings.thumbnailSizes.medium"),
        t("settings.thumbnailSizes.large"),
    ];
    let font_labels: Vec<String> = FONTS.iter().map(|&f| font_name(f)).collect();
    let set = move |change: SettingsChange| feedback.report(app.change_settings(change));

    rsx! {
        section {
            h2 { {t("settings.dateTime")} }
            div { class: "group",
                Row { label: t("settings.clock"),
                    Select {
                        label: t("settings.clock"),
                        choices: clock_labels,
                        selected: index(&clocks, &settings.clock),
                        onchange: {
                            let set = set.clone();
                            move |i: usize| set(Box::new(move |s| s.clock = clocks[i]))
                        },
                    }
                }
                Row { label: t("settings.dateFormat"),
                    Select {
                        label: t("settings.dateFormat"),
                        choices: order_labels,
                        selected: index(&orders, &settings.date_format),
                        onchange: {
                            let set = set.clone();
                            move |i: usize| set(Box::new(move |s| s.date_format = orders[i]))
                        },
                    }
                }
            }
        }
        section {
            h2 { {t("settings.colors")} }
            div { class: "group",
                Row { label: t("settings.theme"),
                    Select {
                        label: t("settings.theme"),
                        choices: theme_labels,
                        selected: index(&themes, &settings.theme),
                        onchange: {
                            let set = set.clone();
                            move |i: usize| set(Box::new(move |s| s.theme = themes[i]))
                        },
                    }
                }
            }
        }
        section {
            h2 { {t("settings.articleList")} }
            div { class: "group",
                Row { label: t("settings.thumbnails"),
                    Select {
                        label: t("settings.thumbnails"),
                        choices: size_labels,
                        selected: index(&sizes, &settings.thumbnail_size),
                        onchange: {
                            let set = set.clone();
                            move |i: usize| set(Box::new(move |s| s.thumbnail_size = sizes[i]))
                        },
                    }
                }
            }
            p { class: "note", {t("settings.thumbnailsNote")} }
        }
        section {
            h2 { {t("settings.motion")} }
            div { class: "group",
                Row { label: t("settings.animations"),
                    Switch {
                        label: t("settings.animations"),
                        on: settings.animations,
                        onchange: {
                            let set = set.clone();
                            move |on: bool| set(Box::new(move |s| s.animations = on))
                        },
                    }
                }
            }
            p { class: "note", {t("settings.animationsNote")} }
        }
        section {
            h2 { {t("settings.fonts")} }
            div { class: "group",
                Row { label: t("settings.uiFont"),
                    Select {
                        label: t("settings.uiFont"),
                        choices: font_labels.clone(),
                        selected: index(&FONTS, &settings.ui_font),
                        onchange: {
                            let set = set.clone();
                            move |i: usize| set(Box::new(move |s| s.ui_font = FONTS[i]))
                        },
                    }
                }
                Row { label: t("settings.headlineFont"),
                    Select {
                        label: t("settings.headlineFont"),
                        choices: font_labels.clone(),
                        selected: index(&FONTS, &settings.headline_font),
                        onchange: {
                            let set = set.clone();
                            move |i: usize| set(Box::new(move |s| s.headline_font = FONTS[i]))
                        },
                    }
                }
                Row { label: t("settings.articleFont"),
                    Select {
                        label: t("settings.articleFont"),
                        choices: font_labels,
                        selected: index(&FONTS, &settings.article_font),
                        onchange: move |i: usize| set(Box::new(move |s| s.article_font = FONTS[i])),
                    }
                }
            }
        }
    }
}

/// The fonts in the order the Tauri version lists them.
const FONTS: [Font; 6] = [
    Font::Inter,
    Font::Geist,
    Font::System,
    Font::SourceSerif,
    Font::Literata,
    Font::Serif,
];

/// A font's name in the settings: the bundled ones go by their own names in every
/// language.
fn font_name(font: Font) -> String {
    match font {
        Font::Inter => "Inter".into(),
        Font::Geist => "Geist".into(),
        Font::System => t("settings.fontNames.system"),
        Font::SourceSerif => "Source Serif".into(),
        Font::Literata => "Literata".into(),
        Font::Serif => t("settings.fontNames.serif"),
    }
}

#[component]
fn ReadingPane(settings: Settings, feedback: Feedback) -> Element {
    let app = use_context::<Arc<App>>();
    let modes = [MarkReadOn::Open, MarkReadOn::Delay, MarkReadOn::Manual];
    let mode_labels = vec![
        t("settings.markReadModes.open"),
        t("settings.markReadModes.delay"),
        t("settings.markReadModes.manual"),
    ];
    let orders = [
        SortSubscriptions::FoldersFirst,
        SortSubscriptions::Alphabetical,
    ];
    let order_labels = vec![
        t("settings.sortOrders.foldersFirst"),
        t("settings.sortOrders.alphabetical"),
    ];
    let set = move |change: SettingsChange| feedback.report(app.change_settings(change));
    rsx! {
        section {
            h2 { {t("settings.subscriptions")} }
            div { class: "group",
                Row { label: t("settings.sort"),
                    Select {
                        label: t("settings.sort"),
                        choices: order_labels,
                        selected: index(&orders, &settings.sort_subscriptions),
                        onchange: {
                            let set = set.clone();
                            move |i: usize| set(Box::new(move |s| s.sort_subscriptions = orders[i]))
                        },
                    }
                }
            }
        }
        section {
            h2 { {t("settings.markingRead")} }
            div { class: "group",
                Row { label: t("settings.markReadOn"),
                    Select {
                        label: t("settings.markReadOn"),
                        choices: mode_labels,
                        selected: index(&modes, &settings.mark_read_on),
                        onchange: {
                            let set = set.clone();
                            move |i: usize| set(Box::new(move |s| s.mark_read_on = modes[i]))
                        },
                    }
                }
                Row { label: t("settings.confirmMarkAll"),
                    Switch {
                        label: t("settings.confirmMarkAll"),
                        on: settings.confirm_mark_all_read,
                        onchange: move |on: bool| set(Box::new(move |s| s.confirm_mark_all_read = on)),
                    }
                }
            }
        }
    }
}

/// Each action and its key, as the Tauri version's `ShortcutList`.
///
/// Clicking a key waits for the next key press on that button, which has the focus
/// then; leaving the button stops waiting.
#[component]
fn ShortcutsPane(overrides: shortcuts::Overrides, feedback: Feedback) -> Element {
    let app = use_context::<Arc<App>>();
    // The action waiting for its new key.
    let mut recording = use_signal(|| None::<Action>);
    // What the last key pressed came to, when worth saying.
    let mut notice = use_signal(|| None::<String>);
    let label_of = |action: Action| t(&format!("shortcuts.actions.{}", action.name()));
    let save = {
        let app = Arc::clone(&app);
        move |new: shortcuts::Overrides| {
            feedback
                .report(app.change_settings(Box::new(move |s: &mut Settings| s.shortcuts = new)));
        }
    };
    let note = match (notice(), recording()) {
        (Some(notice), _) => notice,
        (None, Some(_)) => t_with(
            "shortcuts.recording",
            &[(
                "key",
                &t(if MAC { "keys.delete" } else { "keys.backspace" }),
            )],
        ),
        (None, None) => t("shortcuts.help"),
    };
    rsx! {
        section {
            h2 { {t("shortcuts.heading")} }
            div { class: "group",
                for action in Action::ALL {
                    {
                        let key = shortcuts::key_of(&overrides, action).map(str::to_owned);
                        let waiting = recording() == Some(action);
                        let shown = match (&key, waiting) {
                            (_, true) => t("shortcuts.pressKey"),
                            (Some(key), false) => shortcuts::key_label(key, MAC),
                            (None, false) => t("shortcuts.none"),
                        };
                        let class = match (waiting, key.is_none()) {
                            (true, _) => "key recording",
                            (false, true) => "key none",
                            (false, false) => "key",
                        };
                        let overrides = overrides.clone();
                        let save = save.clone();
                        rsx! {
                            Row { key: "{action.name()}", label: label_of(action),
                                button {
                                    class,
                                    aria_label: format!("{}, {shown}", label_of(action)),
                                    onclick: move |_| {
                                        recording.set(if waiting { None } else { Some(action) });
                                        notice.set(None);
                                    },
                                    onblur: move |_| {
                                        if recording.peek().is_some_and(|a| a == action) {
                                            recording.set(None);
                                        }
                                    },
                                    onkeydown: move |e: Event<KeyboardData>| {
                                        if !waiting {
                                            return;
                                        }
                                        // Tab moves on, as from any button, and stops waiting.
                                        if e.key() == Key::Named(NamedKey::Tab) {
                                            recording.set(None);
                                            return;
                                        }
                                        // The key is this button's alone: Escape mustn't close
                                        // the window, nor a letter reach anything else.
                                        e.stop_propagation();
                                        e.prevent_default();
                                        let code = e.code().to_string();
                                        let new = match shortcuts::capture(&e.key(), &code, e.modifiers()) {
                                            Capture::Wait => return,
                                            Capture::Cancel => {
                                                recording.set(None);
                                                notice.set(None);
                                                return;
                                            }
                                            Capture::Refuse(why) => {
                                                notice.set(Some(match why {
                                                    Refusal::NotSingle => t("shortcuts.singleKey"),
                                                    Refusal::Arrow => t("shortcuts.arrows"),
                                                    Refusal::Enter => t_with(
                                                        "shortcuts.notAllowed",
                                                        &[("key", &shortcuts::key_label("Enter", MAC))],
                                                    ),
                                                }));
                                                return;
                                            }
                                            Capture::Clear => None,
                                            Capture::Key(name) => Some(name),
                                        };
                                        recording.set(None);
                                        let (result, displaced) =
                                            shortcuts::rebind(&overrides, action, new.as_deref());
                                        notice.set(displaced.zip(new).map(|(other, key)| {
                                            t_with(
                                                "shortcuts.displaced",
                                                &[
                                                    ("key", &shortcuts::key_label(&key, MAC)),
                                                    ("action", &label_of(other)),
                                                ],
                                            )
                                        }));
                                        save(result);
                                    },
                                    "{shown}"
                                }
                            }
                        }
                    }
                }
            }
            p { class: "note", aria_live: "polite", "{note}" }
            button {
                class: "button restore",
                disabled: overrides.is_empty(),
                onclick: move |_| {
                    recording.set(None);
                    notice.set(None);
                    save(shortcuts::Overrides::new());
                },
                {t("shortcuts.restore")}
            }
        }
    }
}

/// The account's feeds, each with a switch to open its articles in Reader View.
#[component]
fn ReaderViewPane(
    account_id: String,
    feeds: Result<Vec<skvaller_core::Feed>, String>,
    feedback: Feedback,
) -> Element {
    let app = use_context::<Arc<App>>();
    rsx! {
        section {
            match feeds {
                Err(why) => rsx! { p { class: "error", role: "alert", "{why}" } },
                Ok(feeds) if feeds.is_empty() => rsx! { p { class: "intro", {t("settings.noSubscriptions")} } },
                Ok(feeds) => rsx! {
                    div { class: "group",
                        for feed in feeds {
                            {
                                let (app, account_id, feed_id) = (Arc::clone(&app), account_id.clone(), feed.id.clone());
                                rsx! {
                                    Row { key: "{feed.id}", label: feed.title.clone(),
                                        Switch {
                                            label: feed.title,
                                            on: feed.reader_view,
                                            onchange: move |on: bool| feedback.report(app.set_feed_reader_view(&account_id, &feed_id, on)),
                                        }
                                    }
                                }
                            }
                        }
                    }
                },
            }
            p { class: "note", {t("settings.readerViewNote")} }
        }
    }
}

/// The sync schedules, as the Tauri version offers them.
const SCHEDULES: [SyncSchedule; 6] = [
    SyncSchedule::OnStart,
    SyncSchedule::Every(5),
    SyncSchedule::Every(15),
    SyncSchedule::Every(30),
    SyncSchedule::Every(60),
    SyncSchedule::Manual,
];

/// How long read articles stay, in days.
const KEEP_READ: [u32; 4] = [7, 14, 30, 90];
/// How long unread articles stay, in days; 0 keeps them.
const KEEP_UNREAD: [u32; 5] = [30, 90, 180, 365, 0];

#[component]
fn AccountPane(
    account: Account,
    feedback: Feedback,
    /// Back from the Reader View page, whose row then takes the focus.
    returned: Signal<bool>,
    on_reader_view: EventHandler,
) -> Element {
    let app = use_context::<Arc<App>>();
    let mut name = use_signal(|| account.description.clone());
    let update = {
        let (app, id) = (Arc::clone(&app), account.id.clone());
        move |change: AccountChange| feedback.report(app.update_account(&id, &change))
    };
    let current = account.description.clone();
    let mut rename = {
        let (update, current) = (update.clone(), current.clone());
        move || {
            let next = name.peek().trim().to_owned();
            if next.is_empty() {
                // A blank name isn't taken: the field shows the name again.
                name.set(current.clone());
            } else if next != current {
                // As saved, without the spaces around it.
                name.set(next.clone());
                update(AccountChange {
                    description: Some(next),
                    ..AccountChange::default()
                });
            }
        }
    };
    let schedule_labels: Vec<String> = SCHEDULES.iter().map(|&s| schedule_label(s)).collect();
    let read_labels: Vec<String> = KEEP_READ.iter().map(|&d| period(d)).collect();
    let unread_labels: Vec<String> = KEEP_UNREAD.iter().map(|&d| period(d)).collect();
    let local = account.kind == AccountKind::Local;
    // What the last OPML import or export did.
    let mut note = use_signal(|| None::<String>);
    let mut confirming = use_signal(|| false);
    let mut removing = use_signal(|| false);
    let windows = use_context::<Windows>();
    let window_id = use_window().id();
    let import = {
        let (app, id) = (Arc::clone(&app), account.id.clone());
        move |_| {
            let (app, id) = (Arc::clone(&app), id.clone());
            spawn(async move {
                let Some(file) = rfd::AsyncFileDialog::new()
                    .add_filter("OPML", &["opml", "xml"])
                    .pick_file()
                    .await
                else {
                    return;
                };
                match app.import_opml(&id, file.path()) {
                    Ok(report) => {
                        note.set(Some(import_note(&report)));
                        feedback.clear();
                    }
                    Err(e) => feedback.fail(&e.to_string()),
                }
            });
        }
    };
    let export = use_callback({
        let (app, id) = (Arc::clone(&app), account.id.clone());
        move |()| {
            let (app, id) = (Arc::clone(&app), id.clone());
            spawn(async move {
                let (xml, name) = match app.export_opml(&id) {
                    Ok(opml) => opml,
                    Err(e) => return feedback.fail(&e.to_string()),
                };
                let Some(file) = rfd::AsyncFileDialog::new()
                    .add_filter("OPML", &["opml"])
                    .set_file_name(name)
                    .save_file()
                    .await
                else {
                    return;
                };
                match std::fs::write(file.path(), xml) {
                    Ok(()) => {
                        note.set(Some(t("settings.exported")));
                        feedback.clear();
                    }
                    Err(e) => feedback.fail(&e.to_string()),
                }
            });
        }
    });
    let remove = {
        let (app, id) = (Arc::clone(&app), account.id.clone());
        move |_| {
            let (app, id, windows) = (Arc::clone(&app), id.clone(), windows.clone());
            removing.set(true);
            spawn(async move {
                let removed = app.remove_account(&id).await;
                // Gone, the pane gives way to another; without any account, the main
                // window asks to add one, and Settings closes.
                if removed.is_ok() && app.accounts().is_empty() {
                    windows.close_window(window_id);
                    return;
                }
                removing.set(false);
                feedback.report(removed);
            });
        }
    };
    let question = t_with(
        "settings.removeQuestion",
        &[("account", &account.description)],
    );

    rsx! {
        section {
            h2 { {t("settings.general")} }
            div { class: "group",
                Row { label: t("settings.accountName"),
                    input {
                        class: "text",
                        aria_label: t("settings.accountName"),
                        value: "{name}",
                        spellcheck: "false",
                        oninput: move |e| name.set(e.value()),
                        onkeydown: {
                            let mut rename = rename.clone();
                            move |e: Event<KeyboardData>| {
                                let key = e.key();
                                // With nothing typed, Escape is the window's: it closes.
                                if key == Key::Named(NamedKey::Escape) && *name.peek() == current {
                                    return;
                                }
                                // Other keys typed here are the field's, not the window's.
                                e.stop_propagation();
                                match key {
                                    Key::Named(NamedKey::Enter) => rename(),
                                    // Undoes the typing, as the Tauri version does.
                                    Key::Named(NamedKey::Escape) => name.set(current.clone()),
                                    _ => {}
                                }
                            }
                        },
                        onblur: move |_| rename(),
                    }
                }
                if let AccountKind::Greader { server, .. } = &account.kind {
                    Row { label: t("settings.server"),
                        span { class: "value", "{server}" }
                    }
                }
            }
        }
        section {
            h2 { {t("settings.reading")} }
            div { class: "group",
                button {
                    class: "setting link",
                    onmounted: move |e: MountedEvent| async move {
                        let mut returned = returned;
                        if *returned.peek() {
                            returned.set(false);
                            if let Err(err) = e.data().set_focus(true).await {
                                tracing::warn!("focusing the Reader View row failed: {err}");
                            }
                        }
                    },
                    onclick: move |_| on_reader_view.call(()),
                    span { {t("settings.readerViewRow")} }
                    Icon { size: 16, stroke: "2", paths: CHEVRON_RIGHT }
                }
            }
        }
        section {
            h2 { {t("settings.syncing")} }
            div { class: "group",
                Row { label: t("settings.syncSchedule"),
                    Select {
                        label: t("settings.syncSchedule"),
                        choices: schedule_labels,
                        selected: index(&SCHEDULES, &account.sync),
                        other: schedule_label(account.sync),
                        onchange: {
                            let update = update.clone();
                            move |i: usize| update(AccountChange {
                                sync: Some(SCHEDULES[i]),
                                ..AccountChange::default()
                            })
                        },
                    }
                }
                Row { label: t("settings.syncOnWake"),
                    Switch {
                        label: t("settings.syncOnWake"),
                        on: account.sync_on_wake,
                        onchange: {
                            let update = update.clone();
                            move |on: bool| update(AccountChange {
                                sync_on_wake: Some(on),
                                ..AccountChange::default()
                            })
                        },
                    }
                }
                Row { label: t("settings.keepRead"),
                    Select {
                        label: t("settings.keepRead"),
                        choices: read_labels,
                        selected: index(&KEEP_READ, &account.keep_read_days),
                        other: period(account.keep_read_days),
                        onchange: {
                            let update = update.clone();
                            move |i: usize| update(AccountChange {
                                keep_read_days: Some(KEEP_READ[i]),
                                ..AccountChange::default()
                            })
                        },
                    }
                }
                if local {
                    Row { label: t("settings.keepUnread"),
                        Select {
                            label: t("settings.keepUnread"),
                            choices: unread_labels,
                            selected: index(&KEEP_UNREAD, &account.keep_unread_days),
                            other: period(account.keep_unread_days),
                            onchange: move |i: usize| update(AccountChange {
                                keep_unread_days: Some(KEEP_UNREAD[i]),
                                ..AccountChange::default()
                            }),
                        }
                    }
                }
            }
            p { class: "note", {t("settings.keepStarredNote")} }
        }
        section {
            h2 { {t("settings.subscriptions")} }
            div { class: "buttons",
                if local {
                    button { class: "button wide", onclick: import, {t("settings.importOpml")} }
                }
                button { class: "button wide", onclick: move |_| export.call(()), {t("settings.exportOpml")} }
            }
            if let Some(note) = note() {
                p { class: "note", role: "status", "{note}" }
            }
        }
        section {
            h2 { {t("settings.account")} }
            if confirming() {
                div { class: "group confirm", role: "alertdialog",
                    // Keys here are the question's: Escape cancels it, not the window.
                    onkeydown: move |e: Event<KeyboardData>| {
                        if e.key() == Key::Named(NamedKey::Escape) {
                            e.stop_propagation();
                            confirming.set(false);
                        }
                    },
                    p {
                        strong { "{question}" }
                        if local { {t("settings.removeLocal")} } else { {t("settings.removeSync")} }
                    }
                    div { class: "actions",
                        if local {
                            button { class: "button export", onclick: move |_| export.call(()), {t("settings.exportOpml")} }
                        }
                        // Cancel takes the focus, so a second Return doesn't remove it.
                        button {
                            class: "button",
                            onmounted: move |e: MountedEvent| async move {
                                if let Err(err) = e.data().set_focus(true).await {
                                    tracing::warn!("focusing Cancel failed: {err}");
                                }
                            },
                            onclick: move |_| confirming.set(false),
                            {t("common.cancel")}
                        }
                        button {
                            class: "button destructive",
                            disabled: removing(),
                            onclick: remove,
                            {t("settings.remove")}
                        }
                    }
                }
            } else {
                button {
                    class: "button wide remove",
                    onclick: move |_| confirming.set(true),
                    {t("settings.removeAccount")}
                }
            }
        }
    }
}

/// What an OPML import added and skipped, as the note under its button says.
fn import_note(report: &ImportReport) -> String {
    let added = i64::try_from(report.added).unwrap_or(i64::MAX);
    let skipped = i64::try_from(report.skipped).unwrap_or(i64::MAX);
    match (added, skipped) {
        (0, _) => t("settings.importedNone"),
        (_, 0) => t_count("settings.imported", added),
        _ => t_with(
            "common.twoSentences",
            &[
                ("first", &t_count("settings.imported", added)),
                ("second", &t_count("settings.importSkipped", skipped)),
            ],
        ),
    }
}

/// A sync schedule's name, as the Tauri version names it.
fn schedule_label(schedule: SyncSchedule) -> String {
    match schedule {
        SyncSchedule::OnStart => t("settings.syncSchedules.onStart"),
        SyncSchedule::Every(60) => t("settings.syncSchedules.everyHour"),
        SyncSchedule::Every(minutes) => {
            t_count("settings.syncSchedules.everyMinutes", i64::from(minutes))
        }
        SyncSchedule::Manual => t("settings.syncSchedules.manual"),
    }
}

/// A retention period in the largest unit that fits, as the Tauri version names them.
fn period(days: u32) -> String {
    match days {
        0 => t("settings.keepForever"),
        365 => t_count("settings.years", 1),
        d if d % 30 == 0 => t_count("settings.months", i64::from(d / 30)),
        d if d % 7 == 0 => t_count("settings.weeks", i64::from(d / 7)),
        d => t_count("settings.days", i64::from(d)),
    }
}

/// Where `value` is in `choices`.
fn index<T: PartialEq>(choices: &[T], value: &T) -> Option<usize> {
    choices.iter().position(|c| c == value)
}

/// A labelled row of a group.
#[component]
fn Row(label: String, children: Element) -> Element {
    rsx! {
        div { class: "setting",
            span { "{label}" }
            {children}
        }
    }
}

/// A dropdown of `choices`, by index. A value not among them, as one set by hand in
/// config.json, shows first as `other` until another is picked, as in the Tauri version.
#[component]
fn Select(
    label: String,
    choices: Vec<String>,
    selected: Option<usize>,
    #[props(default)] other: String,
    onchange: EventHandler<usize>,
) -> Element {
    // `other` goes first, so the choices' indexes are one further along. Picking it
    // again doesn't reach `onchange`, as it is the current choice.
    let (choices, selected, shift) = match selected {
        Some(i) => (choices, i, 0),
        None => (std::iter::once(other).chain(choices).collect(), 0, 1),
    };
    rsx! {
        Dropdown {
            label,
            choices,
            selected,
            onchange: move |i: usize| {
                if let Some(i) = i.checked_sub(shift) {
                    onchange.call(i);
                }
            },
        }
    }
}

/// An on/off switch, drawn as the Tauri version's: a track with a knob.
#[component]
fn Switch(label: String, on: bool, onchange: EventHandler<bool>) -> Element {
    rsx! {
        button {
            class: if on { "switch on" } else { "switch" },
            role: "switch",
            aria_label: label,
            "aria-checked": "{on}",
            onclick: move |_| onchange.call(!on),
            span { class: "knob" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periods_use_the_largest_unit_that_fits() {
        assert_eq!(period(7), "1 Week");
        assert_eq!(period(14), "2 Weeks");
        assert_eq!(period(90), "3 Months");
        assert_eq!(period(365), "1 Year");
        assert_eq!(period(0), "Forever");
        assert_eq!(period(10), "10 Days");
    }
}
