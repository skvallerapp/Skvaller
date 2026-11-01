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

- [Tauri 2](https://tauri.app) app shell
- Rust core (`skvaller-core`): sync, storage and API client, with no UI code
- Svelte 5 + TypeScript UI
- SQLite local cache

## Development

Requires [Rust](https://rustup.rs), [Bun](https://bun.sh) and the
[Tauri system dependencies](https://v2.tauri.app/start/prerequisites/).

```sh
bun install
bun run tauri dev      # run the app
cargo test --workspace # Rust tests
bun run check          # type-check the UI
```

## License

[GPL-3.0](LICENSE)
