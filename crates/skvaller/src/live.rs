//! The settings as they are now, for a window to follow as they change, and the
//! changes to the accounts' caches.
//!
//! Each window runs its own `VirtualDom`, so each makes its own [`Live`] from the
//! [`App`]'s config revision, which every settings or account change bumps, and from
//! the system's dark mode and window frame where the app reads them ([`appearance`]).
//! A change to a cache doesn't bump the config revision: a window hears it through
//! [`use_cache_changes`], from [`App::changes`].

use std::collections::HashMap;
use std::ops::Deref;
use std::rc::Rc;
use std::sync::Arc;

use dioxus_native::prelude::*;
use skvaller_app::{App, Change, Settings, Theme};
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;

use crate::appearance::{self, SystemFrame};
use crate::dates::Dates;
use crate::shortcuts::{Action, bindings};

/// The settings, and values derived from them and the system's mode, each updating
/// when its inputs change.
#[derive(Clone, Copy, PartialEq)]
pub struct Live {
    /// Bumped on every change to the settings or the accounts, not their caches.
    pub revision: Signal<u64>,
    pub settings: Memo<Settings>,
    /// The theme to show: the setting, with System made Light or Dark where the app
    /// reads the system's mode itself ([`appearance`]).
    pub theme: Memo<Theme>,
    pub dates: Memo<SharedDates>,
    /// The shortcut keys, by key name.
    pub keys: Memo<Rc<HashMap<String, Action>>>,
    /// How the system draws a window's frame, for the one the app draws (`chrome`).
    pub frame: Signal<SystemFrame>,
}

/// [`Dates`], shared. Compared by pointer: a new one is built only when the clock or
/// date order changes.
#[derive(Clone)]
pub struct SharedDates(Rc<Dates>);

impl PartialEq for SharedDates {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.0, &other.0)
    }
}

impl Deref for SharedDates {
    type Target = Dates;

    fn deref(&self) -> &Dates {
        &self.0
    }
}

/// This window's [`Live`], following the app's config.
pub fn use_live(app: &Arc<App>) -> Live {
    let revision = use_followed({
        let app = Arc::clone(app);
        move || app.watch_config()
    });
    let settings = use_memo({
        let app = Arc::clone(app);
        move || {
            revision();
            app.settings()
        }
    });
    // Only a change of clock or date order makes new dates, as everything showing a date
    // follows them.
    let format = use_memo(move || {
        let settings = settings.read();
        (settings.clock, settings.date_format)
    });
    let dates = use_memo(move || {
        let (clock, order) = format();
        SharedDates(Rc::new(Dates::system(clock, order)))
    });
    let keys = use_memo(move || Rc::new(bindings(&settings.read().shortcuts)));
    let dark = use_followed(appearance::watch);
    let theme = use_memo(move || appearance::resolve(settings.read().theme, dark()));
    let frame = use_followed(appearance::watch_frame);
    Live {
        revision,
        settings,
        theme,
        dates,
        keys,
        frame,
    }
}

/// A signal holding what `watch` makes as it changes, for as long as the component
/// lives.
fn use_followed<T: Copy + PartialEq + 'static>(
    watch: impl Fn() -> watch::Receiver<T> + 'static,
) -> Signal<T> {
    let mut signal = use_signal(|| *watch().borrow());
    use_hook(move || {
        spawn(async move {
            let mut changes = watch();
            // A new receiver marks the current value as seen, so read it before waiting:
            // it may have changed since the signal was made.
            loop {
                let now = *changes.borrow_and_update();
                if now != *signal.peek() {
                    signal.set(now);
                }
                if changes.changed().await.is_err() {
                    break;
                }
            }
        });
    });
    signal
}

/// Calls `heard` with each change to an account's cache, for as long as the component
/// lives. Calls it with `None` when it starts listening and after missing changes: the
/// caller should then assume anything may have changed.
pub fn use_cache_changes(app: &Arc<App>, mut heard: impl FnMut(Option<&Change>) + 'static) {
    let app = Arc::clone(app);
    use_hook(move || {
        spawn(async move {
            let mut changes = app.changes();
            heard(None);
            loop {
                match changes.recv().await {
                    Ok(change) => heard(Some(&change)),
                    Err(RecvError::Lagged(_)) => heard(None),
                    Err(RecvError::Closed) => break,
                }
            }
        });
    });
}
