//! The Settings window, a window of its own as in the Tauri version.
//!
//! It runs its own `VirtualDom`, so it shares nothing with the main window but what it
//! is given: the [`App`], and the [`SettingsWindow`] slot that says whether it is open.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::{Arc, Weak};

use dioxus_native::prelude::*;
use dioxus_native::winit::window::Window;
use dioxus_native::{Config, LogicalSize, WindowAttributes, Windows, use_window};
use skvaller_app::App;

use crate::article;
use crate::fonts;
use crate::i18n::t;
use crate::ui::APP_CSS;

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
    let theme = article::theme_name(app.settings().theme);
    let ui_font = format!("--ui-font: {}", fonts::stack(app.settings().ui_font));
    let accounts = app.accounts();

    rsx! {
        style { {APP_CSS} }
        div {
            class: "root settings",
            "data-theme": theme,
            style: ui_font,
            tabindex: "-1",
            onmounted: move |e: MountedEvent| async move {
                let _ = e.data().set_focus(true).await;
            },
            onkeydown: move |e: Event<KeyboardData>| {
                if e.key() == Key::Named(NamedKey::Escape) {
                    windows.close_window(id);
                }
            },
            h1 { {t("settings.label")} }
            h2 { {t("settings.accounts")} }
            ul {
                for account in accounts {
                    li { key: "{account.id}", "{account.description}" }
                }
            }
        }
    }
}
