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
                   │ broadcast SyncStatus, Change       │ (queued in `pending`)
                   ▼                                    │
 skvaller       ── windows (Dioxus Native) ─────────────┘  reads only from the cache
```

1. `App::load` reads `config.json` and opens each account's cache.
2. `App::start_sync` runs each account's sync in the background, on its schedule.
3. The UI reads folders, feeds and articles from the cache, synchronously.
4. Every `App` method that changes what a window shows of a cache sends a `Change`:
   the account, and whether flags, feeds or articles changed. A sync sends one when it
   changed the articles or their feeds, or failed and may have. Reader View pages are
   stored without one: no list shows them. The main window reloads its sidebar and list from these alone.
5. A read or star change from the UI goes through `App` too: it is written to the
   cache at once, queued in the `pending` table and sent as a `Change`. The next sync
   pushes it.

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
- `icons`: feed icons, fetched into the cache for both kinds of account.
- `opml`, `seed`: subscription import and export; synthetic articles for testing.

### `skvaller-app`

The app without a UI.

- `app`: `App`, the accounts and their caches as the app runs, and `Dirs`.
- `config`: `config.json`, read forgivingly.
- `accounts`, `feeds`: adding and removing accounts; the local account's feeds and
  folders.
- `sync`, `gate`: each account's schedule, one sync at a time.
- `thumbnails`: the background thumbnail runs after a sync.

`App` is shared as `Arc<App>` and is `Sync`. It has three channels out:

- `subscribe()`: a `broadcast` of `SyncStatus`, for sync progress.
- `changes()`: a `broadcast` of `Change`, sent by every method that changes what a
  window shows of a cache:
  read, star and mark all read (`Flags`, `AllRead`), feed and folder changes
  (`Feeds`), and syncs and thumbnail fetches (`Articles`).
- `watch_config()`: a `watch` of the config revision, bumped by every settings or
  account change. A cache change doesn't bump it.

### `skvaller`

The windows, in Dioxus Native (Blitz).

- `main`: starts the sync runtime, sets up the HTTP client, request filter and `thumb:`
  scheme, and launches the main window.
- `ui`: the main window, its state (`Reading`) and layout. It counts the shown
  account's `Change`s in `Cached`: every one reloads the library, every one but a read
  or star change reloads the list, and a mark-all-read also empties Unread of the rows
  it kept. `canvas_css` gives each window's html the theme's background, except on
  Linux while framed, where the window is transparent. Blitz paints it where its layout
  rounding leaves an edge uncovered; it repeats app.css's `--reader-bg`. Its parts:
  - `ui/sidebar`: the view and account switchers, feeds and folders. `ui/tree`: the
    sidebar's rows as a tree and how the selection steps through them, as plain
    functions.
  - `ui/list`: the virtualized article list and its search bar. `ui/listed`: the rows
    each load of it shows, keeping those read or unstarred here and the open article,
    so a sync doesn't pull them away while you read. `ui/pull`: the lists' rubber band
    and pull to refresh.
  - `ui/reader`: the open article and its buttons.
  - `ui/manage`: feed and folder menus.
- `ui/command`: what the main window does. Every key, macOS menu bar item and button
  that asks it for something becomes a `Command`, run by `Reading::run`, which holds
  each command's guards, such as needing the panes rather than the sign-in screen.
  `route` decides what a key does, in a fixed order: the app's chords (the accounts'
  among them), the open overlay's keys, Escape for the search, the arrow keys, then the
  shortcuts. The window's one `Overlay` slot holds the account menu, the
  mark-all-read question or a `ui/manage` menu, question or form, so two can't be
  open at once; its methods say which stays open after a click, a hidden sidebar or
  another list. `leave` says what selecting another list resets. These decisions are
  plain functions, tested without a window, as `overscroll`'s are.
- `settings`, `add_account`: the Settings window and sign-in.
- `article`: the reader's `<iframe srcdoc>` document. A new look (theme, fonts, size,
  line width) remakes it, and the frame loads it from the top: `ui/reader`'s `Remake` reads
  the place first, as a share of how far the frame scrolls, hides the new frame and
  scrolls it back there once its layout holds still. Images that load after that move
  it a little.
- `thumbs`: serves cached thumbnails to `<img>` through `thumb:` URLs.
- `lines`: counts the lines a headline wraps to, from each character's advance, so
  the list sizes its rows without measuring them.
- `scale`: the font size setting. Each kind of text has its own range, a factor on its
  size at M, which app.css reads as variables on the main window's root and the
  article's document through `article::Look`. The sizes the sidebar and the list
  are placed from come from here too, so the CSS and the arithmetic agree.
- `place`: where a scrolled list is, as plain arithmetic on row offsets: the article
  list's visible rows, how it keeps its place from one render to the next (`Keeper`),
  and how far to scroll to show a row, for the article list, `dropdown`, and the
  panes `ui::scroll_into_view` scrolls: the sidebar and Settings. `ui/list`'s `Viewport`
  reads the element and scrolls where it says.
- `overscroll`: a list stretched past its ends by trackpad or touch, and pull to
  refresh, as state alone: `ui/pull` feeds it the events and draws the offset.
- `live`: each window's view of the settings, following `watch_config()`, and of the
  system's dark mode and window frame, following `appearance::watch()` and
  `watch_frame()`. Its `theme` is the one to show. `use_cache_changes` hands a window
  each `Change` from `changes()`, or `None` when it may have missed some.
- `appearance`: the system's dark mode on Linux, which winit doesn't report, read from
  the XDG desktop portal over D-Bus. It makes the System theme Light or Dark. It also
  reads how the system draws a window's frame: on KDE, the outline's color made from
  `kdeglobals` as Breeze makes it, and Breeze's corner radius. On Windows the frame is
  fixed, Windows 11's (`SystemFrame::WINDOWS`), for the window buttons the app draws.
- `chrome`: the windows' frame. On macOS the title bar is hidden: the window buttons
  sit over the toolbars, which grow to the title bar's 52px. On Linux the window has
  no decorations and a transparent surface: the app draws minimize, maximize and
  close at the window's top right, as the system's frames draw them (Breeze's on KDE,
  GNOME's elsewhere), and a frame with the system's outline and rounded corners, and
  edges to resize by, while not maximized or full screen. On Windows the window has no
  title bar but keeps the system's shadow, border and rounded corners, its resize
  borders outside the left, right and bottom edges (the winit fork's
  `with_undecorated_resize_borders`), and snapping by drag and keyboard: the app draws
  Windows 11's caption buttons flush with the top right corner, the top edge and its
  corners to resize by, and on Windows 10 the top border (`top_border`). Hovering its
  maximize button shows no Snap Layouts, which take the system's hit test for it, and
  winit doesn't give one. The reader keeps a least width for the buttons
  (`ui::READER_LEAST`), which the list gives up width to, and the main window's least
  width follows from it. Everywhere the window drags from the toolbars' empty space.
- `top_border`: the 1px top border Windows 10 doesn't draw on the window without a
  title bar: any non-client area at the top makes DWM draw a caption above the window,
  so there is none. `chrome::Frame` draws it, one device pixel tall, in DWM's colors:
  the accent color while active when the user shows it on window borders, otherwise
  grays fitted to DWM's. It reads the build from the registry once per window, and
  the accent again on each change of focus. Windows 11 draws the border over the
  content itself. Windows only.
- `shadow`: the window's drop shadow on KDE Plasma, which KWin draws only for windows
  with a decoration. It renders Breeze's shadow as Breeze does and hands it to KWin
  through `org_kde_kwin_shadow` on winit's own Wayland connection, or on X11 through
  the window's `_KDE_NET_WM_SHADOW` property, on a connection of its own, so KWin draws
  it outside the window. Linux only.
- `menu`: the macOS menu bar (muda), with Settings…, the Accounts menu and the Window
  menu. Each item is a `ui::Command`, which the main window runs through
  `Reading::run`, as it does its keys. Its account items' keys, ⌘1 to ⌘9, are
  `shortcuts::ACCOUNT_KEYS`, as the window's are. `build.rs` embeds `Info.plist` so the menu bar names the app Skvaller;
  `scripts/macos-bundle.sh` puts the same file in `Skvaller.app`.
  Windows and Linux have no menu bar.
- `spin`: when the sync icon turns and stops, as plain state.
- `i18n`, `dates`, `fonts`, `icons`, `shortcuts`: text, date formats, font stacks,
  icons and keys: the bindings, the app's chords, the system's chords a shortcut
  can't take, and how the arrow keys move a menu's highlight, shared by the window's
  menus and `dropdown`. Text fields hand the window only the app's chords, so a
  shortcut never fires while typing.

## Runtime

- **Two Tokio runtimes.** `main` makes one for syncs and background work; Dioxus Native
  runs its own for loading images. Blocking calls (keychain, heavy parsing) go through
  `spawn_blocking`.
- **On Linux, an `appearance` thread** follows the portal's dark mode and window
  frame on a single-threaded Tokio runtime of its own, plus the executor thread zbus
  starts for its connection. `main` starts it
  before loading the app, and the first window waits up to 300 ms for its first
  answers, two local calls, not the network. On KDE under X11, each window's shadow keeps
  a connection of its own to the X server.
- **One copy of the app at a time.** `main` locks `skvaller.lock` in the data folder
  before loading: a second copy would save its own config over the first's and sync
  the same caches. A second launch exits.
- **One `VirtualDom` per window.** Windows share only the `Arc<App>` they're given.
  Each follows the app through `live::use_live`, `live::use_cache_changes` and its own
  `subscribe()`.
- **One sync per account at a time.** A second one asked for meanwhile reports `Busy`
  instead of queueing (`gate`). `sync_now` hands back a wait that ends with the
  account's sync, its own or the one it found running: pull to refresh awaits it.

## Invariants

- **The UI never waits for the network.** `App`'s synchronous methods read only the
  cache. The ones that may fetch, such as `full_text`, are `async` and run in a task.
- **Queued offline changes win.** `Db` upserts never overwrite a flag that has a row in
  `pending`, and pruning skips those articles. `clear_pending` drops a change only if
  its value is still the one the server accepted.
- **Article HTML is untrusted.**
  - It renders in an `<iframe srcdoc>`, so its styles stay out of the app's document.
    Blitz runs no scripts, and `reader` doesn't rely on that: it removes `script`,
    `meta`, `base`, `link`, `embed`, `template` and SVG's animations, unwraps `object`,
    and removes every `on…` handler. An `href`, `src`, `poster` or `srcset` keeps only a
    web URL, except a `data:` image, a `mailto:` link and an in-page `#…`.
  - `reader` makes its URLs absolute and https, links written-out addresses, and holds
    media until asked for.
  - Every request from any document passes `main`'s request filter: only public
    addresses (`net::ArticleNet`), or `thumb:` URLs carrying a token made at launch.
    An SVG's own `<image href>` loads only from `data:`; Blitz never reads it as a file.
    Blitz drops an HTTP response body over 32 MiB (`data:` URLs aren't bodies), and any
    image, an SVG's own rasters too, whose header says it takes over 128 MiB as RGBA or
    in its decoder's own format, or whose decode needs over 256 MiB. Decoders spend
    memory or time on a PNG's ICC profile and text, which are taken out, and on a JPEG's
    ICC profile, extended XMP and gain map, which are made unrecognisable. Each limit is
    per body or per image, not per page.
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
  either side (`place::visible_rows`). Rows are placed from computed heights, never
  measured: each fits its headline's lines, as `lines` counts them at the width the
  list shows and the font size (`scale`), and its thumbnail. A headline is cut once,
  where it may wrap, so a new width only wraps the pieces again. It is clamped to its
  count, so one counted short loses its last line rather than showing half of it.
  When the heights change, the list keeps its place (`place::Keeper`). Thumbnails
  are served cropped to their box, since Blitz keeps every image decoded. The Large
  box is 16:9 at the list's width; its image is served for the saved width in steps
  of 40 px, so resizing the list loads new thumbnails only now and then.
  `--seed-test <count>` fills a local account for testing this.
- **One font scan.** `fonts::font_context` scans the system's fonts once; each window
  and the line counter get a copy that shares it. The collection isn't shared, so an
  article's web fonts stay in its frame.

## On disk

```
<config dir>/<KEYCHAIN_SERVICE>/config.json      settings, accounts and window layout, no tokens
<local data dir>/<KEYCHAIN_SERVICE>/accounts/<id>.db   one SQLite cache per account
OS keychain, service KEYCHAIN_SERVICE            one access token per account
```
