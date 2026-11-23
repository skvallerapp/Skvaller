//! The app's keys: the shortcuts, the app's chords, the system's chords a shortcut
//! can't take, and moving a menu's highlight.
//!
//! Keys are named as [`key_name`] returns them: `j`, ` ` (Space), `Shift+ `, `Enter`,
//! `Ctrl+Alt+k`. The user can rebind every action, to any key the app or the system
//! doesn't use already; config.json keeps only the keys that differ from the defaults,
//! by action name.

use std::collections::{BTreeMap, HashMap};

use dioxus_native::prelude::{
    Code, Event, Key, KeyboardData, Modifiers, ModifiersInteraction, NamedKey,
};
use skvaller_app::FontSize;

use crate::chrome::MAC;
use crate::i18n::t;

/// The modifier of the app's own shortcuts, such as Settings: ⌘ on macOS, Ctrl elsewhere.
pub const COMMAND_MODIFIER: Modifiers = if cfg!(target_os = "macos") {
    Modifiers::META
} else {
    Modifiers::CONTROL
};

/// The modifier of the account shortcuts, ⌘ on macOS and Alt elsewhere.
pub const ACCOUNT_MODIFIER: Modifiers = if cfg!(target_os = "macos") {
    Modifiers::META
} else {
    Modifiers::ALT
};

/// The first nine accounts' keys, with [`ACCOUNT_MODIFIER`]: 1 to 9. They go by the
/// key's place, not its character, which depends on the layout.
pub const ACCOUNT_KEYS: [Code; 9] = [
    Code::Digit1,
    Code::Digit2,
    Code::Digit3,
    Code::Digit4,
    Code::Digit5,
    Code::Digit6,
    Code::Digit7,
    Code::Digit8,
    Code::Digit9,
];

/// The account a key switches to, by index: ⌘1 to ⌘9 on macOS, Alt+1 to Alt+9
/// elsewhere. On macOS the menu bar takes the key first, except where the layout's
/// digits need Shift.
fn account_index(code: Code, mods: Modifiers) -> Option<usize> {
    if mods != ACCOUNT_MODIFIER {
        return None;
    }
    ACCOUNT_KEYS.iter().position(|&key| key == code)
}

/// How the font size changes with ⌘+, ⌘− and ⌘0.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontStep {
    Larger,
    Smaller,
    Reset,
}

impl FontStep {
    /// The change `key` asks for with the command modifier, and Shift if `shift`.
    /// Goes by the typed character, so it follows the layout. `+` needs Shift and `=`
    /// on some layouts, so `=` alone counts as `+`, as in browsers. Only `+` may come
    /// with Shift: Shift and `0` types `=` on a Swedish keyboard.
    fn of(key: &Key, shift: bool) -> Option<Self> {
        let Key::Character(c) = key else {
            return None;
        };
        match (c.as_str(), shift) {
            ("+", _) | ("=", false) => Some(Self::Larger),
            ("-", false) => Some(Self::Smaller),
            ("0", false) => Some(Self::Reset),
            _ => None,
        }
    }

    /// The size after this change of `size`.
    pub fn apply(self, size: FontSize) -> FontSize {
        match self {
            Self::Larger => size.larger(),
            Self::Smaller => size.smaller(),
            Self::Reset => FontSize::M,
        }
    }
}

/// One of the app's own chords, which no shortcut can take.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chord {
    Settings,
    Search,
    ToggleSidebar,
    FontSize(FontStep),
    /// The account at this place in the list.
    Account(usize),
}

