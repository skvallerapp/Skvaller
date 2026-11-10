//! The Settings window, a window of its own as in the Tauri version.
//!
//! It runs its own `VirtualDom`, so it shares nothing with the main window but what it
//! is given: the [`App`], and the [`SettingsWindow`] slot that says whether it is open.
//! A change is saved at once, and every window follows it through the app's config
//! revision. Settings for what the native app doesn't have yet, such as thumbnails, are
//! left out until it does.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Weak};

use dioxus_native::prelude::*;
use dioxus_native::winit::window::Window;
use dioxus_native::{Config, LogicalSize, WindowAttributes, Windows, use_window};
use skvaller_app::{
    Account, AccountChange, AccountKind, App, Clock, DateFormat, Font, MarkReadOn, Settings,
    SyncSchedule, Theme,
};

use crate::article;
use crate::dates::Dates;
use crate::fonts;
use crate::i18n::{t, t_count};
use crate::live::use_live;
use crate::ui::APP_CSS;

const SETTINGS_CSS: &str = include_str!("settings.css");

/// The open Settings window, if any, shared by the windows on the UI thread.
#[derive(Clone, Default)]
pub struct SettingsWindow(Rc<RefCell<Option<Weak<dyn Window>>>>);

impl SettingsWindow {
    fn get(&self) -> Option<Arc<dyn Window>> {
        self.0.borrow().as_ref().and_then(Weak::upgrade)
    }

    /// Opens the Settings window, or brings it to the front when it is open.
    pub fn open(&self, windows: &Windows, app: &Arc<App>) {
        if let Some(window) = self.get() {
            window.set_minimized(false);
            window.focus_window();
            return;
        }
        let attributes = WindowAttributes::default()
            .with_title(t("settings.label"))
            .with_surface_size(LogicalSize::new(720.0, 520.0))
            .with_resizable(false);
        let vdom = VirtualDom::new(SettingsRoot)
            .with_root_context(Arc::clone(app))
            .with_root_context(self.clone());
        let config = Config::new()
            .with_font_ctx(fonts::font_context())
            .with_window_attributes(attributes);
        windows.new_window(vdom, config);
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
enum Pane {
    Appearance,
    Reading,
    /// The account with this id.
    Account(String),
}

/// The Settings window's root component.
#[component]
fn SettingsRoot() -> Element {
    let app = use_context::<Arc<App>>();
    let slot = use_context::<SettingsWindow>();
    let windows = use_context::<Windows>();
    let window = use_window();
    // The main window finds this window through the slot, to focus or close it.
    use_hook(|| *slot.0.borrow_mut() = Some(Arc::downgrade(&window)));
    let id = window.id();
    let live = use_live(&app);
    let pane = use_signal(|| Pane::Appearance);
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
        Pane::Account(id) => accounts.iter().find(|a| &a.id == id).cloned(),
        _ => None,
    };
    let title = match (&current, &account) {
        (Pane::Appearance, _) => t("settings.appearance"),
        (Pane::Reading, _) => t("settings.reading"),
        (Pane::Account(_), Some(a)) => a.description.clone(),
        (Pane::Account(_), None) => String::new(),
    };
    let username = match account.as_ref().map(|a| &a.kind) {
        Some(AccountKind::Greader { username, .. }) => Some(username.clone()),
        _ => None,
    };
    let ui_font = format!("--ui-font: {}", fonts::stack(settings.ui_font));
    let theme = article::theme_name(settings.theme);

    rsx! {
        style { {APP_CSS} }
        style { {SETTINGS_CSS} }
        div {
            class: "root settings",
            "data-theme": theme,
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
                ul {
                    NavRow { pane: Pane::Appearance, current: pane, label: t("settings.appearance"), icon: PALETTE }
                    NavRow { pane: Pane::Reading, current: pane, label: t("settings.reading"), icon: BOOK_OPEN }
                }
                ul { class: "accounts", aria_label: t("settings.accounts"),
                    for a in accounts.iter() {
                        AccountRow { key: "{a.id}", account: a.clone(), current: pane }
                    }
                }
            }
            main {
                header {
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
                            (Pane::Account(_), Some(account)) => rsx! { AccountPane { key: "{account.id}", account, feedback } },
                            (Pane::Account(_), None) => rsx! {},
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
    let selected = *current.read() == pane;
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
    fn report(mut self, result: skvaller_app::Result<()>) {
        match result {
            Ok(()) => self.error.set(None),
            Err(e) => {
                self.error.set(Some(e.to_string()));
                self.failures += 1;
            }
        }
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
    let set = move |change: SettingsChange| feedback.report(app.change_settings(change));
    rsx! {
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
fn AccountPane(account: Account, feedback: Feedback) -> Element {
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
    rsx! {
        div { class: "select",
            select {
                aria_label: label,
                onchange: move |e| {
                    if let Ok(i) = e.value().parse::<usize>() {
                        onchange.call(i);
                    }
                },
                if selected.is_none() {
                    option { value: "other", selected: true, "{other}" }
                }
                for (i , choice) in choices.iter().enumerate() {
                    option { key: "{i}", value: "{i}", selected: Some(i) == selected, "{choice}" }
                }
            }
            Icon { size: 14, stroke: "2", paths: CHEVRONS_UP_DOWN }
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

/// A Lucide icon from its paths.
#[component]
fn Icon(size: u32, stroke: &'static str, paths: &'static [&'static str]) -> Element {
    rsx! {
        svg {
            width: "{size}",
            height: "{size}",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: stroke,
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            for d in paths {
                path { d: *d }
            }
        }
    }
}

const PALETTE: &[&str] = &[
    "M12 22a1 1 0 0 1 0-20 10 9 0 0 1 10 9 5 5 0 0 1-5 5h-2.25a1.75 1.75 0 0 0-1.4 2.8l.3.4a1.75 1.75 0 0 1-1.4 2.8z",
    "M13.5 6.5h.01",
    "M17.5 10.5h.01",
    "M6.5 12.5h.01",
    "M8.5 7.5h.01",
];
const BOOK_OPEN: &[&str] = &[
    "M12 7v14",
    "M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3z",
];
const CIRCLE_USER: &[&str] = &[
    "M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0",
    "M15 10a3 3 0 1 1-6 0 3 3 0 0 1 6 0",
    "M7 20.662V19a2 2 0 0 1 2-2h6a2 2 0 0 1 2 2v1.662",
];
const CHEVRONS_UP_DOWN: &[&str] = &["m7 15 5 5 5-5", "m7 9 5-5 5 5"];

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
