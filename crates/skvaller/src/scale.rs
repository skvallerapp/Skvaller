//! How the font size setting scales the main window's text.
//!
//! It isn't a uniform zoom: each kind of text has its own range, its growth from XXXS
//! to XXXL, so titles barely grow while the text read and scanned grows a lot. A kind's
//! size at step `s` (-4 to 4) is its size at M times `range^(s / 8)`, so M is the size
//! app.css and frame.css draw and the middle of each range. Padding, column widths and
//! the toolbars' height stay; rows and icons grow with their text.
//!
//! app.css reads the factors as variables on the main window's root, which
//! [`root_style`] sets; the article's document gets its own through `article::Look`.
//! The sizes the article list and the sidebar are placed from are here too, so the
//! CSS and the arithmetic agree.

use std::fmt::Write as _;

use skvaller_app::FontSize;

/// A kind of text, with its own range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// Titles and labels: the account's name and status, the sidebar's heading, the
    /// list's title, a row's feed and time, the toolbar and view switcher icons.
    Label,
    /// The sidebar's folder and feed names, their chevrons and the rows' height.
    SidebarRow,
    /// The sidebar's unread counts.
    Count,
    /// The article list's headlines.
    Headline,
    /// The article's body, headline and headings.
    Article,
    /// The article's date and source.
    ArticleMeta,
}

impl Role {
    /// The growth from XXXS to XXXL.
    const fn range(self) -> f64 {
        match self {
            Self::Label => 1.25,
            Self::SidebarRow => 1.65,
            Self::Count => 1.9,
            Self::Headline => 1.8,
            Self::Article => 1.4,
            Self::ArticleMeta => 1.5,
        }
    }

    /// The CSS variable app.css or frame.css reads the factor from.
    const fn var(self) -> &'static str {
        match self {
            Self::Label => "--label-scale",
            Self::SidebarRow => "--row-scale",
            Self::Count => "--count-scale",
            Self::Headline => "--headline-scale",
            Self::Article => "--article-scale",
            Self::ArticleMeta => "--article-meta-scale",
        }
    }
}

/// What `role`'s sizes at M are multiplied by at `size`.
fn factor(size: FontSize, role: Role) -> f64 {
    role.range().powf(f64::from(size.step()) / 8.0)
}

/// `roles`' factors at `size` as CSS variables, for a `style` attribute.
pub fn vars(size: FontSize, roles: &[Role]) -> String {
    let mut style = String::new();
    for &role in roles {
        let _ = write!(style, "; {}: {}", role.var(), factor(size, role));
    }
    style
}

/// The main window's root's variables at `size` (app.css): its kinds of text, and the
/// sidebar's sizes the selection's highlight is placed from.
pub fn root_style(size: FontSize) -> String {
    let mut style = vars(
        size,
        &[Role::Label, Role::SidebarRow, Role::Count, Role::Headline],
    );
    let _ = write!(
        style,
        "; --sidebar-row: {}px; --heading-height: {}px",
        sidebar_row(size),
        heading_height(size)
    );
    style
}

/// A sidebar row's height (`.item`): 6 px above and below its line stay, and the line,
/// 16 px at M, as tall as a chevron, grows. Whole pixels, so rows never start between
/// pixels.
pub fn sidebar_row(size: FontSize) -> f64 {
    (12.0 + 16.0 * factor(size, Role::SidebarRow)).round()
}

/// The sidebar's Feeds heading's height (`.heading`), without its margins.
pub fn heading_height(size: FontSize) -> f64 {
    (14.0 * factor(size, Role::Label)).round()
}

/// The article list's headline's font size in CSS px (`.title`).
pub fn headline_px(size: FontSize) -> f64 {
    14.5 * factor(size, Role::Headline)
}

/// The article list's meta line's font size in CSS px (`.meta`).
pub fn meta_px(size: FontSize) -> f64 {
    12.0 * factor(size, Role::Label)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn m_is_todays_size_and_each_range_spans_its_growth() {
        for role in [
            Role::Label,
            Role::SidebarRow,
            Role::Count,
            Role::Headline,
            Role::Article,
            Role::ArticleMeta,
        ] {
            assert!((factor(FontSize::M, role) - 1.0).abs() < 1e-12);
            let growth = factor(FontSize::Xxxl, role) / factor(FontSize::Xxxs, role);
            assert!((growth - role.range()).abs() < 1e-9, "{role:?}");
        }
        assert_eq!(sidebar_row(FontSize::M), 28.0);
        assert_eq!(heading_height(FontSize::M), 14.0);
        assert!((headline_px(FontSize::M) - 14.5).abs() < 1e-12);
        // The table the sizes were approved from, which has them to about a quarter
        // pixel: XXXS and XXXL of one size of each kind.
        let near = |m: f64, role, (least, most): (f64, f64)| {
            for (size, want) in [(FontSize::Xxxs, least), (FontSize::Xxxl, most)] {
                let px = m * factor(size, role);
                assert!((px - want).abs() < 0.35, "{role:?} at {size:?}: {px}");
            }
        };
        near(30.0, Role::Label, (27.0, 33.5));
        near(13.0, Role::SidebarRow, (10.0, 17.0));
        near(12.0, Role::Count, (9.0, 16.5));
        near(14.5, Role::Headline, (11.0, 19.5));
        near(16.0, Role::Article, (13.5, 19.0));
        near(12.0, Role::ArticleMeta, (10.0, 14.5));
    }
}
