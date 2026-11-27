//! The unread articles of every account on the app's icon, while the setting is on: in
//! the Dock on macOS, and on Linux in a dock or task manager that follows Unity's
//! launcher entries, such as KDE's, Dash to Dock or Plank; on Windows, which has no
//! badge count, as an overlay on the taskbar button.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use dioxus_native::prelude::*;
use dioxus_native::winit::raw_window_handle::RawWindowHandle;
use skvaller_app::App;

use crate::live::Live;

/// How long after a count failed to show it's tried again, and how many times: Windows
/// makes the taskbar button a moment after the window, and the overlay needs it.
const RETRY_AFTER: Duration = Duration::from_secs(1);
const RETRIES: u8 = 10;

/// Keeps the badge on the unread count, as the caches or the setting change.
pub fn use_unread_badge(app: &Arc<App>, live: &Live) {
    let live = *live;
    // Bumped on each cache change, so the effect counts again.
    let mut changed = use_signal(|| 0_u64);
    crate::live::use_cache_changes(app, move |_| *changed.write() += 1);
    // What the icon shows: nothing at launch. Only a new count is sent, so a sync's many
    // changes don't each wake the dock, and Linux never connects while there's none. A
    // count that failed to show is tried again shortly, and on the next change.
    let shown = use_hook(|| Rc::new(Cell::new(0_i64)));
    let retries = use_hook(|| Rc::new(Cell::new(0_u8)));
    let window = dioxus_native::use_raw_window_handle();
    let app = Arc::clone(app);
    use_effect(move || {
        changed();
        // The accounts, too: one added or removed.
        (live.revision)();
        let count: i64 = if live.settings.read().unread_badge {
            app.accounts()
                .iter()
                .filter_map(|a| app.unread_total(&a.id).ok())
                .sum()
        } else {
            0
        };
        if shown.get() == count {
            return;
        }
        if platform::show(&window, count) {
            shown.set(count);
            retries.set(0);
        } else if retries.get() < RETRIES {
            retries.set(retries.get() + 1);
            spawn(async move {
                tokio::time::sleep(RETRY_AFTER).await;
                *changed.write() += 1;
            });
        }
    });
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{MainThreadMarker, NSString};

    use super::RawWindowHandle;

    /// Shows `count` on the icon, or nothing for 0; false if it couldn't.
    pub fn show(_window: &RawWindowHandle, count: i64) -> bool {
        // Effects run on the main thread, where AppKit wants this.
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let label = (count > 0).then(|| NSString::from_str(&count.to_string()));
        NSApplication::sharedApplication(mtm)
            .dockTile()
            .setBadgeLabel(label.as_deref());
        true
    }
}

#[cfg(target_os = "linux")]
mod platform {
    use std::collections::HashMap;
    use std::sync::OnceLock;
    use std::sync::mpsc::{self, Sender};

    use zbus::blocking::Connection;
    use zbus::zvariant::Value;

    use super::RawWindowHandle;

    /// The desktop entry the dock finds the app's icon by.
    const APP_URI: &str = "application://skvaller.desktop";
    const PATH: &str = "/io/github/skvallerapp/Skvaller/LauncherEntry";
    const INTERFACE: &str = "com.canonical.Unity.LauncherEntry";

    /// Shows `count` on the icon, or nothing for 0; false if it couldn't. The dock is
    /// told on a thread of its own, so a failure there shows only in the log.
    pub fn show(_window: &RawWindowHandle, count: i64) -> bool {
        // Off the UI thread: connecting to the session bus may take a while.
        static SENDER: OnceLock<Option<Sender<i64>>> = OnceLock::new();
        let sender = SENDER.get_or_init(|| {
            let (sender, counts) = mpsc::channel::<i64>();
            std::thread::Builder::new()
                .name("badge".into())
                .spawn(move || {
                    let connection = match Connection::session() {
                        Ok(connection) => connection,
                        Err(err) => {
                            tracing::info!("the unread badge can't be shown: {err}");
                            return;
                        }
                    };
                    while let Ok(mut count) = counts.recv() {
                        // Only the latest of a burst.
                        while let Ok(next) = counts.try_recv() {
                            count = next;
                        }
                        let props = HashMap::from([
                            ("count", Value::from(count)),
                            ("count-visible", Value::from(count > 0)),
                        ]);
                        let sent = connection.emit_signal(
                            None::<()>,
                            PATH,
                            INTERFACE,
                            "Update",
                            &(APP_URI, props),
                        );
                        if let Err(err) = sent {
                            tracing::info!("the unread badge can't be shown: {err}");
                        }
                    }
                })
                .inspect_err(|err| tracing::warn!("starting the badge thread failed: {err}"))
                .ok()
                .map(|_| sender)
        });
        sender
            .as_ref()
            .is_some_and(|sender| sender.send(count).is_ok())
    }
}

