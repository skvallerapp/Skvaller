// Rust guideline compliant 2026-10-09

/// The app's commands. Each gets an `allow-` permission, and a window can call only the
/// ones its capability lists. Keep in step with `generate_handler!` in main.rs: a
/// command missing here can't be granted, so every call to it is refused.
const COMMANDS: &[&str] = &[
    "session",
    "accounts",
    "sign_in",
    "add_local_account",
    "set_active_account",
    "update_account",
    "remove_account",
    "rebuild_cache",
    "sync_now",
    "library",
    "articles",
    "article_html",
    "full_text",
    "set_feed_reader_view",
    "set_read",
    "set_starred",
    "set_settings",
    "subscribe",
    "unsubscribe",
    "rename_feed",
    "move_feed",
    "create_folder",
    "rename_folder",
    "delete_folder",
    "import_opml",
    "export_opml",
    "open_settings",
    "take_settings_pane",
];

fn main() {
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(COMMANDS)),
    )
    .expect("tauri-build generates the app's context and permissions");
}
