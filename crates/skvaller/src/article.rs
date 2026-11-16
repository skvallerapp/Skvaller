//! The reader's document for one article: Skvaller's header over the article's HTML.
//!
//! The document goes into an `<iframe srcdoc>`, a document of its own, so the article's
//! styles can't reach the app and the app's can't reach the article.

use skvaller_app::{Font, FontSize, LineWidth, Settings, Theme};
use skvaller_core::{ArticleSummary, Feed, ready_article};

use crate::fonts;
use crate::scale::{self, Role};

const FRAME_CSS: &str = include_str!("frame.css");

/// What the reader's document takes from the settings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Look {
    pub theme: Theme,
    pub article_font: Font,
    pub headline_font: Font,
    pub font_size: FontSize,
    pub line_width: LineWidth,
    /// Settings → Animations: the article fades in as it opens.
    pub animations: bool,
}

impl Look {
    /// The look of `settings`, in `theme`: the setting resolved for the system's mode
    /// (`Live::theme`), not the setting itself.
    pub fn new(settings: &Settings, theme: Theme) -> Self {
        Self {
            theme,
            article_font: settings.article_font,
            headline_font: settings.headline_font,
            font_size: settings.font_size,
            line_width: settings.line_width,
            animations: settings.animations,
        }
    }
}

/// The whole reader document for `article`, with `html` as its untrusted body and
/// `date` as its header shows it, in the theme and fonts of `look`.
pub fn document(
    article: &ArticleSummary,
    feed: Option<&Feed>,
    html: &str,
    date: &str,
    look: Look,
) -> String {
    let theme = theme_name(look.theme);
    let motion = crate::ui::motion(look.animations);
    // The column holds as many characters at any size: its width is in `em`s.
    let line_width = match look.line_width {
        LineWidth::Narrow => "32em",
        LineWidth::Medium => "38em",
        LineWidth::Wide => "46em",
    };
    let fonts = format!(
        "--article-font: {}; --headline-font: {}; --line-width: {line_width}{}",
        fonts::stack(look.article_font),
        fonts::stack(look.headline_font),
        scale::vars(look.font_size, &[Role::Article, Role::ArticleMeta]),
    );
    let date = escape(date);
    let title = linked(&article.title, Some(&article.url));
    let author = escape(&article.author);
    let source = feed.map_or_else(String::new, |f| linked(&f.title, f.site_url.as_deref()));
    let body = ready_article(html, &article.url);
    format!(
        "<!doctype html><html data-theme=\"{theme}\" data-motion=\"{motion}\" style=\"{fonts}\"><head><meta charset=\"utf-8\">\
         <style>{FRAME_CSS}</style></head><body><article><header>\
         <div class=\"date\">{date}</div><h1>{title}</h1>\
         <div class=\"source\">{author}<br>{source}</div></header>{body}</article></body></html>"
    )
}

/// The theme's name, as the CSS selects it in `data-theme`.
pub fn theme_name(theme: Theme) -> &'static str {
    match theme {
        Theme::System => "system",
        Theme::Light => "light",
        Theme::Dark => "dark",
        Theme::Espresso => "espresso",
    }
}

/// Escaped text, as a link when there is a URL. The link opens in the browser, as the
/// article's own links do.
fn linked(text: &str, url: Option<&str>) -> String {
    match url.filter(|u| !u.is_empty()) {
        Some(url) => format!(
            "<a href=\"{}\" target=\"_blank\">{}</a>",
            escape(url),
            escape(text)
        ),
        None => escape(text),
    }
}

/// `text` safe to put in HTML text and in quoted attributes.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_is_escaped_and_the_body_readied() {
        let article = ArticleSummary {
            id: "1".into(),
            feed_id: "f".into(),
            title: "<b>Bold</b>".into(),
            author: "A & B".into(),
            url: "https://example.com/a".into(),
            published: 0,
            read: false,
            starred: false,
            thumbnail: false,
        };
        let doc = document(
            &article,
            None,
            "<img src=\"/i.png\">",
            "today",
            Look::new(&Settings::default(), Theme::Dark),
        );
        assert!(doc.contains("&lt;b&gt;Bold&lt;/b&gt;"));
        assert!(doc.contains("A &amp; B"));
        assert!(doc.contains("<a href=\"https://example.com/a\" target=\"_blank\">"));
        assert!(doc.contains("src=\"https://example.com/i.png\""));
        assert!(doc.contains("data-theme=\"dark\""));
        assert!(doc.contains("--article-font: 'Literata'"));
        assert!(doc.contains("--line-width: 38em; --article-scale: 1;"));
    }
}