#[cfg(target_os = "windows")]
mod platform {
    use std::cell::OnceCell;
    use std::ffi::c_void;

    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
    use windows::Win32::UI::WindowsAndMessaging::{CreateIcon, DestroyIcon, HICON};
    use windows::core::{HSTRING, PCWSTR};

    use super::{RawWindowHandle, overlay};
    use crate::i18n::t_count;

    /// Shows `count` on the taskbar button, or nothing for 0; false if it couldn't, as
    /// before the button is made.
    pub fn show(window: &RawWindowHandle, count: i64) -> bool {
        let RawWindowHandle::Win32(handle) = window else {
            return false;
        };
        let hwnd = HWND(handle.hwnd.get() as *mut c_void);
        let shown = taskbar().and_then(|taskbar| {
            if count == 0 {
                // SAFETY: a live window of this thread; no icon clears the overlay.
                return unsafe { taskbar.SetOverlayIcon(hwnd, HICON::default(), PCWSTR::null()) };
            }
            let icon = icon(&overlay::draw(count))?;
            let description = HSTRING::from(t_count("list.unread", count));
            // SAFETY: a live window of this thread and an icon made above. The taskbar
            // keeps its own copy, so the icon goes once it's set.
            unsafe {
                let set = taskbar.SetOverlayIcon(hwnd, icon, &description);
                let _ = DestroyIcon(icon);
                set
            }
        });
        shown
            .inspect_err(|err| tracing::info!("the unread badge can't be shown: {err}"))
            .is_ok()
    }

    /// The taskbar, made once for the UI thread.
    fn taskbar() -> windows::core::Result<ITaskbarList3> {
        thread_local! {
            static TASKBAR: OnceCell<ITaskbarList3> = const { OnceCell::new() };
        }
        TASKBAR.with(|cell| {
            if let Some(taskbar) = cell.get() {
                return Ok(taskbar.clone());
            }
            // SAFETY: COM calls on this thread. winit has usually started COM on it
            // already, which makes this a no-op.
            let taskbar: ITaskbarList3 = unsafe {
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let taskbar: ITaskbarList3 =
                    CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER)?;
                taskbar.HrInit()?;
                taskbar
            };
            Ok(cell.get_or_init(|| taskbar).clone())
        })
    }

    /// An icon of `rgba`, `overlay::SIZE` pixels square.
    fn icon(rgba: &[u8]) -> windows::core::Result<HICON> {
        let width = usize::from(overlay::SIZE);
        let mut bgra = rgba.to_vec();
        // One bit a pixel, rows padded to 16 bits, set where the pixel is clear: what
        // shows where Windows draws the icon without its alpha, as winit makes it.
        let stride = width.div_ceil(16) * 2;
        let mut mask = vec![0_u8; stride * width];
        for (i, pixel) in bgra.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            pixel.swap(0, 2);
            if pixel[3] == 0 {
                let (y, x) = (i / width, i % width);
                mask[y * stride + x / 8] |= 0x80 >> (x % 8);
            }
        }
        let size = i32::from(overlay::SIZE);
        // SAFETY: both buffers hold `size`×`size` pixels at their depths.
        unsafe { CreateIcon(None, size, size, 1, 32, mask.as_ptr(), bgra.as_ptr()) }
    }
}

/// The unread count as a small image, for the Windows taskbar's overlay: a red circle with
/// the number in white, as Teams and Outlook show theirs. Drawn here, so it needs no font
/// and no image files.
#[cfg(any(target_os = "windows", test))]
mod overlay {
    /// Width and height in pixels. Windows scales it to its overlay size, 16 at 100 %.
    pub const SIZE: u8 = 32;

    const RED: [u8; 3] = [0xd9, 0x30, 0x25];
    const WHITE: [u8; 3] = [0xff, 0xff, 0xff];

    /// Digits and a plus sign, three pixels wide and five high, one row per line.
    const GLYPHS: [(char, [u8; 5]); 11] = [
        ('0', [0b111, 0b101, 0b101, 0b101, 0b111]),
        ('1', [0b010, 0b110, 0b010, 0b010, 0b111]),
        ('2', [0b111, 0b001, 0b111, 0b100, 0b111]),
        ('3', [0b111, 0b001, 0b011, 0b001, 0b111]),
        ('4', [0b101, 0b101, 0b111, 0b001, 0b001]),
        ('5', [0b111, 0b100, 0b111, 0b001, 0b111]),
        ('6', [0b111, 0b100, 0b111, 0b101, 0b111]),
        ('7', [0b111, 0b001, 0b001, 0b001, 0b001]),
        ('8', [0b111, 0b101, 0b111, 0b101, 0b111]),
        ('9', [0b111, 0b101, 0b111, 0b001, 0b111]),
        ('+', [0b000, 0b010, 0b111, 0b010, 0b000]),
    ];

