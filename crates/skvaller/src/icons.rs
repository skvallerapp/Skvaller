//! Lucide icons (lucide.dev, ISC license), drawn in the text color.

use dioxus_native::prelude::*;

/// A Lucide icon from its paths.
#[component]
pub fn Icon(size: u32, stroke: &'static str, paths: &'static [&'static str]) -> Element {
    rsx! {
        svg {
            width: "{size}",
            height: "{size}",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            stroke_width: stroke,
            stroke_linecap: "round",
            stroke_linejoin: "round",
            "aria-hidden": "true",
            for d in paths {
                path { d: *d }
            }
        }
    }
}

pub const PALETTE: &[&str] = &[
    "M12 22a1 1 0 0 1 0-20 10 9 0 0 1 10 9 5 5 0 0 1-5 5h-2.25a1.75 1.75 0 0 0-1.4 2.8l.3.4a1.75 1.75 0 0 1-1.4 2.8z",
    "M13.5 6.5h.01",
    "M17.5 10.5h.01",
    "M6.5 12.5h.01",
    "M8.5 7.5h.01",
];
pub const BOOK_OPEN: &[&str] = &[
    "M12 7v14",
    "M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3z",
];
pub const CIRCLE_USER: &[&str] = &[
    "M22 12a10 10 0 1 1-20 0 10 10 0 0 1 20 0",
    "M15 10a3 3 0 1 1-6 0 3 3 0 0 1 6 0",
    "M7 20.662V19a2 2 0 0 1 2-2h6a2 2 0 0 1 2 2v1.662",
];
pub const CHECK: &[&str] = &["M20 6 9 17l-5-5"];
pub const CHEVRONS_UP_DOWN: &[&str] = &["m7 15 5 5 5-5", "m7 9 5-5 5 5"];
pub const CHEVRON_LEFT: &[&str] = &["m15 18-6-6 6-6"];
pub const CHEVRON_RIGHT: &[&str] = &["m9 18 6-6-6-6"];
pub const LAYERS: &[&str] = &[
    "M12.83 2.18a2 2 0 0 0-1.66 0L2.6 6.08a1 1 0 0 0 0 1.83l8.58 3.91a2 2 0 0 0 1.66 0l8.58-3.9a1 1 0 0 0 0-1.83z",
    "M2 12a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 12",
    "M2 17a1 1 0 0 0 .58.91l8.6 3.91a2 2 0 0 0 1.65 0l8.58-3.9A1 1 0 0 0 22 17",
];
pub const LAPTOP: &[&str] = &[
    "M18 5a2 2 0 0 1 2 2v8.526a2 2 0 0 0 .212.897l1.068 2.127a1 1 0 0 1-.9 1.45H3.62a1 1 0 0 1-.9-1.45l1.068-2.127A2 2 0 0 0 4 15.526V7a2 2 0 0 1 2-2z",
    "M20.054 15.987H3.946",
];
pub const KEYBOARD: &[&str] = &[
    "M10 8h.01",
    "M12 12h.01",
    "M14 8h.01",
    "M16 12h.01",
    "M18 8h.01",
    "M6 8h.01",
    "M7 16h10",
    "M8 12h.01",
    // Lucide's rect (x 2, y 4, 20 by 16, radius 2) as a path.
    "M4 4h16a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2z",
];
pub const SEARCH: &[&str] = &["m21 21-4.34-4.34", "M11 3a8 8 0 1 0 0 16 8 8 0 0 0 0-16z"];
/// Lucide's settings, its circle as a path.
pub const SETTINGS: &[&str] = &[
    "M9.671 4.136a2.34 2.34 0 0 1 4.659 0 2.34 2.34 0 0 0 3.319 1.915 2.34 2.34 0 0 1 2.33 4.033 2.34 2.34 0 0 0 0 3.831 2.34 2.34 0 0 1-2.33 4.033 2.34 2.34 0 0 0-3.319 1.915 2.34 2.34 0 0 1-4.659 0 2.34 2.34 0 0 0-3.32-1.915 2.34 2.34 0 0 1-2.33-4.033 2.34 2.34 0 0 0 0-3.831A2.34 2.34 0 0 1 6.35 6.051a2.34 2.34 0 0 0 3.319-1.915",
    "M9 12a3 3 0 1 0 6 0a3 3 0 1 0 -6 0",
];
/// Lucide's panel-left, its rect as a path.
pub const PANEL_LEFT: &[&str] = &[
    "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z",
    "M9 3v18",
];
pub const ROTATE_CW: &[&str] = &[
    "M21 12a9 9 0 1 1-9-9c2.52 0 4.93 1 6.74 2.74L21 8",
    "M21 3v5h-5",
];
pub const PLUS: &[&str] = &["M5 12h14", "M12 5v14"];
/// Lucide's rss, its dot as a path.
pub const RSS: &[&str] = &[
    "M4 11a9 9 0 0 1 9 9",
    "M4 4a16 16 0 0 1 16 16",
    "M4 19a1 1 0 1 0 2 0a1 1 0 1 0 -2 0",
];
pub const X: &[&str] = &["M18 6 6 18", "m6 6 12 12"];
pub const CIRCLE_CHECK: &[&str] = &["M2 12a10 10 0 1 0 20 0a10 10 0 1 0 -20 0", "m9 12 2 2 4-4"];
pub const SQUARE_TEXT: &[&str] = &[
    "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2z",
    "M7 8h8",
    "M7 12h10",
    "M7 16h6",
];
/// [`SQUARE_TEXT`] filled, its lines cut out: the square's outline at a stroke of 2.25,
/// the lines as round-capped holes, for `fill-rule: evenodd`.
pub const SQUARE_TEXT_FILLED: &str = "M5 1.875h14a3.125 3.125 0 0 1 3.125 3.125v14a3.125 3.125 0 0 1-3.125 3.125H5a3.125 3.125 0 0 1-3.125-3.125V5A3.125 3.125 0 0 1 5 1.875z\
    M7 6.875h8a1.125 1.125 0 0 1 0 2.25h-8a1.125 1.125 0 0 1 0-2.25z\
    M7 10.875h10a1.125 1.125 0 0 1 0 2.25h-10a1.125 1.125 0 0 1 0-2.25z\
    M7 14.875h6a1.125 1.125 0 0 1 0 2.25h-6a1.125 1.125 0 0 1 0-2.25z";
pub const GLOBE: &[&str] = &[
    "M2 12a10 10 0 1 0 20 0a10 10 0 1 0 -20 0",
    "M12 2a14.5 14.5 0 0 0 0 20 14.5 14.5 0 0 0 0-20",
    "M2 12h20",
];
/// The window buttons Skvaller draws on Linux, as GNOME's: Lucide's minus, a smaller
/// square, and copy. KDE's and Windows' are in `chrome`, drawn as Breeze and
/// Windows 11 draw them.
pub const WINDOW_MINIMIZE: &[&str] = &["M5 12h14"];
pub const WINDOW_MAXIMIZE: &[&str] =
    &["M7 5h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V7a2 2 0 0 1 2-2z"];
pub const WINDOW_RESTORE: &[&str] = &[
    "M10 8h10a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H10a2 2 0 0 1-2-2V10a2 2 0 0 1 2-2z",
    "M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2",
];
