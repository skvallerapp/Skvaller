# steam-deck

- Date: 2026-11-11
- Machine: AMD Custom APU 0932, 14798 MB RAM, Arch Linux (kernel 7.2.7-valve2-2-neptune-72-g31504582a073), session wayland, apps on X11/Xwayland
- Builds: tauri `612f433 Open windows in their settings' theme and font from the first paint`, rust `d4ea5b8 Ignore what the tauri branch leaves behind, and dev/`
- Settings: 2 runs, scroll to the bottom of the list, warm-up on
- Memory columns: PSS / RSS in MB, summed over the app and its child processes. PSS splits shared pages between processes; use it to compare.

| Run | Window after | Procs | Launch CPU | Launch PSS/RSS | Idle 30 s CPU | Idle PSS/RSS | Notches | Scroll time | Scroll CPU | After scroll PSS/RSS |
|---|---|---|---|---|---|---|---|---|---|---|
| tauri1 | 0.4 s | 3 | 2.40 s | 377 / 500 | 0.01 s | 373 / 497 | 5600 | 197 s | 101.83 s | 560 / 682 |
| rust1 | 0.2 s | 1 | 0.45 s | 185 / 187 | 0.00 s | 185 / 187 | 7400 | 261 s | 138.80 s | 177 / 179 |
| tauri2 | 0.4 s | 3 | 2.38 s | 368 / 489 | 0.01 s | 364 / 486 | 5600 | 197 s | 102.38 s | 567 / 688 |
| rust2 | 0.2 s | 1 | 0.44 s | 182 / 185 | 0.00 s | 182 / 185 | 7400 | 261 s | 138.32 s | 176 / 179 |

## tauri1

Processes after the scroll (PSS / RSS MB):

- skvaller: 105 / 155
- WebKitWebProces: 418 / 466
- WebKitNetworkPr: 37 / 61

Scroll CPU by thread:

- WebKitWebProces: WebKitWebProces: 38.08 s
- skvaller: skvaller: 16.59 s
- WebKitWebProces: eadedCompositor: 12.33 s
- WebKitWebProces: SkiaCPUWorker: 5.82 s
- WebKitWebProces: ReceiveQueue: 5.42 s
- WebKitWebProces: Core: Scrolling: 4.98 s
- skvaller: ReceiveQueue: 4.94 s
- WebKitWebProces: WebKitWebP:cs: 2.83 s
- skvaller: skvaller:cs: 0.96 s
- WebKitWebProces: WebKitWe:gdrv: 0.89 s
- WebKitWebProces: EventDispatcher: 0.77 s
- skvaller: VBlankMonitor: 0.45 s

Screenshot after the scroll: `results/steam-deck-tauri1.png`

## rust1

Processes after the scroll (PSS / RSS MB):

- skvaller: 177 / 179

Scroll CPU by thread:

- skvaller: skvaller: 116.79 s
- skvaller: tokio-rt-worker: 16.95 s
- skvaller: StyleThread#: 2.50 s
- skvaller: WSI swapchain e: 1.55 s
- skvaller: WSI swapchain q: 0.93 s

Screenshot after the scroll: `results/steam-deck-rust1.png`

## tauri2

Processes after the scroll (PSS / RSS MB):

- skvaller: 110 / 159
- WebKitWebProces: 420 / 468
- WebKitNetworkPr: 37 / 62

Scroll CPU by thread:

- WebKitWebProces: WebKitWebProces: 38.33 s
- skvaller: skvaller: 16.73 s
- WebKitWebProces: eadedCompositor: 12.29 s
- WebKitWebProces: SkiaCPUWorker: 5.82 s
- WebKitWebProces: ReceiveQueue: 5.44 s
- WebKitWebProces: Core: Scrolling: 5.02 s
- skvaller: ReceiveQueue: 5.01 s
- WebKitWebProces: WebKitWebP:cs: 2.86 s
- skvaller: skvaller:cs: 0.96 s
- WebKitWebProces: WebKitWe:gdrv: 0.88 s
- WebKitWebProces: EventDispatcher: 0.78 s
- skvaller: VBlankMonitor: 0.45 s

Screenshot after the scroll: `results/steam-deck-tauri2.png`

## rust2

Processes after the scroll (PSS / RSS MB):

- skvaller: 176 / 179

Scroll CPU by thread:

- skvaller: skvaller: 116.60 s
- skvaller: tokio-rt-worker: 16.73 s
- skvaller: StyleThread#: 2.46 s
- skvaller: WSI swapchain e: 1.54 s
- skvaller: WSI swapchain q: 0.93 s

Screenshot after the scroll: `results/steam-deck-rust2.png`

## Notes

- Every screenshot (tauri1, tauri2, rust1, rust2) shows the bottom of the list, ending at Test
  article 5000. No "rendered in software" warning for either app; the container has
  `vulkan-radeon` (Mesa 26.2.6).
- The runs of each app agree within about 1–2% (Tauri scroll CPU 101.8 / 102.4 s, Rust
  138.8 / 138.3 s).
- Setup: Steam Deck in Desktop Mode (KDE Plasma, Wayland), plugged in, screen on, game mode
  and other heavy apps closed. Both apps were built and run inside the `skvaller` distrobox
  (Arch) and ran on **Xwayland** (`GDK_BACKEND=x11`, no `WAYLAND_DISPLAY`), so both pay the
  same Xwayland cost.
- The all-Rust app needed 7,400 notches to reach the bottom, Tauri 5,600 (one notch scrolls
  less far in the Rust app). Per notch, scroll CPU is about the same: 18.2 ms for Tauri and
  18.7 ms for Rust. For the same distance, the Rust app used about 36% more CPU.
- Almost all of the Rust app's scroll CPU is on its main thread (about 117 s), plus about 17 s on
  `tokio-rt-worker`. Tauri's scroll CPU is spread over WebKit's web process (main thread,
  compositor, Skia workers) and its UI process.
- After the scroll, the Rust app held 177 MB PSS (no growth from launch). Tauri held about
  560 MB, up from about 370 MB at launch.
- The two apps opened in different themes (Tauri dark, Rust light). Each used the fixture's
  settings as it reads them; this was not changed for the run.
