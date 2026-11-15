//! Whether the system is in dark mode, where the app has to ask for itself.
//!
//! The System theme follows `prefers-color-scheme`, which Blitz takes from winit. On
//! macOS and Windows winit reports the system's choice. On Linux it reports none (X11
//! always, Wayland unless the app set one), so Blitz shows light even in a dark
//! desktop. There the app reads the choice from the XDG desktop portal's
//! `org.freedesktop.appearance` `color-scheme` setting, as GTK and Qt apps do, and
//! follows its changes. KDE Plasma (the Steam Deck's desktop) and GNOME both set it.
//!
//! [`resolve`] then makes System Light or Dark in the windows' `data-theme`. Two
//! things stay as they were, as with the Dark and Espresso themes: Blitz's own scheme
//! stays light, so scrollbar thumbs keep their light colors; and a change of mode
//! remakes the open article's document, which loads again at the top. Telling Blitz
//! the scheme would take a change to the fork.

use skvaller_app::Theme;
use tokio::sync::watch;

/// The system's dark mode: `Some(true)` dark, `Some(false)` light, `None` when only
/// `prefers-color-scheme` knows (or nothing does).
pub type SystemDark = Option<bool>;

/// `theme` as the window shows it: System made Light or Dark when the app knows the
/// system's mode itself.
pub fn resolve(theme: Theme, dark: SystemDark) -> Theme {
    match (theme, dark) {
        (Theme::System, Some(true)) => Theme::Dark,
        (Theme::System, Some(false)) => Theme::Light,
        (theme, _) => theme,
    }
}

/// Starts reading the system's mode, in the background. Later calls do nothing.
pub fn start() {
    imp::start();
}

/// Waits briefly for the first answer, so a dark system doesn't open the first window
/// light and turn it dark. A missing or slow portal leaves `None` until it answers.
pub fn wait_for_first_answer() {
    imp::wait_for_first_answer();
}