/// The app's chord `key` makes with `mods`, if any.
pub fn app_chord(key: &Key, code: Code, mods: Modifiers) -> Option<Chord> {
    if mods == COMMAND_MODIFIER {
        match code {
            Code::Comma => return Some(Chord::Settings),
            Code::KeyF => return Some(Chord::Search),
            _ => {}
        }
    }
    if mods == COMMAND_MODIFIER | Modifiers::SHIFT && code == Code::KeyS {
        return Some(Chord::ToggleSidebar);
    }
    if mods - Modifiers::SHIFT == COMMAND_MODIFIER
        && let Some(step) = FontStep::of(key, mods.contains(Modifiers::SHIFT))
    {
        return Some(Chord::FontSize(step));
    }
    account_index(code, mods).map(Chord::Account)
}

/// Whether a key named `name` is one of the system's chords that come to the window:
/// editing text, and closing the window on Windows and Linux. The others, such as ⌘Q
/// and Alt+Tab, the system or the menu bar takes first. On Windows and Linux every
/// chord with the Windows or Super key is the system's.
fn system_chord(name: &str) -> bool {
    let (mods, key) = split(name);
    if !MAC && mods.contains(Modifiers::META) {
        return true;
    }
    let command = mods - Modifiers::SHIFT == COMMAND_MODIFIER;
    let shift = mods.contains(Modifiers::SHIFT);
    match key {
        "c" | "v" | "x" => command && !shift,
        // Select all, and with Shift, as in Blitz's fields, select none; undo, and redo
        // with Shift.
        "a" | "z" => command,
        "y" => command && !shift && !MAC,
        "Home" | "End" => command && !MAC,
        // Closing the window, and its window menu.
        "F4" | " " => !MAC && mods == Modifiers::ALT,
        // Moving by word or line in text, and between spaces on macOS.
        _ if ARROWS.contains(&key) => mods.intersects(CHORD),
        _ => false,
    }
}

/// The modifiers that make a key a chord.
const CHORD: Modifiers = Modifiers::CONTROL
    .union(Modifiers::ALT)
    .union(Modifiers::META);

/// The modifiers in key names, in the order they come there and in labels: each one's
/// name there, its symbol on macOS and its text key elsewhere.
const MODIFIERS: [(Modifiers, &str, char, &str); 4] = [
    (Modifiers::CONTROL, "Ctrl", '⌃', "keys.ctrl"),
    (Modifiers::ALT, "Alt", '⌥', "keys.alt"),
    (Modifiers::SHIFT, "Shift", '⇧', "keys.shift"),
    // The Windows or Super key off macOS, where it only appears in a config.json
    // saved on a Mac.
    (Modifiers::META, "Meta", '⌘', "keys.super"),
];

/// A key name's modifiers, and the key without them.
fn split(name: &str) -> (Modifiers, &str) {
    let mut mods = Modifiers::empty();
    let mut key = name;
    for (modifier, prefix, ..) in MODIFIERS {
        if let Some(rest) = key.strip_prefix(prefix).and_then(|k| k.strip_prefix('+')) {
            mods |= modifier;
            key = rest;
        }
    }
    (mods, key)
}

/// Where `key` moves the highlight of a menu of `count` items, highlighted at `at`:
/// Down and Up step and wrap around, starting at the first or the last when none is
/// highlighted; Home and End go to the ends. `None` for other keys, and in an empty
/// menu.
pub fn highlight(at: Option<usize>, count: usize, key: &Key) -> Option<usize> {
    let last = count.checked_sub(1)?;
    let at = at.filter(|&i| i <= last);
    match key {
        Key::Named(NamedKey::ArrowDown) => {
            Some(at.map_or(0, |i| if i == last { 0 } else { i + 1 }))
        }
        Key::Named(NamedKey::ArrowUp) => {
            Some(at.map_or(last, |i| i.checked_sub(1).unwrap_or(last)))
        }
        Key::Named(NamedKey::Home) => Some(0),
        Key::Named(NamedKey::End) => Some(last),
        _ => None,
    }
}

/// Something a shortcut does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Action {
    Next,
    Previous,
    ReadOn,
    ReadBack,
    ToggleRead,
    ToggleStar,
    OpenInBrowser,
    ToggleReaderView,
    Sync,
    MarkAllRead,
}

