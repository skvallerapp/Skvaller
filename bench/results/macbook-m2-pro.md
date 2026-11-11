# macbook-m2-pro

- Date: 2026-11-11
- Machine: Apple M2 Pro, 16384 MB RAM, macOS 27.2
- Builds: tauri `612f433 Open windows in their settings' theme and font from the first paint`, rust `d4ea5b8 Ignore what the tauri branch leaves behind, and dev/`
- Settings: 2 runs, scroll to the bottom of the list, warm-up on
- Memory columns: footprint / RSS in MB, summed over the app and its child processes. Footprint is what Activity Monitor shows as Memory; use it to compare. WebKit's processes are found as the com.apple.WebKit.* processes that appear after launch.

| Run | Window after | Procs | Launch CPU | Launch footprint/RSS | Idle 30 s CPU | Idle footprint/RSS | Notches | Scroll time | Scroll CPU | After scroll footprint/RSS |
|---|---|---|---|---|---|---|---|---|---|---|
| tauri1 | 0.3 s | 4 | 0.85 s | 150 / 301 | 0.01 s | 154 / 217 | 3700 | 150 s | 63.57 s | 297 / 625 |
| rust1 | 0.5 s | 1 | 0.36 s | 189 / 128 | 0.01 s | 189 / 127 | 2600 | 107 s | 58.43 s | 212 / 168 |
| tauri2 | 0.5 s | 4 | 0.82 s | 135 / 245 | 0.01 s | 136 / 200 | 3700 | 150 s | 63.29 s | 255 / 468 |
| rust2 | 0.5 s | 1 | 0.37 s | 196 / 125 | 0.10 s | 182 / 127 | 2600 | 108 s | 58.98 s | 216 / 169 |

## tauri1

Processes after the scroll (footprint / RSS MB):

- skvaller: 40 / 104
- com.apple.WebKit.GPU: 17 / 48
- com.apple.WebKit.WebContent: 234 / 466
- com.apple.WebKit.Networking: 6 / 6

Screenshot after the scroll: `results/macbook-m2-pro-tauri1.png`

## rust1

Processes after the scroll (footprint / RSS MB):

- skvaller: 212 / 167

Screenshot after the scroll: `results/macbook-m2-pro-rust1.png`

## tauri2

Processes after the scroll (footprint / RSS MB):

- skvaller: 38 / 106
- com.apple.WebKit.GPU: 16 / 36
- com.apple.WebKit.WebContent: 193 / 316
- com.apple.WebKit.Networking: 6 / 7

Screenshot after the scroll: `results/macbook-m2-pro-tauri2.png`

## rust2

Processes after the scroll (footprint / RSS MB):

- skvaller: 216 / 168

Screenshot after the scroll: `results/macbook-m2-pro-rust2.png`

## Notes

- Every screenshot (tauri1, tauri2, rust1, rust2) shows the bottom of the list: Test article 4992 to 5000, the last one fully visible. Both apps showed "Sync failed", as expected.
- Setup: MacBook Pro M2 Pro on power, run from Ghostty (which has Accessibility and Screen Recording access), other apps open but idle. The pointer rested over the list, so article 4996 shows a hover highlight in all screenshots.
- The runs agree within about 1% on CPU (scroll: Tauri 63.6/63.3 s, all-Rust 58.4/59.0 s). Tauri's memory after the scroll varies more between runs (footprint 297 vs 255 MB), all of it in WebKit's WebContent process (234 vs 193 MB); the all-Rust app's stays within 2%.
- One notch scrolls further in the all-Rust app (2,600 notches to the bottom vs 3,700), so per notch it uses more CPU (about 22.6 ms vs 17.1 ms), but for the same distance it uses about 7% less in total.
- The screenshots were cropped to the app window after the run; the script captures the whole screen.
