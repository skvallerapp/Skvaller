//! The windows' frame: on macOS and Linux, no title bar, the toolbars in its place.
//!
//! macOS draws the window buttons over the top of the window, centered in a unified
//! title bar 52px tall, and the toolbars grow to that height to hold them (app.css,
//! under `.mac`). On Linux the window has no decorations at all: the app draws its own
//! minimize, maximize and close buttons at the top right ([`WindowButtons`]), and its
//! own [`Frame`]: the system's outline and rounded corners, read through the portal
//! (`appearance::SystemFrame`), and edges to resize by. The window is transparent
//! there, so the corners show what's behind it. On both the window drags from the
//! toolbars' empty space (app.css, under `.frameless`). Windows keeps the system's
//! title bar.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use dioxus_html::input_data::MouseButton;
use dioxus_native::prelude::*;
use dioxus_native::winit::dpi::{LogicalPosition, PhysicalSize};
use dioxus_native::winit::event::WindowEvent;
use dioxus_native::winit::window::{ResizeDirection, Window};
use dioxus_native::{
    Color, CompositeAlphaMode, Config, WindowAttributes, use_window, use_window_event, use_windows,
};

use crate::appearance::{Buttons, SystemFrame};
use crate::i18n::t;
use crate::icons::{Icon, WINDOW_MAXIMIZE, WINDOW_MINIMIZE, WINDOW_RESTORE, X};
use crate::live::Live;

/// Whether the app runs on macOS.
pub const MAC: bool = cfg!(target_os = "macos");

/// Whether the app runs on Linux, where it draws the window's frame itself.
pub const LINUX: bool = cfg!(target_os = "linux");

/// Whether the window has no title bar, and drags from its toolbars.
pub const FRAMELESS: bool = MAC || LINUX;

/// How soon a second press counts as a double-click, which zooms the window as a
/// double-click on a title bar does by default: macOS's default speed, 500 ms, and
/// GTK's and KDE's, 400 ms. The user's own speed and title-bar action aren't read.
const DOUBLE_CLICK: Duration = Duration::from_millis(if MAC { 500 } else { 400 });

/// Hides the title bar, keeping the window buttons over the window's content.
#[cfg(target_os = "macos")]
fn frame(attributes: WindowAttributes) -> WindowAttributes {
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

/// Leaves out the window's decorations: the system's title bar on X11 and on a
/// Wayland compositor that draws them, winit's own (sctk-adwaita) on one that doesn't.
/// The window is transparent, for its rounded corners ([`surface`]).
#[cfg(target_os = "linux")]
fn frame(attributes: WindowAttributes) -> WindowAttributes {
    attributes.with_decorations(false).with_transparent(true)
}

/// Keeps the system's title bar: `attributes` as they are.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn frame(attributes: WindowAttributes) -> WindowAttributes {
    attributes
}

/// On Linux, renders the window onto a transparent surface, so that what the window's
/// root leaves out, its rounded corners, shows what's behind it. Elsewhere `config` as
/// it is.
fn surface(config: Config) -> Config {
    if LINUX {
        config
            .with_alpha_mode(CompositeAlphaMode::Transparent)
            .with_base_color(Color::TRANSPARENT)
    } else {
        config
    }
}

/// A window's `config` and `attributes` for its frame, together: on Linux the window's
/// transparency needs both ([`frame`], [`surface`]).
pub fn window(config: Config, attributes: WindowAttributes) -> (Config, WindowAttributes) {
    (surface(config), frame(attributes))
}

