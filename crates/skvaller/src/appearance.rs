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
//!
//! On Linux the app also draws the window's frame itself (`chrome`), and the portal
//! says how the system draws its own: [`SystemFrame`]. KDE Plasma passes its
//! `kdeglobals` through the portal, and the outline is made from those colors as
//! Breeze, Plasma's window decoration, makes its own.

use skvaller_app::Theme;
use tokio::sync::watch;

/// The system's dark mode: `Some(true)` dark, `Some(false)` light, `None` when only
/// `prefers-color-scheme` knows (or nothing does).
pub type SystemDark = Option<bool>;

/// How the system draws a window's frame, on Linux, for the frame the app draws.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SystemFrame {
    /// The outline's color, `None` where the system's isn't known: the theme's then.
    pub outline: Option<[u8; 3]>,
    /// The corners' radius in CSS pixels.
    pub radius: f32,
    /// How the window buttons look.
    pub buttons: Buttons,
}

/// The window buttons' look, as the system's window frames draw them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Buttons {
    /// GNOME's (libadwaita's): a line, a square, two squares for restore.
    Adwaita,
    /// KDE's Breeze: chevrons down and up, a diamond for restore, in a circle on hover.
    Breeze,
}

impl SystemFrame {
    /// Where the system isn't known: GNOME's (libadwaita's) corners.
    pub const UNKNOWN: Self = Self {
        outline: None,
        radius: 12.0,
        buttons: Buttons::Adwaita,
    };
}

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

/// Breeze's corner radius, for the shadow KWin draws around it (`shadow`).
#[cfg(target_os = "linux")]
pub use imp::BREEZE_RADIUS;

