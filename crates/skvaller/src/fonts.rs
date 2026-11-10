//! The bundled fonts and the CSS font stacks the font settings pick from.
//!
//! The stacks are the Tauri version's `fonts.ts`, for the `uiFont`, `headlineFont` and
//! `articleFont` settings.
//!
//! The bundled fonts are the variable versions from Google Fonts, under the SIL Open Font
//! License (`fonts/*/OFL.txt`). They cover at least Latin, Cyrillic and Vietnamese;
//! every other script falls back to the system's fonts.

use std::sync::Arc;

use dioxus_native::FontContext;
use parley::fontique::Blob;
use skvaller_app::Font;

const BUNDLED: [&[u8]; 8] = [
    include_bytes!("../fonts/inter/Inter-Variable.ttf"),
    include_bytes!("../fonts/inter/Inter-Italic-Variable.ttf"),
    include_bytes!("../fonts/geist/Geist-Variable.ttf"),
    include_bytes!("../fonts/geist/Geist-Italic-Variable.ttf"),
    include_bytes!("../fonts/sourceserif4/SourceSerif4-Variable.ttf"),
    include_bytes!("../fonts/sourceserif4/SourceSerif4-Italic-Variable.ttf"),
    include_bytes!("../fonts/literata/Literata-Variable.ttf"),
    include_bytes!("../fonts/literata/Literata-Italic-Variable.ttf"),
];

/// The system's fonts with the bundled ones added. Article frames get a copy.
pub fn font_context() -> FontContext {
    let mut ctx = FontContext::new();
    for data in BUNDLED {
        ctx.collection
            .register_fonts(Blob::new(Arc::new(data) as _), None);
    }
    ctx
}

const SANS: &str = "system-ui, 'Segoe UI', 'Noto Sans', sans-serif";
/// `ui-serif` is New York in a browser on macOS, but Blitz doesn't know it yet: Georgia
/// stands in.
const SERIF: &str = "ui-serif, Georgia, 'Noto Serif', serif";

/// The CSS font stack for `font`.
pub fn stack(font: Font) -> String {
    match font {
        Font::Inter => format!("'Inter', {SANS}"),
        Font::Geist => format!("'Geist', {SANS}"),
        Font::System => SANS.to_owned(),
        Font::SourceSerif => format!("'Source Serif 4', {SERIF}"),
        Font::Literata => format!("'Literata', {SERIF}"),
        Font::Serif => SERIF.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_font_registers_under_its_settings_name() {
        let mut ctx = font_context();
        for family in ["Inter", "Geist", "Source Serif 4", "Literata"] {
            assert!(
                ctx.collection.family_by_name(family).is_some(),
                "{family} isn't registered"
            );
        }
    }
}
