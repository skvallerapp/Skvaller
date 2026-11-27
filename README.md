# Skvaller

An open-source desktop RSS reader for Windows, macOS and Linux.

Skvaller is a client for a sync service. It speaks the **Google Reader API**, so it works
with FreshRSS, Miniflux, Inoreader, BazQux and The Old Reader. FreshRSS is the first
service it targets.

> **Status:** early development. Nothing is ready to use yet.

## Features (planned for the MVP)

- Three panes: sidebar, article list, reader
- Smart views: Unread, Starred, All Articles
- Two-way sync of feeds, folders, and read and starred state
- Full offline reading, with changes queued until the next sync
- Full keyboard navigation
- Follows the system's light or dark mode

## Tech stack

Skvaller is written in Rust.

- `skvaller-core`: sync, storage and the API client, with no UI code
- `skvaller-app`: accounts, settings and the background sync, with no UI code
- `skvaller`: the window, in [Dioxus Native](https://github.com/DioxusLabs/blitz)
- SQLite local cache, one per account

[ARCHITECTURE.md](ARCHITECTURE.md) describes how the pieces fit together.

## Development

Requires [Rust](https://rustup.rs).

```sh
cargo run --release -p skvaller   # run the app
cargo test --workspace
```

Before a release, check the dependencies against the RustSec advisory database. It
fails on a known vulnerability; an unmaintained crate is only a warning:

```sh
cargo install cargo-audit --locked   # once
cargo audit
```

On macOS, `cargo run` signs the app with your Apple Development certificate if you
have one (Xcode → Settings → Accounts → Manage Certificates; a free account works).
The keychain then keeps trusting each new build, and stops asking for your password
after every rebuild.

To build `Skvaller.app` in `target/release/bundle`, signed the same way (or ad hoc),
and with `--dmg` a disk image too:

```sh
scripts/macos-bundle.sh [--dmg]
```

The app is built for the Mac's own architecture, so the Build workflow's is Apple
silicon only. It isn't notarized: on another Mac, allow the first launch in System
Settings → Privacy & Security → Open Anyway. An ad hoc signature changes with every
build, so the keychain asks for your password again after each update.

On Linux, install the desktop entry and icon so the launcher lists Skvaller and the
dock shows its icon and unread badge. `Exec=skvaller` needs the program on your `PATH`:

```sh
cargo build --release -p skvaller
install -Dm755 target/release/skvaller ~/.local/bin/skvaller
install -Dm644 crates/skvaller/skvaller.desktop ~/.local/share/applications/skvaller.desktop
install -Dm644 crates/skvaller/skvaller.png ~/.local/share/icons/hicolor/512x512/apps/skvaller.png
```

## License

[GPL-3.0](LICENSE)