impl Action {
    /// Every action, in the order Settings lists them.
    pub const ALL: [Self; 10] = [
        Self::Next,
        Self::Previous,
        Self::ReadOn,
        Self::ReadBack,
        Self::ToggleRead,
        Self::ToggleStar,
        Self::OpenInBrowser,
        Self::ToggleReaderView,
        Self::Sync,
        Self::MarkAllRead,
    ];

    /// The action's name in config.json, and in its label's text key.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Next => "next",
            Self::Previous => "previous",
            Self::ReadOn => "readOn",
            Self::ReadBack => "readBack",
            Self::ToggleRead => "toggleRead",
            Self::ToggleStar => "toggleStar",
            Self::OpenInBrowser => "openInBrowser",
            Self::ToggleReaderView => "toggleReaderView",
            Self::Sync => "sync",
            Self::MarkAllRead => "markAllRead",
        }
    }

    const fn default_key(self) -> &'static str {
        match self {
            Self::Next => "j",
            Self::Previous => "k",
            Self::ReadOn => " ",
            Self::ReadBack => "Shift+ ",
            Self::ToggleRead => "m",
            Self::ToggleStar => "s",
            Self::OpenInBrowser => "b",
            Self::ToggleReaderView => "v",
            Self::Sync => "r",
            Self::MarkAllRead => "a",
        }
    }

    /// Whether the action runs again while its key is held down, rather than once.
    pub const fn repeats(self) -> bool {
        matches!(
            self,
            Self::Next | Self::Previous | Self::ReadOn | Self::ReadBack
        )
    }
}

/// The arrow keys, which move in the sidebar and the list and can't be bound.
pub const ARROWS: [&str; 4] = ["ArrowUp", "ArrowDown", "ArrowLeft", "ArrowRight"];

/// The user's keys that differ from the defaults, by action name; `None` for no key.
pub type Overrides = BTreeMap<String, Option<String>>;

/// The key of `action`, with the user's `overrides` applied; `None` for no key.
pub fn key_of(overrides: &Overrides, action: Action) -> Option<&str> {
    match overrides.get(action.name()) {
        // A saved key that can't be a shortcut, such as a system chord saved on another
        // platform (Ctrl+C from a Mac) or a hand-edited Enter: the default applies.
        Some(Some(key)) if !bindable(key) => Some(action.default_key()),
        Some(key) => key.as_deref(),
        None => Some(action.default_key()),
    }
}

/// Whether the key named `name` can be a shortcut: not an arrow, which moves; not a
/// system chord; and not Enter, which also presses the focused button. [`capture`]
/// refuses these, each with its reason.
fn bindable(name: &str) -> bool {
    !ARROWS.contains(&name) && !system_chord(name) && name != "Enter" && name != "Shift+Enter"
}

/// The action of each key, with the user's `overrides` applied.
pub fn bindings(overrides: &Overrides) -> HashMap<String, Action> {
    Action::ALL
        .into_iter()
        .filter_map(|action| Some((key_of(overrides, action)?.to_owned(), action)))
        .collect()
}

/// Gives `action` the `key`, or no key for `None`.
///
/// An action that had the key loses it. Returns the new overrides, without entries
/// that match the defaults, and the action that lost its key, if any.
pub fn rebind(
    overrides: &Overrides,
    action: Action,
    key: Option<&str>,
) -> (Overrides, Option<Action>) {
    let mut displaced = None;
    let mut result = Overrides::new();
    for other in Action::ALL {
        let mut current = key_of(overrides, other);
        if other == action {
            current = key;
        } else if key.is_some() && current == key {
            current = None;
            displaced = Some(other);
        }
        if current != Some(other.default_key()) {
            result.insert(other.name().to_owned(), current.map(str::to_owned));
        }
    }
    (result, displaced)
}

