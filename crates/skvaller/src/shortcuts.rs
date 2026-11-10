//! Single-key shortcuts, as the Tauri version has them (its `shortcuts.ts`).
//!
//! Keys are named as [`key_name`] returns them: `j`, ` ` (Space), `Shift+ `, `Enter`.
//! The user can rebind every action; config.json keeps only the keys that differ from
//! the defaults, by action name.

use std::collections::{BTreeMap, HashMap};

use dioxus_native::prelude::{Key, Modifiers, NamedKey};

use crate::i18n::t;

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

    /// Runs again while its key is held down. The others run once.
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
        // An arrow key saved before the arrows were reserved does nothing: the default.
        Some(Some(key)) if ARROWS.contains(&key.as_str()) => Some(action.default_key()),
        Some(key) => key.as_deref(),
        None => Some(action.default_key()),
    }
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
    /// A combination with Cmd, Ctrl or Alt.
    NotSingle,
    /// An arrow key.
    Arrow,
    /// Enter, which presses the focused button.
    Enter,
}

/// What a key pressed while waiting for a new key comes to, as in the Tauri version.
pub fn capture(key: &Key, code: &str, mods: Modifiers) -> Capture {
    match key {
        Key::Named(NamedKey::Escape) => return Capture::Cancel,
        Key::Named(NamedKey::Backspace | NamedKey::Delete) => return Capture::Clear,
        _ => {}
    }
    let Some(name) = key_name(key, code, mods) else {
        return if mods.intersects(Modifiers::META | Modifiers::CONTROL | Modifiers::ALT) {
            Capture::Refuse(Refusal::NotSingle)
        } else {
            Capture::Wait
        };
    };
    if ARROWS.contains(&name.as_str()) {
        Capture::Refuse(Refusal::Arrow)
    } else if name == "Enter" || name == "Shift+Enter" {
        Capture::Refuse(Refusal::Enter)
    } else {
        Capture::Key(name)
    }
}

/// How a key name shows in Settings: `⇧Space` on macOS, `Shift+Space` elsewhere.
pub fn key_label(name: &str, mac: bool) -> String {
    let (shift, key) = match name.strip_prefix("Shift+") {
        Some(key) => (true, key),
        None => (false, name),
    };
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
    match (shift, mac) {
        (false, _) => label,
        (true, true) => format!("⇧{label}"),
        (true, false) => format!("{}+{label}", t("keys.shift")),
    }
}

/// The key's name as a shortcut, or `None` when it can't be one: a modifier on its own,
/// a dead or unnamed key, and combinations with Cmd, Ctrl or Alt, which belong to the
/// system and the menus.
pub fn key_name(key: &Key, code: &str, mods: Modifiers) -> Option<String> {
    if mods.intersects(Modifiers::META | Modifiers::CONTROL | Modifiers::ALT) {
        return None;
    }
    let name = match key {
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
                return Some(lower);
            }
            lower
        }
        Key::Named(named) => {
            let name = named.to_string();
            let ignored = [
                "Shift",
                "Control",
                "Alt",
                "Meta",
                "CapsLock",
                "Fn",
                "AltGraph",
                "Dead",
                "Process",
                "Unidentified",
            ];
            if ignored.contains(&name.as_str()) {
                return None;
            }
            name
        }
    };
    Some(if mods.contains(Modifiers::SHIFT) {
        format!("Shift+{name}")
    } else {
        name
    })
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
        key_name(&key, code, mods)
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
        assert_eq!(name(ch("j"), "KeyJ", Modifiers::CONTROL), None);
        assert_eq!(name(ch("j"), "KeyJ", Modifiers::META), None);
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
    fn capture_takes_single_keys_only() {
        let none = Modifiers::empty();
        assert_eq!(
            capture(&ch("n"), "KeyN", none),
            Capture::Key("n".to_owned())
        );
        assert_eq!(
            capture(&ch(" "), "Space", Modifiers::SHIFT),
            Capture::Key("Shift+ ".to_owned())
        );
        assert_eq!(
            capture(&Key::Named(NamedKey::Escape), "Escape", none),
            Capture::Cancel
        );
        assert_eq!(
            capture(&Key::Named(NamedKey::Delete), "Delete", none),
            Capture::Clear
        );
        assert_eq!(
            capture(&Key::Named(NamedKey::Backspace), "Backspace", none),
            Capture::Clear
        );
        assert_eq!(
            capture(&Key::Named(NamedKey::Shift), "ShiftLeft", Modifiers::SHIFT),
            Capture::Wait
        );
        assert_eq!(
            capture(
                &Key::Named(NamedKey::Control),
                "ControlLeft",
                Modifiers::CONTROL
            ),
            Capture::Refuse(Refusal::NotSingle)
        );
        assert_eq!(
            capture(&ch("j"), "KeyJ", Modifiers::ALT),
            Capture::Refuse(Refusal::NotSingle)
        );
        assert_eq!(
            capture(&Key::Named(NamedKey::ArrowLeft), "ArrowLeft", none),
            Capture::Refuse(Refusal::Arrow)
        );
        assert_eq!(
            capture(&Key::Named(NamedKey::Enter), "Enter", Modifiers::SHIFT),
            Capture::Refuse(Refusal::Enter)
        );
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
    }
}
