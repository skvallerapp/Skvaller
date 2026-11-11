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

## License

[GPL-3.0](LICENSE)