/// What a key pressed while Settings waits for an action's new key comes to.
#[derive(Debug, PartialEq, Eq)]
pub enum Capture {
    /// The key, by its name.
    Key(String),
    /// No key for the action: Backspace or Delete.
    Clear,
    /// Stop waiting and leave the action as it was: Escape.
    Cancel,
    /// A modifier on its own: wait for the rest.
    Wait,
    /// Not a key a shortcut can have, and why.
    Refuse(Refusal),
}

/// Why a key can't be a shortcut.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// One of the app's or the system's chords, by its name.
    Taken(String),
    /// An arrow key.
    Arrow,
    /// Enter, which presses the focused button.
    Enter,
    /// A key without a name, such as a dead key, or Option and a key other than a
    /// letter, a digit or Space on macOS.
    Unnamed,
}

/// What a key pressed while waiting for a new key comes to.
pub fn capture(key: &Key, code: Code, mods: Modifiers) -> Capture {
    match key {
        Key::Named(NamedKey::Escape) => return Capture::Cancel,
        Key::Named(NamedKey::Backspace | NamedKey::Delete) => return Capture::Clear,
        _ => {}
    }
    let Some(name) = key_name(key, code, mods) else {
        return if matches!(key, Key::Named(named) if is_modifier(*named)) {
            Capture::Wait
        } else {
            Capture::Refuse(Refusal::Unnamed)
        };
    };
    if app_chord(key, code, mods).is_some() || system_chord(&name) {
        Capture::Refuse(Refusal::Taken(name))
    } else if ARROWS.contains(&name.as_str()) {
        Capture::Refuse(Refusal::Arrow)
    } else if name == "Enter" || name == "Shift+Enter" {
        Capture::Refuse(Refusal::Enter)
    } else {
        Capture::Key(name)
    }
}

/// A key that only changes others, pressed on its way to a chord.
fn is_modifier(key: NamedKey) -> bool {
    matches!(
        key,
        NamedKey::Shift
            | NamedKey::Control
            | NamedKey::Alt
            | NamedKey::Meta
            | NamedKey::AltGraph
            | NamedKey::CapsLock
            | NamedKey::Fn
    )
}

/// A key pressed in a text field: `None` for one of the app's chords, which goes on to
/// the window; otherwise the key, kept from the window, since it is the field's.
pub fn field_key(e: &Event<KeyboardData>) -> Option<Key> {
    let key = e.key();
    if app_chord(&key, e.code(), e.modifiers()).is_some() {
        return None;
    }
    e.stop_propagation();
    Some(key)
}

/// How a key name shows in Settings: `⌃⇧Space` on macOS, `Ctrl+Shift+Space` elsewhere.
pub fn key_label(name: &str, mac: bool) -> String {
    let (mods, key) = split(name);
    let label = match key {
        " " => t("keys.space"),
        "Enter" => t(if mac { "keys.return" } else { "keys.enter" }),
        "ArrowUp" => "↑".to_owned(),
        "ArrowDown" => "↓".to_owned(),
        "ArrowLeft" => "←".to_owned(),
        "ArrowRight" => "→".to_owned(),
        _ if key.chars().count() == 1 => key.to_uppercase(),
        _ => key.to_owned(),
    };
    let mut shown = String::new();
    for (modifier, _, symbol, text) in MODIFIERS {
        if !mods.contains(modifier) {
            continue;
        }
        if mac {
            shown.push(symbol);
        } else {
            shown.push_str(&t(text));
            shown.push('+');
        }
    }
    shown + &label
}

/// How `key` with `mods` shows on this platform, as [`key_label`] shows a key name:
/// `⌘F` on macOS, `Ctrl+F` elsewhere. An empty `key` shows the modifiers alone.
pub fn chord_label(mods: Modifiers, key: &str) -> String {
    let mut name = String::new();
    for (modifier, prefix, ..) in MODIFIERS {
        if mods.contains(modifier) {
            name.push_str(prefix);
            name.push('+');
        }
    }
    key_label(&(name + key), MAC)
}