/// A layer that drags the window from a toolbar's empty space, where there is no title
/// bar.
///
/// app.css lays it over the whole toolbar and its text, under its buttons, so a press
/// on a button stays the button's. A double-click zooms the window; on Wayland a
/// right-click opens the compositor's window menu, as on a title bar (winit doesn't
/// open one on X11).
#[component]
pub fn DragArea() -> Element {
    // A constant, so the hooks below run on every render or on none.
    if !FRAMELESS {
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
                if window.fullscreen().is_some() {
                    return;
                }
                match e.trigger_button() {
                    Some(MouseButton::Primary) => {}
                    Some(MouseButton::Secondary) if LINUX => {
                        let point = e.client_coordinates();
                        window.show_window_menu(LogicalPosition::new(point.x, point.y).into());
                        return;
                    }
                    _ => return,
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

/// Minimize, maximize and close, for the window's top right (the reader's toolbar,
/// Settings' header, the add-account screen's corner), on Linux only, drawn as the
/// system's window frames draw them ([`Buttons`]).
///
/// `on_close` runs before the window closes, for what the window's own close button
/// would do through `CloseRequested`, which closing it from here doesn't send. The
/// maximize button turns to restore while the window is maximized.
#[component]
pub fn WindowButtons(on_close: Option<EventHandler<()>>) -> Element {
    if !LINUX {
        return rsx! {};
    }
    let window = use_window();
    let windows = use_windows();
    let style = (use_context::<Live>().frame)().buttons;
    let maximized = (use_context::<WindowState>().maximized)();
    let id = window.id();
    let (minimize, zoom) = (Arc::clone(&window), window);
    let (zoom_label, zoom_glyph) = if maximized {
        ("window.restore", Glyph::Restore)
    } else {
        ("window.maximize", Glyph::Maximize)
    };

    rsx! {
        div {
            class: "window-buttons",
            class: if style == Buttons::Breeze { "breeze" },
            WindowButton {
                label: t("window.minimize"),
                glyph: Glyph::Minimize,
                style,
                onclick: move |_| minimize.set_minimized(true),
            }
            WindowButton {
                label: t(zoom_label),
                glyph: zoom_glyph,
                style,
                onclick: move |_| zoom.set_maximized(!maximized),
            }
            WindowButton {
                label: t("window.close"),
                glyph: Glyph::Close,
                style,
                close: true,
                onclick: move |_| {
                    if let Some(on_close) = on_close {
                        on_close.call(());
                    }
                    windows.close_window(id);
                },
            }
        }
    }
}

/// One of [`WindowButtons`]: `glyph` in `style`, named `label`; `close` for the close
/// button's colors.
#[component]
fn WindowButton(
    label: String,
    glyph: Glyph,
    style: Buttons,
    #[props(default)] close: bool,
    onclick: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        button {
            class: "icon-button window-button",
            class: if close { "close" },
            aria_label: label.clone(),
            title: label,
            onclick,
            ButtonGlyph { glyph, style }
        }
    }
}

/// What a window button shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Glyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// `glyph` in the system's `style`.
///
/// Breeze's are its own strokes (breezebutton.cpp): drawn in an 18-unit box inset by
/// one unit in 20, one unit to a pixel at the 20px Breeze buttons have at the usual
/// font sizes, with a 1.01-unit pen, round caps and mitred joins (round for restore).
#[component]
fn ButtonGlyph(glyph: Glyph, style: Buttons) -> Element {
    match style {
        Buttons::Adwaita => {
            let paths = match glyph {
                Glyph::Minimize => WINDOW_MINIMIZE,
                Glyph::Maximize => WINDOW_MAXIMIZE,
                Glyph::Restore => WINDOW_RESTORE,
                Glyph::Close => X,
            };
            rsx! { Icon { size: 16, stroke: "2", paths } }
        }
        Buttons::Breeze => {
            let (d, join) = match glyph {
                Glyph::Minimize => ("M4 7 9 12 14 7", "miter"),
                Glyph::Maximize => ("M4 11 9 6 14 11", "miter"),
                Glyph::Restore => ("M4 9 9 4 14 9 9 14Z", "round"),
                Glyph::Close => ("M5 5 13 13M13 5 5 13", "miter"),
            };
            rsx! {
                svg {
                    width: "20",
                    height: "20",
                    view_box: "-1 -1 20 20",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "1.01",
                    stroke_linecap: "round",
                    stroke_linejoin: join,
                    "aria-hidden": "true",
                    path { d }
                }
            }
        }
    }
}

/// The window's drop shadow on KDE Plasma (`shadow`), Breeze's, half as strong while
/// the window is inactive. Only where the system's frame is Breeze's, as it is under
/// KWin: elsewhere the window has nothing to hand it to.
#[cfg(target_os = "linux")]
fn use_kwin_shadow(window: &Arc<dyn Window>) {
    use std::cell::{OnceCell, RefCell};

    use crate::shadow::kwin::KwinShadow;

    let frame = use_context::<Live>().frame;
    // Unset until tried, then the shadow or `None`.
    let shadow = use_hook(|| Rc::new(OnceCell::<Option<RefCell<KwinShadow>>>::new()));
    // Set up once the frame is known to be Breeze's, which may be after the first
    // render when the portal answers late. Tried once: a compositor without KWin's
    // shadows keeps not having them.
    use_effect({
        let (shadow, window) = (Rc::clone(&shadow), Arc::clone(window));
        move || {
            if frame().buttons == Buttons::Breeze {
                shadow.get_or_init(|| {
                    let made = KwinShadow::new(&*window);
                    if made.is_none() {
                        tracing::info!("the window's shadow can't be handed to KWin");
                    }
                    made.map(RefCell::new)
                });
            }
        }
    });
    use_window_event({
        let window = Arc::clone(window);
        move |event, _| {
            if let WindowEvent::Focused(active) = event
                && let Some(Some(shadow)) = shadow.get()
            {
                shadow.borrow_mut().set_active(&*window, *active);
            }
        }
    });
}

/// [`Frame`]'s edges and corners: their classes in app.css, and which way each
/// resizes the window.
const EDGES: [(&str, ResizeDirection); 8] = [
    ("n", ResizeDirection::North),
    ("s", ResizeDirection::South),
    ("e", ResizeDirection::East),
    ("w", ResizeDirection::West),
    ("ne", ResizeDirection::NorthEast),
    ("nw", ResizeDirection::NorthWest),
    ("se", ResizeDirection::SouthEast),
    ("sw", ResizeDirection::SouthWest),
];

/// A window's state, as its frame and window buttons show it, following the window
/// as it changes. Each window's root provides it ([`use_window_state_provider`]); its
/// children read it with `use_context`.
#[derive(Clone, Copy)]
pub struct WindowState {
    /// Whether the window is maximized.
    pub maximized: Signal<bool>,
    /// Whether the window shows its frame: on Linux, unless maximized or full screen,
    /// where its edges meet the screen's.
    pub framed: Signal<bool>,
}

/// Provides the window's [`WindowState`] to the root's children, and returns it. It
/// asks the window only on Linux, where something shows it: on X11 `is_maximized` is
/// a round trip to the server.
pub fn use_window_state_provider() -> WindowState {
    // A constant, so the hooks below run on every render or on none.
    if !LINUX {
        return use_context_provider(|| WindowState {
            maximized: Signal::new(false),
            framed: Signal::new(false),
        });
    }
    let window = use_window();
    let x11 = use_hook(|| on_x11(&*window));
    let read = move |w: &dyn Window, size| {
        let maximized = w.is_maximized();
        (maximized, !maximized && !is_fullscreen(w, size, x11))
    };
    let state = use_context_provider(|| {
        let (maximized, framed) = read(&*window, window.surface_size());
        WindowState {
            maximized: Signal::new(maximized),
            framed: Signal::new(framed),
        }
    });
    use_window_event(move |event, _| {
        // Maximizing or full screen resizes the window, whoever asks for it.
        if let WindowEvent::SurfaceResized(size) = event {
            let (now_maximized, now_framed) = read(&*window, *size);
            let WindowState {
                mut maximized,
                mut framed,
            } = state;
            if *maximized.peek() != now_maximized {
                maximized.set(now_maximized);
            }
            if *framed.peek() != now_framed {
                framed.set(now_framed);
            }
        }
    });
    state
}

/// Whether `window` is on X11.
fn on_x11(window: &dyn Window) -> bool {
    use dioxus_native::winit::raw_window_handle::RawWindowHandle;

    window
        .rwh_06_window_handle()
        .window_handle()
        .is_ok_and(|h| {
            matches!(
                h.as_raw(),
                RawWindowHandle::Xlib(_) | RawWindowHandle::Xcb(_)
            )
        })
}

/// Whether `window`, at `size` and not maximized, is full screen; `x11` when the
/// window is on X11 ([`on_x11`]).
///
/// On X11 winit reports only a full screen the app asked for, not one the window
/// manager made, so there a window as large as its monitor counts too, either way
/// round: winit gives a rotated panel's mode unrotated, as the Steam Deck's. Not for a
/// maximized window, which fills a monitor without a panel just as well.
fn is_fullscreen(window: &dyn Window, size: PhysicalSize<u32>, x11: bool) -> bool {
    if window.fullscreen().is_some() {
        return true;
    }
    x11 && window
        .current_monitor()
        .and_then(|m| m.current_video_mode())
        .is_some_and(|mode| {
            let s = mode.size();
            s == size || (s.height, s.width) == (size.width, size.height)
        })
}

/// The CSS variables of a window's root: its UI font `stack`, and on Linux the
/// system's frame (app.css).
pub fn root_style(stack: &str, frame: SystemFrame) -> String {
    use std::fmt::Write as _;

    let mut style = format!("--ui-font: {stack}");
    if LINUX {
        let _ = write!(style, "; --frame-radius: {}px", frame.radius);
        if let Some([r, g, b]) = frame.outline {
            let _ = write!(style, "; --frame-outline: rgb({r} {g} {b})");
        }
    }
    style
}

/// The window's frame on Linux, in a root whose class has `framed` while
/// [`WindowState::framed`] says: an outline in the system's color, around corners
/// rounded as the system's (app.css), and the edges and corners to resize the window
/// by.
#[component]
pub fn Frame() -> Element {
    if !LINUX {
        return rsx! {};
    }
    let window = use_window();
    #[cfg(target_os = "linux")]
    use_kwin_shadow(&window);
    if !(use_context::<WindowState>().framed)() {
        return rsx! {};
    }

    rsx! {
        div { class: "frame-outline" }
        for (name, direction) in EDGES {
            div {
                key: "{name}",
                class: "resize-edge {name}",
                onmousedown: {
                    let window = Arc::clone(&window);
                    move |e: MouseEvent| {
                        if e.trigger_button() != Some(MouseButton::Primary) {
                            return;
                        }
                        // A resize moves no focus: not off a modal, such as the config
                        // notice, to the root behind it.
                        e.prevent_default();
                        if let Err(err) = window.drag_resize_window(direction) {
                            tracing::warn!("resizing the window failed: {err}");
                        }
                    }
                },
            }
        }
    }
}
