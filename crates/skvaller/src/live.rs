//! The settings as they are now, for a window to follow as they change.
//!
//! Each window runs its own `VirtualDom`, so each makes its own [`Live`] from the
//! [`App`]'s config revision, which every settings or account change bumps.

use std::collections::HashMap;
use std::ops::Deref;
use std::rc::Rc;
use std::sync::Arc;

use dioxus_native::prelude::*;
use skvaller_app::{App, Settings};

use crate::dates::Dates;
use crate::shortcuts::{Action, bindings};

/// The settings and what is made from them, each changing when they do.
#[derive(Clone, Copy, PartialEq)]
pub struct Live {
    /// Bumped on every change to the settings or an account's.
    pub revision: Signal<u64>,
    pub settings: Memo<Settings>,
    pub dates: Memo<SharedDates>,
    /// The shortcut keys, by key name.
    pub keys: Memo<Rc<HashMap<String, Action>>>,
}

/// [`Dates`], shared. Two are the same only when they are one, as building them is what
/// a change of settings costs.
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
    let mut revision = use_signal(|| *app.watch_config().borrow());
    use_hook({
        let app = Arc::clone(app);
        move || {
            spawn(async move {
                let mut changes = app.watch_config();
                while changes.changed().await.is_ok() {
                    let now = *changes.borrow_and_update();
                    revision.set(now);
                }
            });
        }
    });
    let settings = use_memo({
        let app = Arc::clone(app);
        move || {
            revision();
            app.settings()
        }
    });
    let dates = use_memo(move || {
        let settings = settings.read();
        SharedDates(Rc::new(Dates::system(settings.clock, settings.date_format)))
    });
    let keys = use_memo(move || Rc::new(bindings(&settings.read().shortcuts)));
    Live {
        revision,
        settings,
        dates,
        keys,
    }
}
