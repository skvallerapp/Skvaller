//! The top border Windows 10 leaves out on a window without a title bar.
//!
//! With the winit fork's `with_undecorated_resize_borders`, Windows 10 draws its 1px
//! window border on the left, right and bottom, but not on top: any non-client area
//! there makes DWM draw a caption above the window, so the top has none. chrome.rs's
//! `Frame` draws the top border in its place, in DWM's colors. Windows 11 draws the
//! border all round, over the window's content, so there this draws nothing.

use std::ptr;

use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ, RegGetValueW,
};

/// The first build of Windows 11, which draws the border over the window's content.
const WINDOWS_11: u32 = 22_000;

/// DWM's settings: whether the accent color shows on title bars and window borders,
/// and the color.
const DWM: &str = r"Software\Microsoft\Windows\DWM";

/// The border of an inactive window, as DWM draws it on Windows 10: a gray at about
/// half strength over what's behind. Fitted to DWM's left and bottom borders over 70
/// points of the desktop, on Windows 10 22H2 (19045).
const INACTIVE: &str = "rgba(94, 94, 94, 0.48)";

/// The border of an active window when the accent color doesn't show on borders: a
/// darker gray, stronger. Fitted as [`INACTIVE`] is.
const ACTIVE: &str = "rgba(62, 62, 62, 0.63)";

/// Whether the window needs the app's top border: on Windows 10, not on 11. Not when
/// the build can't be read.
pub fn needed() -> bool {
    build().is_some_and(|build| build < WINDOWS_11)
}

/// The top border's color for a window that is `active`, as DWM draws the others: the
/// accent color while active, when the user shows it on window borders.
pub fn color(active: bool) -> String {
    match accent().filter(|_| active) {
        Some([r, g, b]) => format!("rgb({r} {g} {b})"),
        None if active => ACTIVE.to_owned(),
        None => INACTIVE.to_owned(),
    }
}

/// The accent color, when "Show accent color on title bars and window borders" is on.
fn accent() -> Option<[u8; 3]> {
    if dword(HKEY_CURRENT_USER, DWM, "ColorPrevalence")? != 1 {
        return None;
    }
    // 0xAABBGGRR.
    let [r, g, b, _] = dword(HKEY_CURRENT_USER, DWM, "AccentColor")?.to_le_bytes();
    Some([r, g, b])
}

/// The Windows build number.
fn build() -> Option<u32> {
    let path = wide(r"SOFTWARE\Microsoft\Windows NT\CurrentVersion");
    let name = wide("CurrentBuildNumber");
    let mut buf = [0u16; 32];
    let mut size = u32::try_from(size_of_val(&buf)).ok()?;
    // SAFETY: the strings end in a NUL, and `buf` holds `size` bytes, which the call
    // writes the value into, NUL included, or fails.
    let err = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            path.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_SZ,
            ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &raw mut size,
        )
    };
    if err != 0 {
        return None;
    }
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16(&buf[..len]).ok()?.trim().parse().ok()
}

/// The DWORD value `name` under `path` in `key`.
fn dword(key: HKEY, path: &str, name: &str) -> Option<u32> {
    let (path, name) = (wide(path), wide(name));
    let mut value = 0u32;
    let mut size = u32::try_from(size_of::<u32>()).ok()?;
    // SAFETY: the strings end in a NUL, and `value` holds the 4 bytes `size` says,
    // which the call writes a DWORD into, or fails.
    let err = unsafe {
        RegGetValueW(
            key,
            path.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            ptr::null_mut(),
            (&raw mut value).cast(),
            &raw mut size,
        )
    };
    (err == 0).then_some(value)
}

/// `s` as a NUL-terminated UTF-16 string.
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
