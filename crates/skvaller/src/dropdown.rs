//! A dropdown drawn like the app's menus, in place of a `<select>`.
//!
//! Blitz paints a `<select>`'s open list itself, with fixed colors and square corners.
//! This one opens over its button as on macOS, with the current choice over the
//! button's label. It stays inside the window: it slides up or down to fit and scrolls
//! when the window is too short for it.

use std::rc::Rc;
use std::sync::Arc;

use dioxus_html::geometry::{PixelsVector2D, WheelDelta};
use dioxus_native::prelude::*;
use dioxus_native::use_window;

use crate::icons::{CHECK, CHEVRONS_UP_DOWN, Icon};

/// A row's height: `.menu-item`'s.
const ROW: f64 = 28.0;
/// The list's border and padding together: `.popover`'s border and `.menu`'s padding.
const INSET: f64 = 6.0;
/// From a row's left edge to its label: `.menu-item`'s padding, check column and gap.
const LABEL_X: f64 = 28.0;
/// From the button's left edge to its label: `.dropdown-button`'s left padding.
const BUTTON_LABEL_X: f64 = 10.0;
/// The room kept between the list and the window's edges, as `.dropdown-backdrop`'s
/// padding keeps it at the sides.
const MARGIN: f64 = 8.0;
/// How far one wheel notch scrolls, as Blitz scrolls it.
const NOTCH: f64 = 60.0;
/// A frame, at 60 Hz.
const FRAME: std::time::Duration = std::time::Duration::from_millis(16);
/// How many frames to wait at most for a new list's layout.
const LAYOUT_WAITS: u32 = 10;

