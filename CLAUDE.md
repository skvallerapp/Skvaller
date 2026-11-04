# Skvaller

A desktop RSS reader (Windows, macOS, Linux).
It is a client for a sync service through the Google Reader API, with FreshRSS as the
first target. It also has a local account that fetches feeds itself, and several accounts
can be connected at once.

## Stack

- Tauri 2 app shell
- `skvaller-core`: a Rust crate for sync, storage (SQLite via `rusqlite`) and the
  Google Reader API client. It contains no UI code.
- Svelte 5 + TypeScript UI
- Lucide icons (`@lucide/svelte`). Import each icon by path, such as
  `import Rss from '@lucide/svelte/icons/rss'`. Importing from the package root makes
  svelte-check load every icon.

## Architecture rules

- **The UI never waits for the network.** The UI reads only from the local
  SQLite cache. Sync runs in the background and updates the cache.
- **Keep the Rust layer thin and testable.** UI behavior belongs in TypeScript.
  Rust exposes data and commands, and is tested in isolation from Tauri.
- **Article HTML is untrusted.** It always renders with scripts disabled. The CSP in
  `tauri.conf.json` applies only to built apps, not to `tauri dev`, so test CSP-related
  changes with `bun run tauri build`. Keep inline `<style>` and `<script>` out of
  `index.html`: Tauri adds nonces to them, and a nonce disables `'unsafe-inline'`.
- **Never store the user's password.** Store only the access token, in the OS
  keychain.
- **Queued offline changes win.** The server never overwrites a queued local
  read or star change.
- **Reference platforms.** macOS sets the bar for polish. Linux (WebKitGTK)
  sets the bar for performance. The article list must scroll smoothly with 5,000+ items.

## Out of scope

Do not add these: mobile apps, a unified timeline (accounts are switched, never merged),
plugins, AI features. Other sync backends are planned for "Later".
