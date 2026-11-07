//! The system's languages, for the UI to pick its language from. WKWebView's
//! `navigator.languages` lists the languages the app bundle declares, not the user's,
//! so on macOS the UI can't find them itself. See `systemLanguages` in i18n.svelte.ts.

// Rust guideline compliant 2026-10-09

/// A script that gives a window the system's languages, most wanted first.
///
/// It sets `window.__SKVALLER_SYSTEM_LANGUAGES__` to the BCP 47 tags, such as
/// `["sv-SE", "en-US"]`, before the page's own scripts run.
pub fn script() -> String {
    let tags: Vec<String> = sys_locale::get_locales().collect();
    // JSON is valid JavaScript, and escapes whatever the tags hold.
    format!(
        "window.__SKVALLER_SYSTEM_LANGUAGES__ = {};",
        serde_json::Value::from(tags)
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_script_sets_a_list_of_tags() {
        let script = super::script();
        let json = script
            .strip_prefix("window.__SKVALLER_SYSTEM_LANGUAGES__ = ")
            .and_then(|s| s.strip_suffix(';'))
            .expect("the script assigns the list");
        let tags: Vec<String> = serde_json::from_str(json).expect("the list is JSON");
        assert!(tags.iter().all(|t| !t.is_empty()));
    }
}
