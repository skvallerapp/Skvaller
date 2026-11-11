# Skvaller

A desktop RSS reader (Windows, macOS, Linux).
It is a client for a sync service through the Google Reader API, with FreshRSS as the
first target. It also has a local account that fetches feeds itself, and several accounts
can be connected at once.

## Stack

Skvaller is being rebuilt as an all-Rust app. The Tauri + Svelte version is kept, frozen,
on the `tauri` branch: use it as the reference for features, text and styling, and don't
commit to it unless asked.

- `skvaller-core`: a Rust crate for sync, storage (SQLite via `rusqlite`) and the
  Google Reader API client. It contains no UI code.
- `skvaller-app`: config.json, the accounts' caches and the background sync, without a
  UI. Same files and folders as the Tauri version, so its accounts carry over.
- `skvaller`: the window, in Dioxus Native (Blitz). The article renders in an
  `<iframe srcdoc>`, its own document. `--import-opml <file>` adds test feeds to a
  local account.
- `locales/`: the app's text, one JSON file per language, carried over from the Tauri
  version. `en.json` is the source.

## Architecture rules

- **The UI never waits for the network.** The UI reads only from the local
  SQLite cache. Sync runs in the background and updates the cache.
- **Keep `skvaller-core` free of UI code.** It exposes data and commands and is tested
  on its own.
- **Article HTML is untrusted.** It never runs scripts, and its styles must not affect
  the rest of the app.
- **Never store the user's password.** Store only the access token, in the OS
  keychain, under `KEYCHAIN_SERVICE`. Keep that name unchanged.
- **Queued offline changes win.** The server never overwrites a queued local
  read or star change.
- **Reference platforms.** macOS sets the bar for polish. Linux sets the bar for
  performance; the Steam Deck is the low-power test machine. The article list must
  scroll smoothly with 5,000+ items.
- **Screen reader support** is a goal, but not a blocker for shipping a feature.

## On the `bench` branch

This branch holds the Tauri vs all-Rust CPU and memory test. Read `bench/README.md` and follow
it; build the apps from `main` and `tauri` worktrees, not from this branch.