    /// What the badge says: the count, or 99+ past that.
    pub fn label(count: i64) -> String {
        if count > 99 {
            "99+".to_owned()
        } else {
            count.to_string()
        }
    }

    /// The badge for `count`, as `SIZE`×`SIZE` RGBA pixels. Only for a count above 0: with
    /// none, the overlay goes.
    pub fn draw(count: i64) -> Vec<u8> {
        let size = usize::from(SIZE);
        let mut rgba = vec![0u8; size * size * 4];

        // The circle, its edge smoothed by sampling each pixel 4×4 times. In eighths of
        // a pixel, so it's all whole numbers: a sample's centre is at 8x + 2sx + 1.
        let r = i32::from(SIZE) * 4;
        for (y, py) in (0..i32::from(SIZE)).enumerate() {
            for (x, px) in (0..i32::from(SIZE)).enumerate() {
                let mut inside: u16 = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        let dx = 8 * px + 2 * sx + 1 - r;
                        let dy = 8 * py + 2 * sy + 1 - r;
                        if dx * dx + dy * dy <= r * r {
                            inside += 1;
                        }
                    }
                }
                let i = (y * size + x) * 4;
                rgba[i..i + 3].copy_from_slice(&RED);
                // At most 16 × 255 / 16.
                rgba[i + 3] = u8::try_from(inside * 255 / 16).unwrap_or(u8::MAX);
            }
        }

        // The text, as large as fits: fewer characters, bigger pixels.
        let text = label(count);
        let glyphs: Vec<[u8; 5]> = text
            .chars()
            .filter_map(|c| GLYPHS.iter().find(|(g, _)| *g == c).map(|(_, rows)| *rows))
            .collect();
        let scale = match glyphs.len() {
            1 => 4,
            2 => 3,
            _ => 2,
        };
        let gap = scale;
        let width = glyphs.len() * 3 * scale + (glyphs.len().saturating_sub(1)) * gap;
        let height = 5 * scale;
        let (left, top) = ((size - width) / 2, (size - height) / 2);
        for (n, rows) in glyphs.iter().enumerate() {
            let x0 = left + n * (3 * scale + gap);
            for (row, bits) in rows.iter().enumerate() {
                for col in 0..3 {
                    if bits & (0b100 >> col) == 0 {
                        continue;
                    }
                    for y in top + row * scale..top + (row + 1) * scale {
                        for x in x0 + col * scale..x0 + (col + 1) * scale {
                            let i = (y * size + x) * 4;
                            rgba[i..i + 3].copy_from_slice(&WHITE);
                            rgba[i + 3] = 255;
                        }
                    }
                }
            }
        }
        rgba
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn pixel(rgba: &[u8], x: usize, y: usize) -> [u8; 4] {
            let i = (y * usize::from(SIZE) + x) * 4;
            rgba[i..i + 4].try_into().unwrap()
        }

        #[test]
        fn counts_past_99_read_99_plus() {
            assert_eq!(label(7), "7");
            assert_eq!(label(42), "42");
            assert_eq!(label(99), "99");
            assert_eq!(label(100), "99+");
            assert_eq!(label(12_345), "99+");
        }

        #[test]
        fn a_red_circle_with_white_text_and_clear_corners() {
            for count in [1, 8, 42, 99, 100, 5_000] {
                let rgba = draw(count);
                assert_eq!(rgba.len(), usize::from(SIZE) * usize::from(SIZE) * 4);
                assert_eq!(pixel(&rgba, 0, 0)[3], 0, "corner of {count}");
                assert_eq!(pixel(&rgba, 31, 31)[3], 0, "corner of {count}");
                // Near the edge, inside the circle: red.
                assert_eq!(
                    pixel(&rgba, 16, 2),
                    [0xd9, 0x30, 0x25, 255],
                    "edge of {count}"
                );
                let white = rgba
                    .chunks(4)
                    .filter(|p| p == &[0xff, 0xff, 0xff, 255])
                    .count();
                assert!(white > 20, "text of {count}");
            }
        }

        #[test]
        fn the_text_is_centred_and_fits() {
            // 8 is a full block of three columns at the largest size: 12×20 pixels.
            let rgba = draw(8);
            let white: Vec<(usize, usize)> = (0..32)
                .flat_map(|y| (0..32).map(move |x| (x, y)))
                .filter(|&(x, y)| pixel(&rgba, x, y) == [0xff, 0xff, 0xff, 255])
                .collect();
            let xs = (
                white.iter().map(|p| p.0).min().unwrap(),
                white.iter().map(|p| p.0).max().unwrap(),
            );
            let ys = (
                white.iter().map(|p| p.1).min().unwrap(),
                white.iter().map(|p| p.1).max().unwrap(),
            );
            assert_eq!(xs, (10, 21));
            assert_eq!(ys, (6, 25));
        }
    }
}
