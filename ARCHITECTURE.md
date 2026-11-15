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
  `search` is an FTS5 index of the articles' titles, authors and text, kept in step
  by triggers, whatever writes the articles. They call `search_text`, a Rust function
  registered on the write connection, which turns the untrusted HTML into plain text.
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
- `lines`: counts the lines a headline wraps to, from each character's advance, so
  the list sizes its rows without measuring them.
- `overscroll`: a list stretched past its ends by trackpad or touch, and pull to
  refresh, as state alone: `ui` feeds it the events and draws the offset.
- `live`: each window's view of the settings, following `watch_config()`, and of the
  system's dark mode and window frame, following `appearance::watch()` and
  `watch_frame()`. Its `theme` is the one to show.
- `appearance`: the system's dark mode on Linux, which winit doesn't report, read from
  the XDG desktop portal over D-Bus. It makes the System theme Light or Dark. It also
  reads how the system draws a window's frame: on KDE, the outline's color made from
  `kdeglobals` as Breeze makes it, and Breeze's corner radius.
- `chrome`: the windows' frame. On macOS the title bar is hidden: the window buttons
  sit over the toolbars, which grow to the title bar's 52px. On Linux the window has
  no decorations and a transparent surface: the app draws minimize, maximize and
  close at the window's top right, as the system's frames draw them (Breeze's on KDE,
  GNOME's elsewhere), and a frame with the system's outline and rounded corners, and
  edges to resize by, while not maximized or full screen. On macOS and Linux the
  window drags from the toolbars' empty space. Windows keeps the system's title bar.
- `shadow`: the window's drop shadow on KDE Plasma, which KWin draws only for windows
  with a decoration. It renders Breeze's shadow as Breeze does and hands it to KWin
  through `org_kde_kwin_shadow` on winit's own Wayland connection, or on X11 through
  the window's `_KDE_NET_WM_SHADOW` property, on a connection of its own, so KWin draws
  it outside the window. Linux only.
- `menu`: the macOS menu bar (muda), with Settings…, the Accounts menu and the Window
  menu. Its items send commands to the main window's root, which runs them as its own
  shortcuts do. `build.rs` embeds `Info.plist` so the menu bar names the app Skvaller.
  Windows and Linux have no menu bar.
- `i18n`, `dates`, `fonts`, `icons`, `shortcuts`: text, date formats, font stacks,
  icons and key bindings.

## Runtime

- **Two Tokio runtimes.** `main` makes one for syncs and background work; Dioxus Native
  runs its own for loading images. Blocking calls (keychain, heavy parsing) go through
  `spawn_blocking`.
- **On Linux, an `appearance` thread** follows the portal's dark mode and window
  frame, plus the executor thread zbus starts for its connection. `main` starts it
  before loading the app, and the first window waits up to 300 ms for its first
  answer, a local call, not the network. On KDE under X11, each window's shadow keeps
  a connection of its own to the X server.
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
  either side (`ui::visible_rows`). Rows are placed from computed heights, never
  measured: each fits its headline's lines, as `lines` counts them at the width the
  list shows, and its thumbnail. A headline is cut once, where it may wrap, so a
  new width only wraps the pieces again. It is clamped to its count, so one counted
  short loses its last line rather than showing half of it. When the heights change,
  the list keeps its place (`ui::Anchor`). Thumbnails are served cropped to their box,
  since Blitz keeps every image decoded. The Large box is 16:9 at the list's width;
  its image is served for the saved width in steps of 40 px, so resizing the list
  loads new thumbnails only now and then. `--seed-test <count>` fills a local account
  for testing this.
- **One font scan.** `fonts::font_context` scans the system's fonts once; each window
  and the line counter get a copy that shares it. The collection isn't shared, so an
  article's web fonts stay in its frame.

## On disk

```
<config dir>/<KEYCHAIN_SERVICE>/config.json      settings, accounts and window layout, no tokens
<local data dir>/<KEYCHAIN_SERVICE>/accounts/<id>.db   one SQLite cache per account
OS keychain, service KEYCHAIN_SERVICE            one access token per account
```