/// The key's name as a shortcut, with its modifiers in front in the order of
/// [`MODIFIERS`], or `None` when it can't be one: a modifier on its own, or a dead or
/// unnamed key.
pub fn key_name(key: &Key, code: Code, mods: Modifiers) -> Option<String> {
    let code = code.to_string();
    let code = code.as_str();
    let mut mods = mods & (CHORD | Modifiers::SHIFT);
    let name = match key {
        // Option on its own changes the character on macOS: with it, a letter, a digit
        // or Space goes by its place. With Ctrl or Cmd the character stays the key's.
        Key::Character(_) if MAC && mods & CHORD == Modifiers::ALT => match code {
            "Space" => " ".to_owned(),
            _ => code
                .strip_prefix("Key")
                .or_else(|| code.strip_prefix("Digit"))?
                .to_lowercase(),
        },
        Key::Character(text) => {
            let mut chars = text.chars();
            let (Some(c), None) = (chars.next(), chars.next()) else {
                return None;
            };
            // With a non-Latin layout, such as Cyrillic, a letter key goes by its Latin
            // letter, so J stays J whichever layout is on.
            let c = match code.strip_prefix("Key") {
                Some(latin) if c.is_alphabetic() && !is_latin(c) => {
                    latin.chars().next().unwrap_or(c)
                }
                _ => c,
            };
            // Letters match with Caps Lock on too.
            let lower: String = c.to_lowercase().collect();
            // A shifted symbol is its own character already: `!` rather than `Shift+1`.
            if c != ' ' && lower.to_uppercase() == lower {
                mods.remove(Modifiers::SHIFT);
            }
            lower
        }
        Key::Named(named) => {
            if is_modifier(*named)
                || matches!(
                    named,
                    NamedKey::Dead | NamedKey::Process | NamedKey::Unidentified
                )
            {
                return None;
            }
            named.to_string()
        }
    };
    let mut full = String::new();
    for (modifier, prefix, ..) in MODIFIERS {
        if mods.contains(modifier) {
            full.push_str(prefix);
            full.push('+');
        }
    }
    Some(full + &name)
}

