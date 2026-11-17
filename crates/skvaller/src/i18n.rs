//! The app's text, from `locales/en.json`.
//!
//! Keys are flat, such as `list.label`. A message that depends on a number has one key
//! per plural form, `<key>_one` and `<key>_other` in English. Placeholders are `{name}`.
//! Only English is loaded until there is a language setting.

use std::collections::HashMap;
use std::sync::LazyLock;

static EN: LazyLock<HashMap<String, String>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../../../locales/en.json"))
        .expect("en.json is a flat JSON object of strings")
});

/// The message for `key`, or the key itself when there is none, so a typo shows.
pub fn t(key: &str) -> String {
    EN.get(key).cloned().unwrap_or_else(|| key.to_owned())
}

/// The message for `key` with each `{name}` filled in from `args`.
pub fn t_with(key: &str, args: &[(&str, &str)]) -> String {
    args.iter().fold(t(key), |text, (name, value)| {
        text.replace(&format!("{{{name}}}"), value)
    })
}

/// A message from elsewhere, such as an error's, as a sentence: a capital first letter
/// and a full stop.
pub fn sentence(text: &str) -> String {
    let mut chars = text.trim().chars();
    let Some(first) = chars.next() else {
        return String::new();
    };
    let mut out: String = first.to_uppercase().chain(chars).collect();
    if !out.ends_with(['.', '!', '?']) {
        out.push('.');
    }
    out
}

/// The plural form of `key` for `count`, with `{count}` filled in.
pub fn t_count(key: &str, count: i64) -> String {
    let form = if count == 1 { "one" } else { "other" };
    t(&format!("{key}_{form}")).replace("{count}", &count.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plurals_pick_the_form_and_fill_in_the_count() {
        assert_eq!(t_count("list.unread", 1), "1 Unread Item");
        assert_eq!(t_count("list.unread", 3), "3 Unread Items");
        assert_eq!(t("no.such.key"), "no.such.key");
        assert_eq!(
            t_with("settings.removeQuestion", &[("account", "News")]),
            "Remove News?"
        );
        assert_eq!(
            sentence("the account was removed"),
            "The account was removed."
        );
    }
}
