# Architecture

How Skvaller fits together. Each module's details are in its module docs (`//!`).

## Bird's-eye view

```
 server (Google Reader API)        feeds (local account)
            │                               │
            ▼                               ▼
 skvaller-core  ── sync / refresh_local ──► SQLite cache, one per account
                                                     │  ▲
 skvaller-app   ── App: accounts, config, schedules ─┘  │ set_read, set_starred
                   │ broadcast SyncStatus               │ (queued in `pending`)
                   ▼                                    │
 skvaller       ── windows (Dioxus Native) ─────────────┘  reads only from the cache
```

1. `App::load` reads `config.json` and opens each account's cache.
2. `App::start_sync` runs each account's sync in the background, on its schedule.
3. The UI reads folders, feeds and articles from the cache, synchronously.
4. When a sync changes the cache, `App` sends a `SyncStatus` with `changed` set. The
   main window bumps a revision, which reloads the sidebar and the list.
5. A read or star change from the UI is written to the cache at once and queued in
   the `pending` table. The next sync pushes it.

## Crates

Dependencies go one way: `skvaller` → `skvaller-app` → `skvaller-core`.

### `skvaller-core`

Sync, storage, the API client and everything that touches untrusted content. No UI
code and no knowledge of config or windows; tested on its own (`wiremock` for the
server).

- `db`: the SQLite cache. `pending` holds queued read and star changes. A second
  connection serves the UI's reads, so they don't wait behind a sync's writes (WAL).
- `api`: the Google Reader client. Only calls FreshRSS, Miniflux and The Old Reader
  all have; articles come by ID.
- `sync`: two-way sync. Push queued changes, pull subscriptions, articles and flags,
  prune old read articles.
- `local`: the local account. Fetches and parses feeds itself; its state lives only in
  the cache.
- `keychain`: the access token in the OS keychain. Blocking calls.
- `net`: `Untrusted`, the HTTP client for addresses that come from feed content.
- `html`, `text`, `reader`, `full_text`: parsing, previewing, readying and extracting
  untrusted article HTML.
- `thumbnail`: lead images, fetched after a sync and stored small in the cache.
- `opml`, `seed`: subscription import and export; synthetic articles for testing.

### `skvaller-app`

The app without a UI.

- `app`: `App`, the accounts and their caches as the app runs, and `Dirs`.
- `config`: `config.json`, read forgivingly.
- `accounts`, `feeds`: adding and removing accounts; the local account's feeds and
  folders.
- `sync`, `gate`: each account's schedule, one sync at a time.
- `thumbnails`: the background thumbnail runs after a sync.

`App` is shared as `Arc<App>` and is `Sync`. It has two channels out:

- `subscribe()`: a `broadcast` of `SyncStatus`, for sync progress and cache changes.
- `watch_config()`: a `watch` of the config revision, bumped by every settings or
  account change.

### `skvaller`

The windows, in Dioxus Native (Blitz).

- `main`: starts the sync runtime, sets up the HTTP client, request filter and `thumb:`
  scheme, and launches the main window.
- `ui`: the main window, with the sidebar, the virtualized article list and the reader.
  `ui/manage`: feed and folder menus.
- `settings`, `add_account`: the Settings window and sign-in.
- `article`: the reader's `<iframe srcdoc>` document.
- `thumbs`: serves cached thumbnails to `<img>` through `thumb:` URLs.
- `live`: each window's view of the settings, following `watch_config()`.
- `chrome`: the windows' frame. On macOS the title bar is hidden: the window buttons
  sit over the toolbars, which grow to the title bar's 52px, and the window drags from
  their empty space. Windows and Linux keep the system's title bar.
- `i18n`, `dates`, `fonts`, `icons`, `shortcuts`: text, date formats, font stacks,
  icons and key bindings.

## Runtime

- **Two Tokio runtimes.** `main` makes one for syncs and background work; Dioxus Native
  runs its own for loading images. Blocking calls (keychain, heavy parsing) go through
  `spawn_blocking`.
- **One `VirtualDom` per window.** Windows share only the `Arc<App>` they're given.
  Each follows the app through `live::use_live` and its own `subscribe()`.
- **One sync per account at a time.** A second one asked for meanwhile reports `Busy`
  instead of queueing (`gate`).

## Invariants

- **The UI never waits for the network.** `App`'s synchronous methods read only the
  cache. The ones that may fetch, such as `full_text`, are `async` and run in a task.
- **Queued offline changes win.** `Db` upserts never overwrite a flag that has a row in
  `pending`, and pruning skips those articles. `clear_pending` drops a change only if
  its value is still the one the server accepted.
- **Article HTML is untrusted.**
  - It renders in an `<iframe srcdoc>`, so its styles stay out of the app's document.
    Blitz runs no scripts.
  - `reader` makes its URLs absolute and https, and holds media until asked for.
  - Every request from any document passes `main`'s request filter: only public
    addresses (`net::ArticleNet`), or `thumb:` URLs carrying a token made at launch.
  - Reader View pages, thumbnails and icons are fetched through `net::Untrusted`, which
    checks the resolved address on every connection and redirect. The sync server and
    subscribed feeds are the user's choice and aren't filtered.
  - `html` bounds nesting depth, so a crafted page can't stall the parser.
- **The password is never stored.** Sign-in trades it for a token, which `keychain`
  saves under `KEYCHAIN_SERVICE`. That name also names the config and data folders
  (`Dirs::system`), so changing it loses both the tokens and the caches.
- **A bad `config.json` doesn't stop the app.** `config` reads unknown or broken values
  as defaults and leaves out accounts it can't read.
- **5,000+ items scroll smoothly.** The list renders only the visible rows plus some
  either side (`ui::visible_rows`). Thumbnails are served cropped to their box, since
  Blitz keeps every image decoded. The Large box follows the list's saved width in
  steps of 40 px, so resizing the list loads new thumbnails only now and then.
  `--seed-test <count>` fills a local account for testing this.

## On disk

```
<config dir>/<KEYCHAIN_SERVICE>/config.json      settings, accounts and window layout, no tokens
<local data dir>/<KEYCHAIN_SERVICE>/accounts/<id>.db   one SQLite cache per account
OS keychain, service KEYCHAIN_SERVICE            one access token per account
```
