# steam-deck-skia-vulkan

- Date: 2026-11-11
- Machine: AMD Custom APU 0932, 14798 MB RAM, Arch Linux (kernel 7.2.7-valve2-2-neptune-72-g31504582a073), session wayland, apps on X11/Xwayland
- Builds: tauri `612f433 Open windows in their settings' theme and font from the first paint`, rust `d4ea5b8 Ignore what the tauri branch leaves behind, and dev/`
- Settings: 1 runs, scroll to the bottom of the list, warm-up on
- Memory columns: PSS / RSS in MB, summed over the app and its child processes. PSS splits shared pages between processes; use it to compare.

| Run | Window after | Procs | Launch CPU | Launch PSS/RSS | Idle 30 s CPU | Idle PSS/RSS | Notches | Scroll time | Scroll CPU | After scroll PSS/RSS |
|---|---|---|---|---|---|---|---|---|---|---|
| tauri1 | 0.4 s | 3 | 2.41 s | 375 / 497 | 0.44 s | 367 / 490 | 5700 | 203 s | 101.97 s | 564 / 687 |
| rust1 | 0.2 s | 1 | 0.44 s | 162 / 165 | 0.00 s | 162 / 165 | 7500 | 283 s | 116.87 s | 156 / 158 |

## tauri1

Processes after the scroll (PSS / RSS MB):

- skvaller: 107 / 157
- WebKitWebProces: 419 / 468
- WebKitNetworkPr: 37 / 62

Scroll CPU by thread:

- WebKitWebProces: WebKitWebProces: 38.25 s
- skvaller: skvaller: 16.67 s
- WebKitWebProces: eadedCompositor: 12.31 s
- WebKitWebProces: SkiaCPUWorker: 5.78 s
- WebKitWebProces: ReceiveQueue: 5.45 s
- WebKitWebProces: Core: Scrolling: 4.96 s
- skvaller: ReceiveQueue: 4.90 s
- WebKitWebProces: WebKitWebP:cs: 2.85 s
- skvaller: skvaller:cs: 0.97 s
- WebKitWebProces: WebKitWe:gdrv: 0.89 s
- WebKitWebProces: EventDispatcher: 0.78 s
- skvaller: VBlankMonitor: 0.44 s

Screenshot after the scroll: `results/steam-deck-skia-vulkan-tauri1.png`

## rust1

Processes after the scroll (PSS / RSS MB):

- skvaller-skia-v: 156 / 158

Scroll CPU by thread:

- skvaller-skia-v: skvaller-skia-v: 94.75 s
- skvaller-skia-v: StyleThread#: 2.49 s
- skvaller-skia-v: WSI swapchain e: 1.62 s
- skvaller-skia-v: WSI swapchain q: 1.01 s

Screenshot after the scroll: `results/steam-deck-skia-vulkan-rust1.png`

## Notes

- **Renderer: Skia on Vulkan**, instead of the default Vello hybrid. This needs local patches, so
  it can't be built from `main` as is:
  - `crates/skvaller/Cargo.toml` (uncommitted, on `main` at `d4ea5b8`): a
    `skia-vulkan = ["skia", "dep:anyrender_skia", "anyrender_skia/vulkan"]` feature, built with
    `--no-default-features --features skia-vulkan`.
  - `anyrender_skia` (anyrender `814e100`, patched in through `[patch]`): it has a Vulkan backend
    but never selects it; on Linux `Auto` always means OpenGL. The patch makes `Auto` choose
    Vulkan when the `vulkan` feature is on (and accepts `ANYRENDER_SKIA_BACKEND=vulkan`). It also
    makes the Vulkan swapchain use FIFO (vsync) instead of preferring MAILBOX, to match Vello's
    `AutoVsync`.
  - The log says "Selected Skia Vulkan backend"; the app loads `libvulkan_radeon.so`.
- Both screenshots show the bottom of the list, ending at Test article 5000. No "rendered in
  software" warning.
- Only one measured run per app (`--runs 1`), so the runs couldn't be checked against each other.
  On this Deck, repeat runs have agreed within about 1% (see `steam-deck.md`).
- This run used the confirmed bottom check in `bench.py`: with Skia, XGetImage sometimes returns
  a stale frame of the window (on Vulkan for several batches in a row), so an unchanged snapshot
  is now confirmed against the compositor with one more batch. That extra batch is in both apps'
  notch counts (100 more than in `steam-deck.md`, about 1.6–1.9 s of scroll CPU).
- Compared with Vello (`steam-deck.md`): scroll CPU 116.9 s vs about 138.5 s (about 17% less once
  the extra batch is allowed for), with no `tokio-rt-worker` CPU (Vello: about 17 s) and about
  22 s less on the main thread. Memory is the lowest of the renderers: 162 MB PSS at launch,
  156 MB after the scroll. Scroll CPU is about the same as Skia on OpenGL
  (`steam-deck-skia-gl.md`).
- Setup as in `steam-deck.md`: Desktop Mode, plugged in, both apps in the `skvaller` distrobox on
  Xwayland.