/// The system's mode as it changes, starting to read it if [`start`] hasn't.
pub fn watch() -> watch::Receiver<SystemDark> {
    imp::sender().subscribe()
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use std::sync::OnceLock;

    use tokio::sync::watch;

    use super::SystemDark;

    /// Nothing to read: winit reports the system's mode here.
    pub fn start() {}

    pub fn wait_for_first_answer() {}

    /// Always `None`.
    pub fn sender() -> &'static watch::Sender<SystemDark> {
        static SENDER: OnceLock<watch::Sender<SystemDark>> = OnceLock::new();
        SENDER.get_or_init(|| watch::channel(None).0)
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::sync::mpsc;
    use std::sync::{Mutex, OnceLock, PoisonError};
    use std::time::Duration;

    use tokio::sync::watch;
    use zbus::blocking::{Connection, Proxy};
    use zbus::zvariant::{OwnedValue, Value};

    use super::SystemDark;

    const PORTAL: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    const SETTINGS: &str = "org.freedesktop.portal.Settings";
    const NAMESPACE: &str = "org.freedesktop.appearance";
    const KEY: &str = "color-scheme";
    const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";

    /// How long [`wait_for_first_answer`] waits. The portal answers in about 10 ms.
    const FIRST_ANSWER: Duration = Duration::from_millis(300);

    /// The mode, and the first answer's signal until something waits for it.
    struct Reader {
        sender: watch::Sender<SystemDark>,
        first: Mutex<Option<mpsc::Receiver<()>>>,
    }

    fn reader() -> &'static Reader {
        static READER: OnceLock<Reader> = OnceLock::new();
        READER.get_or_init(|| {
            let (sender, _) = watch::channel(None);
            let (first, answered) = mpsc::channel();
            let thread_sender = sender.clone();
            let started = std::thread::Builder::new()
                .name("appearance".into())
                .spawn(move || {
                    if let Err(err) = follow(&thread_sender, &first) {
                        tracing::info!("the system's dark mode can't be followed: {err}");
                    }
                    // Dropping `first` ends a wait for it if nothing was sent.
                });
            if let Err(err) = started {
                tracing::warn!("starting the appearance thread failed: {err}");
            }
            Reader {
                sender,
                first: Mutex::new(Some(answered)),
            }
        })
    }

    pub fn start() {
        reader();
    }

    pub fn wait_for_first_answer() {
        let first = reader()
            .first
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(first) = first {
            // An answer, the thread's end or the timeout: each lets the window open.
            let _ = first.recv_timeout(FIRST_ANSWER);
        }
    }

    pub fn sender() -> &'static watch::Sender<SystemDark> {
        &reader().sender
    }

    /// Reads the portal's setting into `sender`, telling `first` once it has, then
    /// follows its changes until the session bus goes away.
    fn follow(sender: &watch::Sender<SystemDark>, first: &mpsc::Sender<()>) -> zbus::Result<()> {
        let connection = Connection::session()?;
        let proxy = Proxy::new(&connection, PORTAL, PATH, SETTINGS)?;
        // Subscribed before reading, so a change between the two isn't missed. A bus
        // that refuses the subscription may still answer the read: the mode then shows
        // as it was at launch.
        let changes = proxy.receive_signal("SettingChanged");
        // A portal without the setting yet may set it later: keep listening.
        match read(&proxy) {
            Ok(dark) => {
                sender.send_replace(dark);
            }
            Err(err) => tracing::info!("the system's dark mode can't be read: {err}"),
        }
        let _ = first.send(());
        for message in changes? {
            let Ok((namespace, key, value)) =
                message.body().deserialize::<(String, String, OwnedValue)>()
            else {
                continue;
            };
            if namespace == NAMESPACE && key == KEY {
                sender.send_replace(dark(&value));
            }
        }
        Ok(())
    }

    /// The setting now. `ReadOne` is the portal's call since version 2; older ones have
    /// only `Read`, which wraps the value in one more variant.
    fn read(proxy: &Proxy<'_>) -> zbus::Result<SystemDark> {
        match proxy.call::<_, _, OwnedValue>("ReadOne", &(NAMESPACE, KEY)) {
            Ok(value) => Ok(dark(&value)),
            Err(zbus::Error::MethodError(name, ..)) if name.as_str() == UNKNOWN_METHOD => {
                let value: OwnedValue = proxy.call("Read", &(NAMESPACE, KEY))?;
                Ok(dark(&value))
            }
            Err(err) => Err(err),
        }
    }

    /// The portal's `color-scheme`: 1 prefers dark, 2 prefers light, 0 (or anything
    /// else) states no preference.
    fn dark(value: &Value<'_>) -> SystemDark {
        match value {
            Value::Value(inner) => dark(inner),
            Value::U32(1) => Some(true),
            Value::U32(2) => Some(false),
            _ => None,
        }
    }

    #[cfg(test)]
    mod tests {
        use zbus::zvariant::Value;

        use super::dark;

        #[test]
        fn the_portals_color_scheme_reads_as_dark_light_or_unknown() {
            assert_eq!(dark(&Value::U32(1)), Some(true));
            assert_eq!(dark(&Value::U32(2)), Some(false));
            assert_eq!(dark(&Value::U32(0)), None);
            // `Read`'s answer, a variant in a variant.
            assert_eq!(dark(&Value::Value(Box::new(Value::U32(1)))), Some(true));
            // Not the portal's type.
            assert_eq!(dark(&Value::from("dark")), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use skvaller_app::Theme;

    use super::resolve;

    #[test]
    fn only_the_system_theme_follows_the_systems_mode() {
        assert_eq!(resolve(Theme::System, Some(true)), Theme::Dark);
        assert_eq!(resolve(Theme::System, Some(false)), Theme::Light);
        assert_eq!(resolve(Theme::System, None), Theme::System);
        assert_eq!(resolve(Theme::Espresso, Some(true)), Theme::Espresso);
        assert_eq!(resolve(Theme::Light, Some(true)), Theme::Light);
    }
}
