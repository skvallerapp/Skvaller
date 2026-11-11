# dev-server-xvfb

- Date: 2026-11-11
- Machine: 12th Gen Intel(R) Core(TM) i5-12400T, 7947 MB RAM, Debian GNU/Linux 13 (trixie) (kernel 6.12.113+deb13-cloud-amd64), session ?, apps on X11/Xwayland
- Builds: tauri `612f433 Open windows in their settings' theme and font from the first paint`, rust `6e0a81b Crop Large thumbnails to their box`
- Settings: 1 runs, 360 notches, warm-up off
- Memory columns: PSS / RSS in MB, summed over the app and its child processes. PSS splits shared pages between processes; use it to compare.

| Run | Window after | Procs | Launch CPU | Launch PSS/RSS | Idle 30 s CPU | Idle PSS/RSS | Scroll CPU | After scroll PSS/RSS |
|---|---|---|---|---|---|---|---|---|
| tauri1 | 35.3 s | 3 | 2.98 s | 510 / 693 | 0.02 s | 500 / 683 | 10.28 s | 775 / 958 |
| rust1 | 0.2 s | 1 | 0.82 s | 313 / 318 | 0.00 s | 313 / 318 | 24.51 s | 304 / 309 |

## tauri1

Processes after the scroll (PSS / RSS MB):

- skvaller: 137 / 213
- WebKitWebProces: 596 / 670
- WebKitNetworkPr: 42 / 75

Scroll CPU by thread:

- WebKitWebProces: llvmpipe-: 4.57 s
- WebKitWebProces: eadedCompositor: 2.06 s
- WebKitWebProces: WebKitWebProces: 1.80 s
- skvaller: skvaller: 0.64 s
- WebKitWebProces: SkiaCPUWorker: 0.22 s
- WebKitWebProces: ReceiveQueue: 0.16 s
- skvaller: ReceiveQueue: 0.14 s
- WebKitWebProces: Core: Scrolling: 0.14 s
- WebKitWebProces: JITWorker: 0.10 s

**Warning: llvmpipe threads ran, so this app rendered in software, not on the GPU.**

Screenshot after the scroll: `results/dev-server-xvfb-tauri1.png`

## rust1

Processes after the scroll (PSS / RSS MB):

- skvaller: 304 / 309

Scroll CPU by thread:

- skvaller: llvmpipe-: 18.39 s
- skvaller: skvaller: 5.27 s
- skvaller: tokio-rt-worker: 0.41 s
- skvaller: WSI swapchain q: 0.37 s

**Warning: llvmpipe threads ran, so this app rendered in software, not on the GPU.**

Screenshot after the scroll: `results/dev-server-xvfb-rust1.png`
