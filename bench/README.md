# Tauri vs all-Rust: CPU and memory

`bench.py` runs the Tauri build (`tauri` branch) and the all-Rust build (`main`) one after the
other on the same data and measures their CPU time and memory, WebKit's processes included.
This file is the instructions for an agent (or a person) running it on a Mac or a Steam Deck.

## What it measures

Each app starts with a fresh copy of `fixture/`: one local account, 20 feeds, 5,000 unread
articles, 3,334 thumbnails, Medium thumbnails. The feeds point at `.invalid` hosts, so sync
fails at once and the same way in both apps ("Sync failed" is expected). Your own Skvaller data
is not touched: the apps get their folders from `.home/` here (via `XDG_*` on Linux, `HOME` on
macOS).

Per run, summed over the app and all its processes:

| Phase | What happens |
|---|---|
| Window after | Seconds from launch until the window is on screen |
| Launch | CPU time and memory 10 s after the window appeared |
| Idle | CPU time over the next 30 s, memory at the end |
| Scroll | Mouse-wheel notches over the article list, 30 ms apart, from the top **to the bottom of the list**; CPU time and memory after |

The scroll runs in batches of 100 notches. After each batch the script captures the window,
and it stops when a batch leaves the window unchanged: the list is at the bottom. So both apps
scroll exactly the same distance, all 5,000 articles, however far one notch goes in each
(it differs per app and per OS). The report has the notches and seconds each app needed.

First, each app is launched once without measuring (warm-up) so shader caches are filled.
Then the apps alternate, `--runs` times each (default 2). On Linux the report also splits
the scroll CPU by thread, which shows where the work goes (e.g. a GPU driver's threads).

## 1. Build both apps

Use worktrees next to the repo so both builds are current and this branch stays only the
tool. From the repo root:

```sh
git fetch origin
git worktree add ../skvaller-main origin/main
git worktree add ../skvaller-tauri origin/tauri
(cd ../skvaller-main && cargo build --release -p skvaller)
(cd ../skvaller-tauri && bun install && bunx tauri build --no-bundle)
```

The binaries are `../skvaller-main/target/release/skvaller` and
`../skvaller-tauri/target/release/skvaller`. Both must be release builds.

### macOS

- Needs the Xcode Command Line Tools (`swiftc`, used to build `mac-input.swift` on the first
  run), Rust and bun.
- Give the terminal app that runs the script **Accessibility** access (to send scroll events)
  and **Screen Recording** access (to see when the list reaches the bottom), in System
  Settings > Privacy & Security. The script stops with a message if either is missing.

### Steam Deck

- Run everything in Desktop Mode, inside the existing distrobox (`distrobox list`,
  `distrobox enter <name>`). Check what it already has; it may have been set up for the Tauri
  build only. It needs: Rust (rustup), bun, the Tauri Linux prerequisites (webkit2gtk-4.1 and
  its dev files, build tools, OpenSSL, librsvg), `libxtst`, Python 3, and the **Vulkan
  driver for the Deck's GPU** (Arch: `vulkan-radeon`, Debian/Ubuntu: `mesa-vulkan-drivers`).
  Without the Vulkan driver the all-Rust app falls back to software rendering, and the
  numbers are worthless; the report warns when that happens.
- If the all-Rust build asks for a system library, install its dev package in the container.
- The Deck's desktop runs Wayland. The script runs both apps on **Xwayland** (it unsets
  `WAYLAND_DISPLAY` and sets `GDK_BACKEND=x11`), because that is where it can send scroll
  events (XTest). Both apps pay the same Xwayland cost; say so when you report.
- Keep the Deck plugged in, its screen on, and close Steam's game mode and other heavy apps.

## 2. Run

Quit any running Skvaller first. From the repo root:

```sh
python3 bench/bench.py \
  --tauri ../skvaller-tauri/target/release/skvaller \
  --rust ../skvaller-main/target/release/skvaller \
  --machine steam-deck        # or macbook-<chip>, e.g. macbook-m1
```

It takes about 15–20 minutes. Don't touch the mouse or the keyboard while it runs: the scroll
goes to whatever is under the pointer. It writes `bench/results/<machine>.md` and a
screenshot after each run's scroll.

## 3. Check the run before you report it

Open every screenshot.

- Both apps' article lists must be at the bottom: the last articles (Test article 4,99x)
  showing. If one isn't, the run doesn't count: find out why (a window partly off screen,
  another window on top) and run again.
- If the script says the window didn't change while scrolling: on macOS, check Screen
  Recording access, and run again with `--wheel-delta 3` in case the list scrolled up (it is
  already at the top, so up looks like no change). Otherwise, make sure the whole window is on
  screen.
- On Linux, the report must have no "rendered in software" warning for either app. If the
  all-Rust app has one, fix the container's Vulkan driver and run again.
- The runs of the same app should agree within a few percent. If they don't, something else
  was using the machine: run again.

## 4. Report

Add to the end of `bench/results/<machine>.md` a short `## Notes` section: whether every
screenshot shows the bottom of the list, anything unusual, and the setup (Xwayland on the Deck, power
plugged in). Then commit the `.md` and the `.png` files to this branch and push:

```sh
git add bench/results && git commit -m "Add <machine> bench results" && git pull --rebase && git push
```

Commit messages in this repo carry no `Co-Authored-By` or "Generated with" lines.

## Reading the numbers

- Compare apps on the same machine, not machines with each other.
- Memory: on Linux use PSS (it splits shared libraries fairly between WebKit's processes); on
  macOS use footprint (what Activity Monitor shows as Memory).
- CPU is CPU-seconds summed over all threads and processes, so it can exceed the
  wall-clock time.
- `results/dev-server-xvfb.md` is the reference run from the Linux dev server. Both apps
  rendered in software there (no GPU), which inflates the all-Rust app's scroll CPU and memory
  far more than WebKit's. Its Tauri "window after" includes a ~35 s D-Bus wait that only
  happens on that server.
