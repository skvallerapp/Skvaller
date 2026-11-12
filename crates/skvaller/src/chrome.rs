//! The windows' frame: on macOS, no title bar, with the toolbars in its place.
//!
//! macOS draws the window buttons over the top of the window, centered in a unified
//! title bar 52px tall, and the toolbars grow to that height to hold them (app.css,
//! under `.mac`). The window then drags from the toolbars' empty space. Windows and
//! Linux keep the system's title bar.

use std::cell::Cell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use dioxus_html::input_data::MouseButton;
use dioxus_native::prelude::*;
use dioxus_native::{WindowAttributes, use_window};

/// Whether the app runs on macOS.
pub const MAC: bool = cfg!(target_os = "macos");

/// How soon a second press counts as a double-click, which zooms the window as a
/// double-click on a title bar does by default. 500 ms is macOS's default speed. The
/// user's own speed and title-bar action (System Settings) aren't read.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);

/// Hides the title bar, keeping the window buttons over the window's content.
#[cfg(target_os = "macos")]
pub fn frame(attributes: WindowAttributes) -> WindowAttributes {
    use dioxus_native::winit::platform::macos::WindowAttributesMacOS;

    let mac = WindowAttributesMacOS::default()
        .with_titlebar_transparent(true)
        .with_title_hidden(true)
        .with_fullsize_content_view(true)
        // The empty toolbar this adds makes the title bar 52px, with the buttons
        // centered in it and inset from the corner as in Finder or Mail.
        .with_unified_titlebar(true);
    attributes.with_platform_attributes(Box::new(mac))
}

/// Keeps the system's title bar: `attributes` as they are.
#[cfg(not(target_os = "macos"))]
pub fn frame(attributes: WindowAttributes) -> WindowAttributes {
    attributes
}

/// A layer that drags the window from a toolbar's empty space, on macOS only.
///
/// app.css lays it over the whole toolbar and its text, under its buttons, so a press
/// on a button stays the button's. A double-click zooms the window.
#[component]
pub fn DragArea() -> Element {
    // A constant, so the hooks below run on every render or on none.
    if !MAC {
        return rsx! {};
    }
    let window = use_window();
    let last_press = use_hook(|| Rc::new(Cell::new(None::<Instant>)));

    rsx! {
        div {
            class: "drag-area",
            onmousedown: move |e: MouseEvent| {
                // In full screen there is nothing to drag, and a zoom asked for there
                // would wait and happen on leaving it.
                if e.trigger_button() != Some(MouseButton::Primary)
                    || window.fullscreen().is_some()
                {
                    return;
                }
                let now = Instant::now();
                let double = last_press
                    .replace(Some(now))
                    .is_some_and(|then| now.duration_since(then) < DOUBLE_CLICK);
                if double {
                    last_press.set(None);
                    window.set_maximized(!window.is_maximized());
                } else if let Err(err) = window.drag_window() {
                    tracing::warn!("dragging the window failed: {err}");
                }
            },
        }
    }
}
