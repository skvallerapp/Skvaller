# dev-server-xvfb

- Date: 2026-11-11
- Machine: 12th Gen Intel(R) Core(TM) i5-12400T, 7947 MB RAM, Debian GNU/Linux 13 (trixie) (kernel 6.12.113+deb13-cloud-amd64), session ?, apps on X11/Xwayland
- Builds: tauri `612f433 Open windows in their settings' theme and font from the first paint`, rust `1d80478 Add a CPU and memory test comparing the Tauri and all-Rust builds`
- Settings: 1 runs, scroll to the bottom of the list, warm-up off
- Memory columns: PSS / RSS in MB, summed over the app and its child processes. PSS splits shared pages between processes; use it to compare.

| Run | Window after | Procs | Launch CPU | Launch PSS/RSS | Idle 30 s CPU | Idle PSS/RSS | Notches | Scroll time | Scroll CPU | After scroll PSS/RSS |
|---|---|---|---|---|---|---|---|---|---|---|
| tauri1 | 35.3 s | 3 | 2.42 s | 513 / 687 | 0.01 s | 503 / 677 | 5200 | 183 s | 135.30 s | 796 / 970 |
| rust1 | 0.2 s | 1 | 0.39 s | 313 / 318 | 0.00 s | 313 / 318 | 7400 | 264 s | 539.97 s | 321 / 326 |

## tauri1

Processes after the scroll (PSS / RSS MB):

- skvaller: 137 / 208
- WebKitWebProces: 617 / 686
- WebKitNetworkPr: 42 / 75

Scroll CPU by thread:

- WebKitWebProces: llvmpipe-: 63.63 s
- WebKitWebProces: eadedCompositor: 24.97 s
- WebKitWebProces: WebKitWebProces: 23.70 s
- skvaller: skvaller: 8.97 s
- WebKitWebProces: SkiaCPUWorker: 3.24 s
- WebKitWebProces: ReceiveQueue: 2.18 s
- skvaller: ReceiveQueue: 2.06 s
- WebKitWebProces: Core: Scrolling: 1.40 s
- WebKitWebProces: EventDispatcher: 0.35 s
- skvaller: VBlankMonitor: 0.24 s

**Warning: llvmpipe threads ran, so this app rendered in software, not on the GPU.**

Screenshot after the scroll: `results/dev-server-xvfb-tauri1.png`

## rust1

Processes after the scroll (PSS / RSS MB):

- skvaller: 321 / 326

Scroll CPU by thread:

- skvaller: llvmpipe-: 410.29 s
- skvaller: skvaller: 111.23 s
- skvaller: tokio-rt-worker: 8.97 s
- skvaller: WSI swapchain q: 8.71 s
- skvaller: StyleThread#: 0.58 s
- skvaller: WSI swapchain e: 0.12 s

**Warning: llvmpipe threads ran, so this app rendered in software, not on the GPU.**

Screenshot after the scroll: `results/dev-server-xvfb-rust1.png`
