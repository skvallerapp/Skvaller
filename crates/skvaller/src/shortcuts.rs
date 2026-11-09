//! Single-key shortcuts, as the Tauri version has them (its `shortcuts.ts`).
//!
//! Keys are named as [`key_name`] returns them: `j`, ` ` (Space), `Shift+ `, `Enter`.
//! The user can rebind every action; config.json keeps only the keys that differ from
//! the defaults, by action name.

use std::collections::{BTreeMap, HashMap};

use dioxus_native::prelude::{Key, Modifiers};

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
    const ALL: [Self; 10] = [
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

    /// The action's name in config.json.
    const fn name(self) -> &'static str {
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

/// The action of each key, with the user's `overrides` (`None` for no key) applied.
pub fn bindings(overrides: &BTreeMap<String, Option<String>>) -> HashMap<String, Action> {
    let mut map = HashMap::new();
    for action in Action::ALL {
        let key = match overrides.get(action.name()) {
            // An arrow key saved before the arrows were reserved does nothing: the default.
            Some(Some(key)) if ARROWS.contains(&key.as_str()) => Some(action.default_key()),
            Some(key) => key.as_deref(),
            None => Some(action.default_key()),
        };
        if let Some(key) = key {
            map.insert(key.to_owned(), action);
        }
    }
    map
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
    use dioxus_native::prelude::NamedKey;

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
}