/// The system's window frame as it changes, starting to read it if [`start`] hasn't.
pub fn watch_frame() -> watch::Receiver<SystemFrame> {
    imp::frame_sender().subscribe()
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use std::sync::OnceLock;

    use tokio::sync::watch;

    use super::{SystemDark, SystemFrame};

    /// Nothing to read: winit reports the system's mode here.
    pub fn start() {}

    pub fn wait_for_first_answer() {}

    /// Always `None`.
    pub fn sender() -> &'static watch::Sender<SystemDark> {
        static SENDER: OnceLock<watch::Sender<SystemDark>> = OnceLock::new();
        SENDER.get_or_init(|| watch::channel(None).0)
    }

    /// Always [`SystemFrame::UNKNOWN`]: the system draws the frame here.
    pub fn frame_sender() -> &'static watch::Sender<SystemFrame> {
        static SENDER: OnceLock<watch::Sender<SystemFrame>> = OnceLock::new();
        SENDER.get_or_init(|| watch::channel(SystemFrame::UNKNOWN).0)
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::HashMap;
    use std::sync::mpsc;
    use std::sync::{Mutex, OnceLock, PoisonError};
    use std::time::Duration;

    use futures_util::StreamExt;
    use tokio::sync::watch;
    use zbus::zvariant::{OwnedValue, Value};
    use zbus::{Connection, Proxy};

    use super::{Buttons, SystemDark, SystemFrame};

    const PORTAL: &str = "org.freedesktop.portal.Desktop";
    const PATH: &str = "/org/freedesktop/portal/desktop";
    const SETTINGS: &str = "org.freedesktop.portal.Settings";
    const NAMESPACE: &str = "org.freedesktop.appearance";
    const KEY: &str = "color-scheme";
    const UNKNOWN_METHOD: &str = "org.freedesktop.DBus.Error.UnknownMethod";
    /// KDE's `kdeglobals`, a namespace per group, such as `org.kde.kdeglobals.WM`.
    const KDEGLOBALS: &str = "org.kde.kdeglobals";

    /// Breeze's corner radius: its `Frame_FrameRadius`, 2.5, times the decoration's
    /// small spacing, 2 px at the usual font sizes.
    pub const BREEZE_RADIUS: f32 = 5.0;
    /// The `kdeglobals` groups the frame is made from. KDE's portal answers `ReadAll`
    /// for these, though not `ReadOne`.
    const FRAME_NAMESPACES: [&str; 5] = [
        "org.kde.kdeglobals.Colors:Header",
        "org.kde.kdeglobals.Colors:View",
        "org.kde.kdeglobals.Colors:Window",
        "org.kde.kdeglobals.KDE",
        "org.kde.kdeglobals.WM",
    ];
    /// `KColorScheme::frameContrast`'s default, where `[KDE] frameContrast` isn't set.
    const FRAME_CONTRAST: f64 = 0.2;

    /// Groups of settings by namespace, as the portal's `ReadAll` answers.
    type Groups = HashMap<String, HashMap<String, OwnedValue>>;

    /// How long [`wait_for_first_answer`] waits: for two calls to the portal, the mode's
    /// and the frame's, which each answer in about 10 ms.
    const FIRST_ANSWER: Duration = Duration::from_millis(300);

    /// The mode and the frame, and the first answer's signal until something waits for
    /// it.
    struct Reader {
        sender: watch::Sender<SystemDark>,
        frame: watch::Sender<SystemFrame>,
        first: Mutex<Option<mpsc::Receiver<()>>>,
    }

    fn reader() -> &'static Reader {
        static READER: OnceLock<Reader> = OnceLock::new();
        READER.get_or_init(|| {
            let (sender, _) = watch::channel(None);
            let (frame, _) = watch::channel(SystemFrame::UNKNOWN);
            let (first, answered) = mpsc::channel();
            let thread_sender = sender.clone();
            let thread_frame = frame.clone();
            let started = std::thread::Builder::new()
                .name("appearance".into())
                .spawn(move || {
                    // Its own runtime: zbus runs the connection, this only waits on it.
                    let followed = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(zbus::Error::from)
                        .and_then(|runtime| {
                            runtime.block_on(follow(&thread_sender, &thread_frame, &first))
                        });
                    if let Err(err) = followed {
                        tracing::info!("the system's appearance can't be followed: {err}");
                    }
                    // Dropping `first` ends a wait for it if nothing was sent.
                });
            if let Err(err) = started {
                tracing::warn!("starting the appearance thread failed: {err}");
            }
            Reader {
                sender,
                frame,
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

    pub fn frame_sender() -> &'static watch::Sender<SystemFrame> {
        &reader().frame
    }

    /// Reads the portal's settings into `sender` and `frame`, telling `first` once it
    /// has, then follows their changes until the session bus goes away.
    async fn follow(
        sender: &watch::Sender<SystemDark>,
        frame: &watch::Sender<SystemFrame>,
        first: &mpsc::Sender<()>,
    ) -> zbus::Result<()> {
        let connection = Connection::session().await?;
        let proxy = Proxy::new(&connection, PORTAL, PATH, SETTINGS).await?;
        // Subscribed before reading, so a change between the two isn't missed. A bus
        // that refuses the subscription may still answer the read: the mode then shows
        // as it was at launch.
        let changes = proxy.receive_signal("SettingChanged").await;
        // A portal without the setting yet may set it later: keep listening.
        match read(&proxy).await {
            Ok(dark) => {
                sender.send_replace(dark);
            }
            Err(err) => tracing::info!("the system's dark mode can't be read: {err}"),
        }
        // Read once: later changes come with their values. No call is made while the
        // changes are followed, as its answer would wait behind a burst of them (a new
        // color scheme changes many keys at once) once their queue is full.
        let mut groups = proxy
            .call::<_, _, Groups>("ReadAll", &(FRAME_NAMESPACES.as_slice(),))
            .await
            .unwrap_or_else(|err| {
                tracing::info!("the system's window frame can't be read: {err}");
                Groups::new()
            });
        let send_frame = |groups: &Groups| {
            let now = kde_frame(groups).unwrap_or(SystemFrame::UNKNOWN);
            frame.send_if_modified(|was| std::mem::replace(was, now) != now);
        };
        send_frame(&groups);
        let _ = first.send(());
        let mut changes = changes?;
        while let Some(message) = changes.next().await {
            let Ok((namespace, key, value)) =
                message.body().deserialize::<(String, String, OwnedValue)>()
            else {
                continue;
            };
            if namespace == NAMESPACE && key == KEY {
                sender.send_replace(dark(&value));
            } else if FRAME_NAMESPACES.contains(&namespace.as_str()) {
                groups.entry(namespace).or_default().insert(key, value);
                send_frame(&groups);
            }
        }
        Ok(())
    }

    /// The setting now. `ReadOne` is the portal's call since version 2; older ones have
    /// only `Read`, which wraps the value in one more variant.
    async fn read(proxy: &Proxy<'_>) -> zbus::Result<SystemDark> {
        match proxy
            .call::<_, _, OwnedValue>("ReadOne", &(NAMESPACE, KEY))
            .await
        {
            Ok(value) => Ok(dark(&value)),
            Err(zbus::Error::MethodError(name, ..)) if name.as_str() == UNKNOWN_METHOD => {
                let value: OwnedValue = proxy.call("Read", &(NAMESPACE, KEY)).await?;
                Ok(dark(&value))
            }
            Err(err) => Err(err),
        }
    }

    /// The frame Breeze draws, from `kdeglobals`' `groups`: an outline of the window's
    /// Frame color mixed toward the text color by `[KDE] frameContrast`, and Breeze's
    /// corners. `None` without KDE's colors.
    ///
    /// The Frame color is KWin's: the Header colors' background where the scheme has
    /// them, else `[WM] frame`, else the window background. An inactive window's
    /// outline is taken as the same, as it is without Header colors.
    fn kde_frame(groups: &Groups) -> Option<SystemFrame> {
        let entry = |group: &str, key: &str| {
            groups
                .get(&format!("{KDEGLOBALS}.{group}"))
                .and_then(|g| g.get(key))
                .and_then(|v| string(v))
        };
        let color = |group: &str, key: &str| entry(group, key).and_then(rgb);
        let background = color("Colors:Window", "BackgroundNormal")?;
        let frame = if groups.contains_key(&format!("{KDEGLOBALS}.Colors:Header")) {
            color("Colors:Header", "BackgroundNormal").unwrap_or(background)
        } else {
            color("WM", "frame").unwrap_or(background)
        };
        let text = color("Colors:View", "ForegroundNormal")?;
        let contrast = entry("KDE", "frameContrast")
            .and_then(|c| c.trim().parse::<f64>().ok())
            .unwrap_or(FRAME_CONTRAST)
            .clamp(0.0, 1.0);
        Some(SystemFrame {
            outline: Some(mix(frame, text, contrast)),
            radius: BREEZE_RADIUS,
            buttons: Buttons::Breeze,
        })
    }

    /// A string setting, in a variant or not.
    fn string<'a>(value: &'a Value<'a>) -> Option<&'a str> {
        match value {
            Value::Value(inner) => string(inner),
            Value::Str(s) => Some(s.as_str()),
            _ => None,
        }
    }

    /// A KDE color, `r,g,b` (an alpha after them is left out).
    fn rgb(text: &str) -> Option<[u8; 3]> {
        let mut parts = text.split(',').map(|p| p.trim().parse::<u8>());
        let mut next = || parts.next()?.ok();
        Some([next()?, next()?, next()?])
    }

    /// `a` moved toward `b` by `amount`, channel by channel, as `KColorUtils::mix`.
    fn mix(a: [u8; 3], b: [u8; 3], amount: f64) -> [u8; 3] {
        let channel = |i: usize| {
            let (a, b) = (f64::from(a[i]), f64::from(b[i]));
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                reason = "within 0 to 255, as both ends are"
            )]
            let mixed = (a + (b - a) * amount).round() as u8;
            mixed
        };
        [channel(0), channel(1), channel(2)]
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

        use super::{Groups, SystemFrame, dark, kde_frame, mix, rgb};

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

        fn groups(entries: &[(&str, &str, &str)]) -> Groups {
            let mut groups = Groups::new();
            for (group, key, value) in entries {
                groups
                    .entry(format!("org.kde.kdeglobals.{group}"))
                    .or_default()
                    .insert((*key).to_owned(), Value::from(*value).try_into().unwrap());
            }
            groups
        }

        #[test]
        fn kde_colors_read_as_r_g_b() {
            assert_eq!(rgb("36,39,44"), Some([36, 39, 44]));
            assert_eq!(rgb("36, 39, 44, 255"), Some([36, 39, 44]));
            assert_eq!(rgb("36,39"), None);
            assert_eq!(rgb("#24272c"), None);
            assert_eq!(mix([0, 100, 255], [255, 100, 0], 0.2), [51, 100, 204]);
        }

        #[test]
        fn the_outline_mixes_the_frame_toward_the_text_as_breeze_does() {
            // The Steam Deck's Vapor scheme: [WM] without a frame color, no Header.
            let vapor = groups(&[
                ("Colors:Window", "BackgroundNormal", "36,39,44"),
                ("Colors:View", "ForegroundNormal", "239,240,241"),
                ("WM", "activeBackground", "36,39,47"),
            ]);
            let frame = kde_frame(&vapor).unwrap();
            assert_eq!(frame.outline, Some([77, 79, 83]));
            assert!((frame.radius - 5.0).abs() < f32::EPSILON);
            assert_eq!(frame.buttons, super::Buttons::Breeze);

            // Header colors come first, and frameContrast sets the mix.
            let header = groups(&[
                ("Colors:Window", "BackgroundNormal", "239,240,241"),
                ("Colors:Header", "BackgroundNormal", "222,224,226"),
                ("Colors:View", "ForegroundNormal", "35,38,41"),
                ("WM", "frame", "0,0,0"),
                ("KDE", "frameContrast", "0.5"),
            ]);
            assert_eq!(kde_frame(&header).unwrap().outline, Some([129, 131, 134]));

            // Not KDE.
            assert_eq!(kde_frame(&Groups::new()), None);
            assert_eq!(SystemFrame::UNKNOWN.outline, None);
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
