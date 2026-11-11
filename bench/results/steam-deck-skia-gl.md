# steam-deck-skia-gl

- Date: 2026-11-11
- Machine: AMD Custom APU 0932, 14798 MB RAM, Arch Linux (kernel 7.2.7-valve2-2-neptune-72-g31504582a073), session wayland, apps on X11/Xwayland
- Builds: tauri `612f433 Open windows in their settings' theme and font from the first paint`, rust `d4ea5b8 Ignore what the tauri branch leaves behind, and dev/`
- Settings: 1 runs, scroll to the bottom of the list, warm-up on
- Memory columns: PSS / RSS in MB, summed over the app and its child processes. PSS splits shared pages between processes; use it to compare.

| Run | Window after | Procs | Launch CPU | Launch PSS/RSS | Idle 30 s CPU | Idle PSS/RSS | Notches | Scroll time | Scroll CPU | After scroll PSS/RSS |
|---|---|---|---|---|---|---|---|---|---|---|
| tauri1 | 0.4 s | 3 | 2.42 s | 366 / 487 | 0.02 s | 358 / 479 | 5700 | 203 s | 102.13 s | 482 / 601 |
| rust1 | 0.2 s | 1 | 0.48 s | 199 / 202 | 0.00 s | 199 / 202 | 7400 | 266 s | 115.39 s | 191 / 193 |

## tauri1

Processes after the scroll (PSS / RSS MB):

- skvaller: 107 / 156
- WebKitWebProces: 337 / 384
- WebKitNetworkPr: 37 / 61

Scroll CPU by thread:

- WebKitWebProces: WebKitWebProces: 38.53 s
- skvaller: skvaller: 16.65 s
- WebKitWebProces: eadedCompositor: 12.37 s
- WebKitWebProces: SkiaCPUWorker: 5.78 s
- WebKitWebProces: ReceiveQueue: 5.47 s
- skvaller: ReceiveQueue: 4.85 s
- WebKitWebProces: Core: Scrolling: 4.84 s
- WebKitWebProces: WebKitWebP:cs: 2.84 s
- skvaller: skvaller:cs: 0.97 s
- WebKitWebProces: WebKitWe:gdrv: 0.89 s
- WebKitWebProces: EventDispatcher: 0.78 s
- skvaller: VBlankMonitor: 0.44 s

Screenshot after the scroll: `results/steam-deck-skia-gl-tauri1.png`

## rust1

Processes after the scroll (PSS / RSS MB):

- skvaller-skia-g: 191 / 193

Scroll CPU by thread:

- skvaller-skia-g: skvaller-skia-g: 92.38 s
- skvaller-skia-g: skvaller-s:cs: 3.23 s
- skvaller-skia-g: StyleThread#: 2.28 s

Screenshot after the scroll: `results/steam-deck-skia-gl-rust1.png`

## Notes

- **Renderer: Skia on OpenGL**, instead of the default Vello hybrid. The rust build is `main` at
  `d4ea5b8` with one local, uncommitted change: a `skia = ["dioxus-native/skia"]` feature in
  `crates/skvaller/Cargo.toml`, built with `--no-default-features --features skia`. On Linux,
  `anyrender_skia` (anyrender `814e100`, skia-safe 0.156) draws through OpenGL (EGL, Mesa
  radeonsi); the log says "Selected Skia OpenGL backend".
- Both screenshots show the bottom of the list, ending at Test article 5000. No "rendered in
  software" warning.
- Only one measured run per app (`--runs 1`), so the runs couldn't be checked against each other.
  On this Deck, repeat runs have agreed within about 1% (see `steam-deck.md`).
- This run used the confirmed bottom check in `bench.py`: with Skia, XGetImage sometimes returns
  a stale frame of the window, so an unchanged snapshot is now confirmed against the compositor
  with one more batch. That extra batch is in both apps' notch counts (100 more than in
  `steam-deck.md`, about 1.6–1.9 s of scroll CPU).
- Compared with Vello (`steam-deck.md`): scroll CPU 115.4 s vs about 138.5 s (about 17% less),
  with no `tokio-rt-worker` CPU (Vello: about 17 s) and about 24 s less on the main thread.
  Memory is a little higher than Vello's: 199 MB PSS at launch, 191 MB after the scroll.
- Setup as in `steam-deck.md`: Desktop Mode, plugged in, both apps in the `skvaller` distrobox on
  Xwayland.