/// A button showing the choice at `selected`, which opens the list of `choices`.
///
/// `onchange` gets the index of a choice picked other than the current one.
#[component]
pub fn Dropdown(
    label: String,
    choices: Vec<String>,
    selected: usize,
    onchange: EventHandler<usize>,
) -> Element {
    let window = use_window();
    let mut button = use_signal(|| None::<Rc<MountedData>>);
    let mut list = use_signal(|| None::<Rc<MountedData>>);
    let mut open = use_signal(|| None::<Open>);
    let count = choices.len();
    let current = choices.get(selected).cloned().unwrap_or_default();

    let show = move || {
        let Some(mounted) = button.peek().clone() else {
            return;
        };
        let window = Arc::clone(&window);
        spawn(async move {
            let rect = match mounted.get_client_rect().await {
                Ok(rect) => rect,
                Err(e) => {
                    tracing::warn!("placing a dropdown's list failed: {e}");
                    return;
                }
            };
            let size = window
                .surface_size()
                .to_logical::<f64>(window.scale_factor());
            let button = Rect {
                x: rect.min_x(),
                y: rect.min_y(),
                width: rect.width(),
                height: rect.height(),
            };
            let place = place(button, selected, count, size.height);
            open.set(Some(Open {
                at: selected,
                place,
            }));
        });
    };
    let mut close = move || {
        open.set(None);
        list.set(None);
        if let Some(mounted) = button.peek().clone() {
            spawn(async move {
                if let Err(e) = mounted.set_focus(true).await {
                    tracing::warn!("focusing a dropdown failed: {e}");
                }
            });
        }
    };
    let mut pick = move |i: usize| {
        close();
        // A highlight left from choices that changed while the list was open.
        if i != selected && i < count {
            onchange.call(i);
        }
    };
    // Moves the highlight to `i` and scrolls the list to show it.
    let mut highlight = move |i: usize| {
        let Some(state) = *open.peek() else {
            return;
        };
        open.set(Some(Open { at: i, ..state }));
        let Some(mounted) = list.peek().clone() else {
            return;
        };
        spawn(async move {
            let Ok(offset) = mounted.get_scroll_offset().await else {
                return;
            };
            let y = reveal(offset.y, state.place.height, i);
            if (y - offset.y).abs() > 0.5
                && let Err(e) = mounted
                    .scroll(PixelsVector2D::new(0.0, y), ScrollBehavior::Instant)
                    .await
            {
                tracing::warn!("scrolling a dropdown's list failed: {e}");
            }
        });
    };
    let labels = choices.clone();
    let show_on_click = show.clone();
    let on_key = move |e: Event<KeyboardData>| {
        // Shortcuts such as Cmd+W go on to the window.
        if e.modifiers()
            .intersects(Modifiers::CONTROL | Modifiers::META | Modifiers::ALT)
        {
            return;
        }
        // Closes the list and moves the focus on: the window's handlers would cancel
        // that, as they do for keys in the Add Feed form.
        if e.key() == Key::Named(NamedKey::Tab) {
            open.set(None);
            list.set(None);
            e.stop_propagation();
            return;
        }
        // Copied out: a borrow held into the match would make setting `open` panic.
        let state = *open.peek();
        let handled = match (state, e.key()) {
            // Return is left to the window, as on macOS.
            (None, Key::Named(NamedKey::ArrowDown | NamedKey::ArrowUp)) => {
                show();
                true
            }
            (None, Key::Character(c)) if c == " " => {
                show();
                true
            }
            (None, _) => false,
            (Some(s), Key::Named(NamedKey::ArrowDown)) => {
                highlight((s.at + 1).min(count.saturating_sub(1)));
                true
            }
            (Some(s), Key::Named(NamedKey::ArrowUp)) => {
                highlight(s.at.saturating_sub(1));
                true
            }
            (Some(_), Key::Named(NamedKey::Home)) => {
                highlight(0);
                true
            }
            (Some(_), Key::Named(NamedKey::End)) => {
                highlight(count.saturating_sub(1));
                true
            }
            (Some(s), Key::Named(NamedKey::Enter)) => {
                pick(s.at);
                true
            }
            (Some(_), Key::Named(NamedKey::Escape)) => {
                close();
                true
            }
            (Some(s), Key::Character(c)) if c == " " => {
                pick(s.at);
                true
            }
            (Some(s), Key::Character(c)) => {
                if let Some(i) = find(&labels, s.at, &c) {
                    highlight(i);
                }
                true
            }
            (Some(_), _) => true,
        };
        if handled {
            e.stop_propagation();
            e.prevent_default();
        }
    };

    let state = open();
    rsx! {
        // The keys are heard here, so they work with the focus on the button or, after
        // a press on it, on the list.
        div { class: "dropdown", onkeydown: on_key,
            button {
                class: "dropdown-button",
                aria_label: "{label}, {current}",
                "aria-haspopup": "listbox",
                "aria-expanded": "{state.is_some()}",
                onmounted: move |e: MountedEvent| button.set(Some(e.data())),
                onclick: move |e| {
                    // The main window closes its popovers on any click that reaches it.
                    e.stop_propagation();
                    show_on_click();
                },
                span { class: "label", "{current}" }
                Icon { size: 14, stroke: "2", paths: CHEVRONS_UP_DOWN }
            }
            if let Some(Open { at, place }) = state {
                // Over the whole window: a click or wheel turn outside the list closes
                // it. The spacer puts the list at its place, and gives way first when
                // the list is too wide to fit there.
                div {
                    class: "dropdown-backdrop",
                    onclick: move |e| {
                        e.stop_propagation();
                        close();
                    },
                    onwheel: move |_| close(),
                    // A press here would take the focus from the button, and the keys
                    // with it, if the release then lands on the list.
                    onmousedown: move |e| e.prevent_default(),
                    div { style: "flex: 0 1 {place.left - MARGIN}px" }
                    div {
                        class: "menu popover dropdown-list",
                        role: "listbox",
                        // Focused by a press, so its keys reach the wrapper, but left out
                        // of the Tab order: Tab from the button would land on it just as
                        // it closes, leaving nothing focused.
                        tabindex: "-1",
                        style: "margin-top: {place.top}px; height: {place.height}px; min-width: {place.width}px",
                        onclick: move |e| e.stop_propagation(),
                        // Kept from the backdrop, whose cancel would stop a drag of the
                        // list's scrollbar.
                        onmousedown: move |e| e.stop_propagation(),
                        // Scrolled here, as Blitz passes what the list can't take of a
                        // turn on to the page under it, away from the button. A stopgap,
                        // without Blitz's eased glide, until the fork honors
                        // `overscroll-behavior: contain`.
                        onwheel: move |e| {
                            e.stop_propagation();
                            e.prevent_default();
                            // Blitz's sign: positive scrolls toward the start.
                            let by = match e.delta() {
                                WheelDelta::Pixels(d) => d.y,
                                WheelDelta::Lines(d) => d.y * NOTCH,
                                WheelDelta::Pages(d) => d.y * place.height,
                            };
                            let Some(mounted) = list.peek().clone() else {
                                return;
                            };
                            spawn(async move {
                                let Ok(offset) = mounted.get_scroll_offset().await else {
                                    return;
                                };
                                let y = (offset.y - by).clamp(0.0, place.range);
                                if let Err(e) = mounted
                                    .scroll(PixelsVector2D::new(0.0, y), ScrollBehavior::Instant)
                                    .await
                                {
                                    tracing::warn!("scrolling a dropdown's list failed: {e}");
                                }
                            });
                        },
                        onmounted: move |e: MountedEvent| {
                            let mounted = e.data();
                            list.set(Some(Rc::clone(&mounted)));
                            async move {
                                if place.scroll > 0.0 {
                                    scroll_when_laid_out(&mounted, place).await;
                                }
                            }
                        },
                        for (i , choice) in choices.iter().enumerate() {
                            div {
                                key: "{i}",
                                class: if i == at { "menu-item highlighted" } else { "menu-item" },
                                role: "option",
                                "aria-selected": "{i == at}",
                                onmouseenter: move |_| {
                                    // Copied out: a borrow held into the block would make
                                    // setting `open` panic.
                                    let state = *open.peek();
                                    if let Some(s) = state
                                        && s.at != i
                                    {
                                        open.set(Some(Open { at: i, ..s }));
                                    }
                                },
                                onclick: move |_| pick(i),
                                span { class: "check",
                                    if i == selected {
                                        Icon { size: 14, stroke: "2.5", paths: CHECK }
                                    }
                                }
                                span { class: "name", "{choice}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Scrolls a list just mounted to `place.scroll`, once Blitz has laid it out.
///
/// A list mounted moments ago has no size yet, so a scroll would stop at 0. Its scroll
/// range shows when it has one; this waits for that for a few frames at most.
async fn scroll_when_laid_out(list: &MountedData, place: Place) {
    for _ in 0..LAYOUT_WAITS {
        match list.get_scroll_size().await {
            Ok(size) if size.height >= place.range - 0.5 => break,
            Ok(_) => tokio::time::sleep(FRAME).await,
            Err(e) => {
                tracing::warn!("reading a dropdown's list failed: {e}");
                return;
            }
        }
    }
    if let Err(e) = list
        .scroll(
            PixelsVector2D::new(0.0, place.scroll),
            ScrollBehavior::Instant,
        )
        .await
    {
        tracing::warn!("scrolling a dropdown's list failed: {e}");
    }
}

/// The open list: its highlighted row and its place.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Open {
    at: usize,
    place: Place,
}

/// A box in window coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

/// Where the open list goes, in window coordinates, and how far it starts scrolled.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    left: f64,
    top: f64,
    width: f64,
    height: f64,
    scroll: f64,
    /// How far the list scrolls: 0 when it fits.
    range: f64,
}

/// Places the list of `count` rows so the row at `selected` covers `button`, as on macOS.
///
/// Near the window's top or bottom the list slides to stay inside, and when the window
/// is too short for it, it fills the window's height and scrolls so the row stays as
/// close to the button as it can. The CSS keeps it off the sides.
fn place(button: Rect, selected: usize, count: usize, window_height: f64) -> Place {
    let full = 2.0 * INSET + rows(count);
    let height = full.min((window_height - 2.0 * MARGIN).max(ROW + 2.0 * INSET));
    let row_y = button.y + (button.height - ROW) / 2.0;
    let wanted = row_y - INSET - rows(selected);
    let top = wanted.min(window_height - MARGIN - height).max(MARGIN);
    let range = full - height;
    let scroll = (top - wanted).clamp(0.0, range);
    let left = (button.x + BUTTON_LABEL_X - INSET - LABEL_X).max(MARGIN);
    Place {
        left,
        top,
        width: button.x + button.width - left,
        height,
        scroll,
        range,
    }
}

/// The height of `n` rows.
#[expect(clippy::cast_precision_loss, reason = "a dropdown has few choices")]
fn rows(n: usize) -> f64 {
    n as f64 * ROW
}

/// The scroll offset that shows row `i` in a list of `height`, scrolled to `offset`.
fn reveal(offset: f64, height: f64, i: usize) -> f64 {
    let top = INSET + rows(i);
    let bottom = top + ROW;
    if top - INSET < offset {
        top - INSET
    } else if bottom + INSET > offset + height {
        bottom + INSET - height
    } else {
        offset
    }
}

/// The first label after `at` that starts with `typed`, wrapping around, ignoring case.
fn find(labels: &[String], at: usize, typed: &str) -> Option<usize> {
    let typed = typed.to_lowercase();
    (1..=labels.len())
        .map(|step| (at + step) % labels.len())
        .find(|&i| labels[i].to_lowercase().starts_with(&typed))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUTTON: Rect = Rect {
        x: 400.0,
        y: 200.0,
        width: 120.0,
        height: 26.0,
    };

    #[test]
    fn selected_row_covers_the_button() {
        let p = place(BUTTON, 2, 5, 600.0);
        let row_top = p.top + INSET + 2.0 * ROW;
        assert_eq!(row_top, BUTTON.y + (BUTTON.height - ROW) / 2.0);
        assert_eq!(p.left + INSET + LABEL_X, BUTTON.x + BUTTON_LABEL_X);
        assert_eq!(p.scroll, 0.0);
    }

    #[test]
    fn slides_down_from_the_top_edge() {
        let button = Rect { y: 10.0, ..BUTTON };
        let p = place(button, 4, 5, 600.0);
        assert_eq!(p.top, MARGIN);
        assert_eq!(p.height, 2.0 * INSET + 5.0 * ROW);
        assert_eq!(p.scroll, 0.0);
    }

    #[test]
    fn slides_up_from_the_bottom_edge() {
        let button = Rect { y: 580.0, ..BUTTON };
        let p = place(button, 0, 5, 600.0);
        assert_eq!(p.top + p.height, 600.0 - MARGIN);
    }

    #[test]
    fn scrolls_when_taller_than_the_window() {
        let p = place(BUTTON, 30, 40, 400.0);
        assert_eq!(p.top, MARGIN);
        assert_eq!(p.height, 400.0 - 2.0 * MARGIN);
        // The row stays over the button.
        let row_top = p.top + INSET + 30.0 * ROW - p.scroll;
        assert_eq!(row_top, BUTTON.y + (BUTTON.height - ROW) / 2.0);
    }

    #[test]
    fn scroll_stops_at_the_list_end() {
        let button = Rect { y: 10.0, ..BUTTON };
        let p = place(button, 39, 40, 400.0);
        assert_eq!(p.scroll, 2.0 * INSET + 40.0 * ROW - p.height);
    }

    #[test]
    fn keeps_off_the_left_edge() {
        let button = Rect { x: 4.0, ..BUTTON };
        let p = place(button, 0, 3, 600.0);
        assert_eq!(p.left, MARGIN);
        assert_eq!(p.left + p.width, button.x + button.width);
    }

    #[test]
    fn reveal_scrolls_only_as_needed() {
        let height = 2.0 * INSET + 5.0 * ROW;
        assert_eq!(reveal(0.0, height, 2), 0.0);
        assert_eq!(reveal(0.0, height, 5), ROW);
        assert_eq!(reveal(3.0 * ROW, height, 1), ROW);
    }

    #[test]
    fn find_wraps_and_ignores_case() {
        let labels = ["Apple", "banana", "Blueberry", "cherry"].map(String::from);
        assert_eq!(find(&labels, 0, "b"), Some(1));
        assert_eq!(find(&labels, 1, "B"), Some(2));
        assert_eq!(find(&labels, 2, "b"), Some(1));
        assert_eq!(find(&labels, 0, "z"), None);
    }
}