/// A letter of the Latin script: ASCII and the Latin-1, Extended-A/B and Extended
/// Additional blocks.
fn is_latin(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, '\u{C0}'..='\u{24F}' | '\u{1E00}'..='\u{1EFF}')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[expect(clippy::needless_pass_by_value, reason = "keeps the cases short")]
    fn name(key: Key, code: &str, mods: Modifiers) -> Option<String> {
        key_name(&key, code.parse().unwrap(), mods)
    }

    fn ch(c: &str) -> Key {
        Key::Character(c.to_owned())
    }

    #[test]
    fn letters_match_with_caps_lock_shift_and_other_layouts() {
        assert_eq!(
            name(ch("j"), "KeyJ", Modifiers::empty()).as_deref(),
            Some("j")
        );
        // Caps Lock: the character is upper case, without Shift.
        assert_eq!(
            name(ch("J"), "KeyJ", Modifiers::empty()).as_deref(),
            Some("j")
        );
        assert_eq!(
            name(ch("J"), "KeyJ", Modifiers::SHIFT).as_deref(),
            Some("Shift+j")
        );
        // Cyrillic о on the J key.
        assert_eq!(
            name(ch("о"), "KeyJ", Modifiers::empty()).as_deref(),
            Some("j")
        );
        // A Latin letter outside ASCII stays itself.
        assert_eq!(
            name(ch("å"), "BracketLeft", Modifiers::empty()).as_deref(),
            Some("å")
        );
    }

    #[test]
    fn space_symbols_and_named_keys() {
        assert_eq!(
            name(ch(" "), "Space", Modifiers::empty()).as_deref(),
            Some(" ")
        );
        assert_eq!(
            name(ch(" "), "Space", Modifiers::SHIFT).as_deref(),
            Some("Shift+ ")
        );
        assert_eq!(
            name(ch("!"), "Digit1", Modifiers::SHIFT).as_deref(),
            Some("!")
        );
        assert_eq!(
            name(
                Key::Named(NamedKey::ArrowDown),
                "ArrowDown",
                Modifiers::empty()
            )
            .as_deref(),
            Some("ArrowDown")
        );
        assert_eq!(
            name(Key::Named(NamedKey::Shift), "ShiftLeft", Modifiers::SHIFT),
            None
        );
    }

    #[test]
    fn overrides_replace_or_remove_a_default_but_never_take_an_arrow() {
        let overrides = BTreeMap::from([
            ("next".to_owned(), Some("n".to_owned())),
            ("sync".to_owned(), None),
            ("previous".to_owned(), Some("ArrowUp".to_owned())),
        ]);
        let map = bindings(&overrides);
        assert_eq!(map.get("n"), Some(&Action::Next));
        assert_eq!(map.get("j"), None);
        assert_eq!(map.get("r"), None);
        assert_eq!(map.get("k"), Some(&Action::Previous));
        assert_eq!(map.get(" "), Some(&Action::ReadOn));
    }

    fn over(entries: &[(&str, Option<&str>)]) -> Overrides {
        entries
            .iter()
            .map(|&(a, k)| (a.to_owned(), k.map(str::to_owned)))
            .collect()
    }

    #[test]
    fn rebinding_moves_a_taken_key_and_keeps_only_changes() {
        // `k` belongs to Previous, which loses it.
        let (result, displaced) = rebind(&Overrides::new(), Action::Next, Some("k"));
        assert_eq!(displaced, Some(Action::Previous));
        assert_eq!(result, over(&[("next", Some("k")), ("previous", None)]));
        // Back to the default: the entry goes.
        let (result, displaced) = rebind(&result, Action::Next, Some("j"));
        assert_eq!(displaced, None);
        assert_eq!(result, over(&[("previous", None)]));
        // No key for an action.
        let (result, _) = rebind(&Overrides::new(), Action::Sync, None);
        assert_eq!(result, over(&[("sync", None)]));
        // A stale arrow override counts as the default, and is dropped.
        let (result, _) = rebind(&over(&[("next", Some("ArrowUp"))]), Action::Sync, Some("y"));
        assert_eq!(result, over(&[("sync", Some("y"))]));
    }

    #[test]
    fn chords_show_as_the_platform_writes_them() {
        if MAC {
            assert_eq!(chord_label(COMMAND_MODIFIER, "f"), "⌘F");
            assert_eq!(chord_label(ACCOUNT_MODIFIER, ""), "⌘");
        } else {
            assert_eq!(
                chord_label(COMMAND_MODIFIER, ","),
                format!("{}+,", t("keys.ctrl"))
            );
            assert_eq!(
                chord_label(ACCOUNT_MODIFIER, ""),
                format!("{}+", t("keys.alt"))
            );
        }
    }

    #[test]
    fn a_saved_system_chord_falls_back_to_the_default() {
        let command = if MAC { "Meta" } else { "Ctrl" };
        let copy = format!("{command}+c");
        let overrides = over(&[("sync", Some(copy.as_str()))]);
        assert_eq!(
            key_of(&overrides, Action::Sync),
            Some(Action::Sync.default_key())
        );
        let overrides = over(&[("sync", Some("Enter"))]);
        assert_eq!(
            key_of(&overrides, Action::Sync),
            Some(Action::Sync.default_key())
        );
    }

    #[test]
    fn capture_takes_chords_the_app_and_the_system_leave_free() {
        let none = Modifiers::empty();
        let key = |name: &str| Capture::Key(name.to_owned());
        let taken = |name: &str| Capture::Refuse(Refusal::Taken(name.to_owned()));
        assert_eq!(capture(&ch("n"), Code::KeyN, none), key("n"));
        assert_eq!(
            capture(&ch(" "), Code::Space, Modifiers::SHIFT),
            key("Shift+ ")
        );
        assert_eq!(
            capture(&Key::Named(NamedKey::Escape), Code::Escape, none),
            Capture::Cancel
        );
        assert_eq!(
            capture(&Key::Named(NamedKey::Delete), Code::Delete, none),
            Capture::Clear
        );
        assert_eq!(
            capture(&Key::Named(NamedKey::Backspace), Code::Backspace, none),
            Capture::Clear
        );
        // A modifier on its own waits for the rest.
        assert_eq!(
            capture(
                &Key::Named(NamedKey::Shift),
                Code::ShiftLeft,
                Modifiers::SHIFT
            ),
            Capture::Wait
        );
        assert_eq!(
            capture(
                &Key::Named(NamedKey::Control),
                Code::ControlLeft,
                Modifiers::CONTROL
            ),
            Capture::Wait
        );
        assert_eq!(
            capture(&ch("k"), Code::KeyK, Modifiers::CONTROL | Modifiers::SHIFT),
            key("Ctrl+Shift+k")
        );
        // The app's chords, and the system's for editing text.
        let command = if MAC { "Meta" } else { "Ctrl" };
        assert_eq!(
            capture(&ch("f"), Code::KeyF, COMMAND_MODIFIER),
            taken(&format!("{command}+f"))
        );
        assert_eq!(
            capture(&ch("0"), Code::Digit0, COMMAND_MODIFIER),
            taken(&format!("{command}+0"))
        );
        let account = if MAC { "Meta" } else { "Alt" };
        assert_eq!(
            capture(&ch("3"), Code::Digit3, ACCOUNT_MODIFIER),
            taken(&format!("{account}+3"))
        );
        assert_eq!(
            capture(&ch("c"), Code::KeyC, COMMAND_MODIFIER),
            taken(&format!("{command}+c"))
        );
        assert_eq!(
            capture(&ch("Z"), Code::KeyZ, COMMAND_MODIFIER | Modifiers::SHIFT),
            taken(if MAC { "Shift+Meta+z" } else { "Ctrl+Shift+z" })
        );
        assert_eq!(
            capture(
                &Key::Named(NamedKey::ArrowLeft),
                Code::ArrowLeft,
                Modifiers::ALT
            ),
            taken("Alt+ArrowLeft")
        );
        assert_eq!(
            capture(&Key::Named(NamedKey::ArrowLeft), Code::ArrowLeft, none),
            Capture::Refuse(Refusal::Arrow)
        );
        assert_eq!(
            capture(&Key::Named(NamedKey::Enter), Code::Enter, Modifiers::SHIFT),
            Capture::Refuse(Refusal::Enter)
        );
        assert_eq!(
            capture(&Key::Named(NamedKey::Dead), Code::KeyE, none),
            Capture::Refuse(Refusal::Unnamed)
        );
        assert_eq!(
            capture(&ch("a"), Code::KeyA, COMMAND_MODIFIER | Modifiers::SHIFT),
            taken(if MAC { "Shift+Meta+a" } else { "Ctrl+Shift+a" })
        );
        if !MAC {
            assert_eq!(
                capture(&ch(" "), Code::Space, Modifiers::ALT),
                taken("Alt+ ")
            );
            assert_eq!(
                capture(&Key::Named(NamedKey::F4), Code::F4, Modifiers::ALT),
                taken("Alt+F4")
            );
            assert_eq!(
                capture(&ch("j"), Code::KeyJ, Modifiers::META),
                taken("Meta+j")
            );
        }
    }

    #[test]
    fn chords_name_their_modifiers_in_order() {
        assert_eq!(
            name(ch("j"), "KeyJ", Modifiers::CONTROL).as_deref(),
            Some("Ctrl+j")
        );
        assert_eq!(
            name(ch("j"), "KeyJ", Modifiers::META | Modifiers::CONTROL).as_deref(),
            Some("Ctrl+Meta+j")
        );
        // A shifted symbol drops Shift with a chord too.
        assert_eq!(
            name(ch("!"), "Digit1", Modifiers::CONTROL | Modifiers::SHIFT).as_deref(),
            Some("Ctrl+!")
        );
        // Option types `∆` on the J key on macOS, makes E a dead key, and has no name
        // with other keys.
        if MAC {
            assert_eq!(
                name(ch("∆"), "KeyJ", Modifiers::ALT).as_deref(),
                Some("Alt+j")
            );
            assert_eq!(
                name(ch("Ø"), "KeyO", Modifiers::ALT | Modifiers::SHIFT).as_deref(),
                Some("Alt+Shift+o")
            );
            assert_eq!(
                name(Key::Named(NamedKey::Dead), "KeyE", Modifiers::ALT),
                None
            );
            assert_eq!(name(ch("≤"), "Comma", Modifiers::ALT), None);
            // With Ctrl, the key's own character.
            assert_eq!(
                name(ch("j"), "KeyJ", Modifiers::CONTROL | Modifiers::ALT).as_deref(),
                Some("Ctrl+Alt+j")
            );
        }
    }

    #[test]
    fn account_shortcuts_go_by_the_digit_key_with_the_platform_modifier() {
        let (mods, other) = if cfg!(target_os = "macos") {
            (Modifiers::META, Modifiers::ALT)
        } else {
            (Modifiers::ALT, Modifiers::META)
        };
        assert_eq!(account_index(Code::Digit1, mods), Some(0));
        assert_eq!(account_index(Code::Digit9, mods), Some(8));
        assert_eq!(account_index(Code::Digit0, mods), None);
        assert_eq!(account_index(Code::KeyA, mods), None);
        assert_eq!(account_index(Code::Digit1, other), None);
        assert_eq!(account_index(Code::Digit1, mods | Modifiers::SHIFT), None);
    }

    #[test]
    fn the_highlight_wraps_and_starts_at_the_end_it_moves_from() {
        let (down, up) = (
            Key::Named(NamedKey::ArrowDown),
            Key::Named(NamedKey::ArrowUp),
        );
        assert_eq!(highlight(None, 3, &down), Some(0));
        assert_eq!(highlight(None, 3, &up), Some(2));
        assert_eq!(highlight(Some(0), 3, &down), Some(1));
        assert_eq!(highlight(Some(2), 3, &down), Some(0));
        assert_eq!(highlight(Some(0), 3, &up), Some(2));
        assert_eq!(highlight(Some(1), 3, &Key::Named(NamedKey::Home)), Some(0));
        assert_eq!(highlight(Some(1), 3, &Key::Named(NamedKey::End)), Some(2));
        // A highlight left past the end by items that went.
        assert_eq!(highlight(Some(5), 3, &down), Some(0));
        assert_eq!(highlight(Some(0), 0, &down), None);
        assert_eq!(highlight(Some(0), 3, &Key::Named(NamedKey::Enter)), None);
    }

    #[test]
    fn labels_name_space_shift_and_arrows() {
        assert_eq!(key_label("j", false), "J");
        assert_eq!(key_label("å", true), "Å");
        assert_eq!(key_label("Shift+ ", true), "⇧Space");
        assert_eq!(key_label("Shift+ ", false), "Shift+Space");
        assert_eq!(key_label("Enter", true), "Return");
        assert_eq!(key_label("ArrowUp", false), "↑");
        assert_eq!(key_label("F5", false), "F5");
        assert_eq!(key_label("Ctrl+Alt+Shift+Meta+k", true), "⌃⌥⇧⌘K");
        assert_eq!(key_label("Ctrl+Alt+k", false), "Ctrl+Alt+K");
        assert_eq!(key_label("Alt++", false), "Alt++");
    }
}
