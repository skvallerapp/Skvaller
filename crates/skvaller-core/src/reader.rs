//! Untrusted article HTML, readied for showing in an isolated reader document.
//!
//! The renderer runs no scripts, and the reader doesn't rely on it: scripts, event
//! handlers, and script URLs in `href`, `src`, `poster` and `srcset` go. The rest makes what the article loads and links to
//! behave: resources and links are made absolute against the article's own URL (the
//! reader document's base is the app's, not the article's), resources are asked for
//! over https, media waits to be asked for, and forms can't be sent.

use std::collections::HashSet;
use std::fmt::Write;

use reqwest::Url;

use dom_query::Document;

use crate::html::{escape, parse};
use crate::net::absolute;
use crate::thumbnail::youtube_video;

/// `html` readied for the reader, with relative URLs resolved against `base`.
///
/// - [`ACTIVE`] elements go with their contents, `object` is unwrapped, and every `on…`
///   event handler goes: they would run, or steer the frame, in a renderer that ran
///   scripts.
/// - Web addresses written out in the text become links, outside links already.
/// - Links to other pages become absolute and get `target="_blank"`, so they leave the
///   reader's frame for the browser; one that can't be made absolute loses its `href`,
///   so it can't resolve against the app. Links within the article (`#…`) stay in it.
///   Only http, https and mailto links are kept: no other scheme should leave the frame.
/// - `src`, `poster` and `srcset` URLs, and other elements' `href` (SVG's, also as
///   `xlink:href`), become absolute, and `http:` becomes `https:`: most hosts serve
///   https too. Only web URLs, `data:` images and `#…` are kept.
/// - Audio and video don't autoplay or preload, and show their controls: without
///   scripts, controls are the only way to play.
/// - Forms are unwrapped, their fields kept: sending one would load any page into the
///   reader's frame, in place of the article and its header.
/// - A YouTube embed, lazy-loading or not, becomes its video's still, linked to the
///   video: the player needs scripts. So does a still of a video, as FreshRSS puts in
///   from the feed: it is uncropped. One still per video; a YouTube video's own article
///   without one starts with it. A Short's still is not cropped, and FreshRSS's link to
///   a shown video's old Flash player goes.
/// - Other iframes become a link to what they embed, named by their title or else its
///   URL: loaded, they would put a page in the reader, almost always a player that
///   needs scripts, and its host would see the reader. One that can't be linked, being
///   in a link already or of another scheme, goes; so does a repeat of one.
///
/// An article nested too deeply to parse safely gives an empty string.
#[must_use]
pub fn ready_article(html: &str, base: &str) -> String {
    let Some(doc) = parse(html) else {
        return String::new();
    };
    drop_flips(&doc);
    drop_active(&doc);
    let base = Url::parse(base).ok();
    let texts: Vec<_> = doc
        .root()
        .descendants_it()
        .filter(dom_query::NodeRef::is_text)
        .collect();
    for node in texts {
        // Most text has no address: look for one before walking up the tree.
        let Some(html) = linked(&node.text()) else {
            continue;
        };
        if !node
            .ancestors_it(None)
            .any(|a| UNLINKED.iter().any(|name| a.has_name(name)))
        {
            node.replace_with_html(html);
        }
    }
    ready_links(&doc, base.as_ref());
    doc.root().strip_elements(&["form"]);
    let mut stills: HashSet<String> = HashSet::new();
    // A Short's still has no black bars to crop.
    let short = base
        .as_ref()
        .is_some_and(|b| b.path().starts_with("/shorts/"));
    for el in doc.select("iframe, img").nodes() {
        // Lazy-loading pages put a placeholder in `src` (often the page itself) and
        // the embed in `data-src`. An empty or `#` one would resolve to the page.
        let id = ["src", "data-src"]
            .into_iter()
            .filter_map(|name| el.attr(name))
            .filter(|src| !src.trim().is_empty() && !src.trim().starts_with('#'))
            .find_map(|src| youtube_video(absolute(base.as_ref(), &src)?.as_str()));
        let Some(id) = id else { continue };
        // Repeats, such as a lazy embed's copy in `<noscript>`, go.
        if stills.contains(&id) {
            el.remove_from_parent();
            continue;
        }
        // A link can't go inside another: in one, the still is only the image.
        let linked = el.ancestors_it(None).any(|a| a.has_name("a"));
        el.replace_with_html(if linked {
            still_image(&id, short)
        } else {
            video_still(&id, short)
        });
        stills.insert(id);
    }
    link_embeds(&doc, base.as_ref());
    // FreshRSS links each video's old Flash player, as 💾, beside its still.
    for link in doc.select("a[download]").nodes() {
        let href = link.attr("href").unwrap_or_default();
        if youtube_video(&href).is_some_and(|id| stills.contains(&id)) {
            let parent = link.parent();
            link.remove_from_parent();
            if let Some(p) = parent.filter(|p| {
                p.has_name("p") && p.element_children().is_empty() && p.text().trim().is_empty()
            }) {
                p.remove_from_parent();
            }
        }
    }
    for media in doc.select("audio, video").nodes() {
        media.remove_attr("autoplay");
        media.set_attr("preload", "none");
        media.set_attr("controls", "");
    }
    for el in doc.select("[src], [poster], [srcset]").nodes() {
        for name in ["src", "poster"] {
            if let Some(value) = el.attr(name).filter(|v| !v.trim().is_empty()) {
                match resource(&value, base.as_ref()) {
                    Some(url) => el.set_attr(name, &url),
                    None => el.remove_attr(name),
                }
            }
        }
        if let Some(srcset) = el.attr("srcset") {
            match ready_srcset(&srcset, base.as_ref()) {
                Some(srcset) => el.set_attr("srcset", &srcset),
                None => el.remove_attr("srcset"),
            }
        }
    }
    let head = doc.select("head").inner_html();
    let body = doc.select("body").inner_html();
    let lead = match youtube_video(base.as_ref().map_or("", Url::as_str)) {
        Some(id) if !stills.contains(&id) => format!("<p>{}</p>", video_still(&id, short)),
        _ => String::new(),
    };
    format!("{head}{lead}{body}")
}

/// `doc` without the [`FLIPS`] inside `math` and `svg`: what is text in them here
/// would be markup once the output is parsed again, and get past the other passes.
fn drop_flips(doc: &Document) {
    for el in doc.select("math, svg").nodes() {
        for inner in el.descendants() {
            if FLIPS.iter().any(|name| inner.has_name(name)) {
                inner.remove_from_parent();
            }
        }
    }
}

/// Each iframe left as a link to what it embeds: see [`ready_article`].
fn link_embeds(doc: &Document, base: Option<&Url>) {
    let mut embeds: HashSet<Url> = HashSet::new();
    for el in doc.select("iframe").nodes() {
        let url = ["src", "data-src"]
            .into_iter()
            .filter_map(|name| el.attr(name))
            .filter(|src| !src.trim().is_empty() && !src.trim().starts_with('#'))
            // A lazy one's `src` is often `about:blank`.
            .find_map(|src| {
                absolute(base, &src).filter(|url| matches!(url.scheme(), "http" | "https"))
            })
            .filter(|url| !embeds.contains(url))
            .filter(|_| !el.ancestors_it(None).any(|a| a.has_name("a")));
        let Some(url) = url else {
            el.remove_from_parent();
            continue;
        };
        let title = el.attr("title").unwrap_or_default();
        let name = if title.trim().is_empty() {
            url.as_str()
        } else {
            title.trim()
        };
        el.replace_with_html(format!(
            "<a href=\"{}\" target=\"_blank\">{}</a>",
            escape(url.as_str()),
            escape(name)
        ));
        embeds.insert(url);
    }
}

/// Each `href` readied: see [`ready_article`].
fn ready_links(doc: &Document, base: Option<&Url>) {
    // By its local name, so SVG's `xlink:href` too.
    let refs: Vec<_> = doc
        .root()
        .descendants_it()
        .filter(|el| el.attr("href").is_some())
        .collect();
    for el in refs {
        // The first, as the renderer reads it. Any other, such as an `xlink:href`
        // beside an `href`, goes with it.
        let href = el.attr("href").unwrap_or_default();
        let href = href.trim();
        el.remove_attr("href");
        let is_link = el.has_name("a") || el.has_name("area");
        if href.starts_with('#') {
            el.set_attr("href", href);
            if is_link {
                el.remove_attr("target");
            }
        } else if is_link {
            if let Some(url) = absolute(base, href).filter(leaves_frame) {
                el.set_attr("href", url.as_str());
                el.set_attr("target", "_blank");
            }
        } else if let Some(url) = resource(href, base) {
            // What SVG's `image` and `use` load.
            el.set_attr("href", &url);
        }
    }
}

/// `doc` without its [`ACTIVE`] elements and event handlers. An `object` is unwrapped,
/// its fallback kept.
fn drop_active(doc: &Document) {
    doc.select(&ACTIVE.join(", ")).remove();
    doc.root().strip_elements(&["object"]);
    for el in doc.root().descendants() {
        let handlers: Vec<String> = el
            .attrs()
            .iter()
            .map(|a| a.name.local.to_string())
            .filter(|name| {
                name.get(..2)
                    .is_some_and(|on| on.eq_ignore_ascii_case("on"))
            })
            .collect();
        if !handlers.is_empty() {
            el.remove_attrs(&handlers.iter().map(String::as_str).collect::<Vec<_>>());
        }
    }
}

/// Elements that run code, load a plugin, or change how the frame loads or resolves:
/// `meta` can refresh it to another page, `base` and `link` change what it resolves and
/// loads, and SVG's animations can set an `href` the reader never sees. A `template`'s
/// contents are out of the reader's reach, and never shown. Styles stay, in `style`
/// elements and attributes.
const ACTIVE: [&str; 11] = [
    "animate",
    "animateMotion",
    "animateTransform",
    "base",
    "embed",
    "link",
    "meta",
    "param",
    "script",
    "set",
    "template",
];

/// Elements that can parse differently once written out, when inside `math` or `svg`:
/// those whose contents are raw text in HTML, which is markup there, and the two that
/// keep math inside the text elements of `math`. An article with them could hide an
/// element from the reader's passes in text that becomes that element in the frame.
const FLIPS: [&str; 11] = [
    "iframe",
    "malignmark",
    "mglyph",
    "noembed",
    "noframes",
    "noscript",
    "plaintext",
    "script",
    "style",
    "textarea",
    "xmp",
];

/// Elements whose text is not linked: links already, text not shown as HTML, and
/// elements a link can't go in or would mean something else in.
const UNLINKED: [&str; 16] = [
    "a",
    "button",
    "iframe",
    "math",
    "noembed",
    "noframes",
    "noscript",
    "plaintext",
    "script",
    "select",
    "style",
    "svg",
    "template",
    "textarea",
    "title",
    "xmp",
];

/// `text` as HTML with its http and https addresses linked, or `None` if it has none.
/// An address starts a word and runs as [`url_end`] says, less what ends the sentence
/// around it: see [`trim_url`].
fn linked(text: &str) -> Option<String> {
    let mut out = String::new();
    // `text` before `done` is in `out`.
    let mut done = 0;
    let mut from = 0;
    while let Some(found) = text[from..].find("://") {
        let sep = from + found;
        from = sep + 3;
        let scheme = |len: usize, name: &str| {
            let start = sep.checked_sub(len).filter(|&s| s >= done)?;
            text.get(start..sep)?
                .eq_ignore_ascii_case(name)
                .then_some(start)
        };
        let Some(start) = scheme(5, "https").or_else(|| scheme(4, "http")) else {
            continue;
        };
        // Not `xhttps://`: a scheme starts a word, or follows CJK text.
        if text[..start]
            .bytes()
            .next_back()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        {
            continue;
        }
        let end = url_end(&text[from..]).map_or(text.len(), |i| from + i);
        let url = trim_url(&text[start..end]);
        if url.len() <= sep + 3 - start {
            continue;
        }
        out.push_str(&escape(&text[done..start]));
        let shown = escape(url);
        let _ = write!(out, "<a href=\"{shown}\">{shown}</a>");
        done = start + url.len();
        from = done;
    }
    if done == 0 && out.is_empty() {
        return None;
    }
    out.push_str(&escape(&text[done..]));
    Some(out)
}

/// Where a written-out address ends in `rest`, the text after its `://`: at whitespace,
/// a quote, an angle bracket, an invisible break or CJK text, which runs on with no
/// space after one. Browsers copy a CJK path percent-encoded, so few addresses are
/// written with one, and one with a CJK host is not found.
fn url_end(rest: &str) -> Option<usize> {
    rest.find(|c: char| {
        c.is_whitespace()
            || matches!(c, '<' | '>' | '"' | '“' | '”' | '‘' | '«' | '»')
            || matches!(c, '\u{AD}' | '\u{200B}' | '\u{2060}' | '\u{FEFF}')
            || matches!(c, '\u{2E80}'..='\u{9FFF}' | '\u{AC00}'..='\u{D7AF}' | '\u{F900}'..='\u{FAFF}' | '\u{FF00}'..='\u{FFEF}')
    })
}

/// `url` less the punctuation after it, which ends the sentence: a closing bracket only
/// when the address has no opening one for it, as Wikipedia's `…_(film)` does.
fn trim_url(url: &str) -> &str {
    // Counted once: counting on each trim is quadratic in a run of `)`.
    let count = |c: char| url.matches(c).count();
    let mut unpaired = [('(', ')'), ('[', ']'), ('{', '}')]
        .map(|(o, c)| (c, count(c).cast_signed() - count(o).cast_signed()));
    let mut end = url.len();
    while let Some(c) = url[..end].chars().next_back() {
        let trimmed = match c {
            '.' | ',' | ';' | ':' | '!' | '?' | '\'' | '’' | '*' | '…' | '`' => true,
            ')' | ']' | '}' => unpaired
                .iter_mut()
                .find(|(close, _)| *close == c)
                .is_some_and(|(_, n)| {
                    let trim = *n > 0;
                    *n -= 1;
                    trim
                }),
            _ => false,
        };
        if !trimmed {
            break;
        }
        end -= c.len_utf8();
    }
    &url[..end]
}

/// A YouTube video's still, linked to the video.
fn video_still(id: &str, short: bool) -> String {
    format!(
        "<a href=\"https://www.youtube.com/watch?v={id}\" target=\"_blank\">{}</a>",
        still_image(id, short)
    )
}

/// A YouTube video's still. `hqdefault.jpg` exists for every video; it is 4:3 with
/// black bars, which `frame.css` crops off, except for a Short's, which has none.
fn still_image(id: &str, short: bool) -> String {
    let class = if short {
        "skvaller-short"
    } else {
        "skvaller-video"
    };
    format!("<img class=\"{class}\" src=\"https://i.ytimg.com/vi/{id}/hqdefault.jpg\" alt=\"\">")
}

fn leaves_frame(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https" | "mailto")
}

/// `url` made absolute against `base`, and asked for over https; or `None` unless it is
/// a web URL or a `data:` image. One that can't be made absolute would resolve against
/// the app.
fn resource(url: &str, base: Option<&Url>) -> Option<String> {
    let mut url = absolute(base, url)?;
    match url.scheme() {
        // Infallible between two special schemes.
        "http" => _ = url.set_scheme("https"),
        "https" => {}
        "data"
            if url
                .path()
                .get(..6)
                .is_some_and(|t| t.eq_ignore_ascii_case("image/")) => {}
        _ => return None,
    }
    Some(url.into())
}

/// A `srcset` with each candidate's URL readied by [`resource`]. As in HTML, a URL runs
/// to the next whitespace, less trailing commas, and its descriptors to the next comma.
/// `None` when no candidate is left.
fn ready_srcset(srcset: &str, base: Option<&Url>) -> Option<String> {
    let mut candidates = Vec::new();
    let mut rest = srcset;
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_whitespace() || c == ',');
        if rest.is_empty() {
            break;
        }
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let (raw, after) = rest.split_at(end);
        let url = raw.trim_end_matches(',');
        let mut descriptors = "";
        rest = after;
        if url.len() == raw.len() {
            let stop = after.find(',').unwrap_or(after.len());
            descriptors = after[..stop].trim();
            rest = &after[stop..];
        }
        let Some(url) = resource(url, base) else {
            continue;
        };
        candidates.push(if descriptors.is_empty() {
            url
        } else {
            format!("{url} {descriptors}")
        });
    }
    (!candidates.is_empty()).then(|| candidates.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://example.com/posts/1";

    #[test]
    fn markup_hidden_in_math_text_does_not_become_an_iframe_once_parsed_again() {
        for article in [
            "<math><mtext><table><mglyph><style><iframe src=https://evil.example/x>",
            "<form><math><mtext></form><form><mglyph><style></math><iframe src=https://evil.example/x>",
            "<svg></p><style><iframe src=https://evil.example/x>",
            "<math><mtext><table><mglyph><xmp><form action=https://evil.example/>",
        ] {
            let html = ready_article(article, BASE);
            let again = parse(&format!("<article>{html}</article>")).unwrap();
            assert_eq!(
                again.select("iframe, form").length(),
                0,
                "{article}: {html}"
            );
        }
    }

    #[test]
    fn links_become_absolute_and_in_article_links_stay() {
        let html = ready_article(
            r##"<p><a href="/about">a</a> <a href="#fn1" target="_top">1</a> <a href="http://[bad">b</a></p>"##,
            BASE,
        );
        assert!(
            html.contains(r#"href="https://example.com/about" target="_blank""#),
            "{html}"
        );
        assert!(html.contains(r##"<a href="#fn1">1</a>"##), "{html}");
        assert!(html.contains("<a>b</a>"), "{html}");
    }

    #[test]
    fn written_out_addresses_become_links_outside_links() {
        let html = ready_article(
            r#"<p>See https://example.com/a?b=1&amp;c=2. Or <a href="/x">http://in.link</a> &lt;b&gt;</p>"#,
            BASE,
        );
        assert_eq!(
            html,
            r#"<p>See <a href="https://example.com/a?b=1&amp;c=2" target="_blank">https://example.com/a?b=1&amp;c=2</a>. Or <a href="https://example.com/x" target="_blank">http://in.link</a> &lt;b&gt;</p>"#
        );
        for raw in [
            "<xmp>https://x.com</xmp>",
            "<svg><text>https://x.com</text></svg>",
            "<button>https://x.com</button>",
            "<select><option>https://x.com</option></select>",
        ] {
            let html = ready_article(raw, BASE);
            assert!(!html.contains("<a"), "{html}");
        }
        let html = ready_article("<pre>  https://x.com/a\n\tb</pre>", BASE);
        assert_eq!(
            html,
            "<pre>  <a href=\"https://x.com/a\" target=\"_blank\">https://x.com/a</a>\n\tb</pre>"
        );
    }

    #[test]
    fn an_address_ends_where_its_sentence_goes_on() {
        let link = |url: &str| format!(r#"<a href="{url}">{url}</a>"#);
        let wiki = "https://en.wikipedia.org/wiki/Rust_(programming_language)";
        for (text, html) in [
            (format!("See {wiki} now"), format!("See {} now", link(wiki))),
            (format!("(see {wiki})."), format!("(see {}).", link(wiki))),
            (
                "[https://x.com/a]".into(),
                format!("[{}]", link("https://x.com/a")),
            ),
            (
                "“https://x.com/b”".into(),
                format!("“{}”", link("https://x.com/b")),
            ),
            (
                "*https://x.com/c*!".into(),
                format!("*{}*!", link("https://x.com/c")),
            ),
            (
                "（https://x.jp/a）。詳細はhttps://x.jp/をご覧ください".into(),
                format!(
                    "（{}）。詳細は{}をご覧ください",
                    link("https://x.jp/a"),
                    link("https://x.jp/")
                ),
            ),
            ("HTTPS://X.COM/A".into(), link("HTTPS://X.COM/A")),
            ("https://a.com/{x}".into(), link("https://a.com/{x}")),
            (
                "see https://a.com/x…".into(),
                format!("see {}…", link("https://a.com/x")),
            ),
            (
                "`https://a.com/x`".into(),
                format!("`{}`", link("https://a.com/x")),
            ),
            (
                "https://a.com/don’t’".into(),
                format!("{}’", link("https://a.com/don’t")),
            ),
            (
                "https://a.com\u{200B}https://b.com".into(),
                format!("{}\u{200B}{}", link("https://a.com"), link("https://b.com")),
            ),
            (
                "https://a.com에서".into(),
                format!("{}에서", link("https://a.com")),
            ),
            (
                "xhttps://a.com (https://b.com)".into(),
                format!("xhttps://a.com ({})", link("https://b.com")),
            ),
            (
                "a&b https://x.com/?q=\"c\"".into(),
                format!("a&amp;b {}&quot;c&quot;", link("https://x.com/?q=")),
            ),
        ] {
            assert_eq!(linked(&text).as_deref(), Some(html.as_str()), "{text}");
        }
        for text in [
            "https://",
            "https://...",
            "see http",
            "ftp://x.com",
            "no links",
        ] {
            assert_eq!(linked(text), None, "{text}");
        }
        // Trimming a run of brackets takes linear time.
        let long = format!("https://a.com/{}", ")".repeat(200_000));
        assert_eq!(linked(&long).map(|h| h.len()), Some(200_000 + 2 * 14 + 15));
    }

    #[test]
    fn freshrss_s_flash_link_beside_a_still_goes_and_a_short_s_still_is_not_cropped() {
        let html = ready_article(
            r#"<figure><p><img src="https://i4.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"></p><p class="enclosure-content"><a download href="https://www.youtube.com/v/dQw4w9WgXcQ?version=3">💾</a></p><figcaption>Text</figcaption></figure>"#,
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
        );
        assert_eq!(
            html,
            format!(
                "<figure><p>{}</p><figcaption>Text</figcaption></figure>",
                STILL.replace("skvaller-video", "skvaller-short")
            )
        );
    }

    const STILL: &str = r#"<a href="https://www.youtube.com/watch?v=dQw4w9WgXcQ" target="_blank"><img class="skvaller-video" src="https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg" alt=""></a>"#;

    #[test]
    fn a_youtube_embed_becomes_its_linked_still_and_other_iframes_links() {
        let html = ready_article(
            r#"<p>a</p><iframe src="//www.youtube-nocookie.com/embed/dQw4w9WgXcQ"></iframe><iframe src="https://other.example/x"></iframe><p>b</p>"#,
            BASE,
        );
        assert_eq!(
            html,
            format!(
                r#"<p>a</p>{STILL}<a href="https://other.example/x" target="_blank">https://other.example/x</a><p>b</p>"#
            )
        );
        let html = ready_article(
            r#"<iframe src="about:blank" data-src="/map?a=1&amp;b=2" title="A &lt;map&gt;"></iframe><noscript><iframe src="/map?a=1&amp;b=2"></iframe></noscript><iframe src="javascript:x"></iframe><a href="/y"><iframe src="/z"></iframe></a>"#,
            BASE,
        );
        assert_eq!(
            html,
            r#"<a href="https://example.com/map?a=1&amp;b=2" target="_blank">A &lt;map&gt;</a><noscript></noscript><a href="https://example.com/y" target="_blank"></a>"#
        );
    }

    #[test]
    fn a_lazy_youtube_embed_becomes_one_still() {
        let html = ready_article(
            r#"<iframe src="https://example.com/posts/1" data-src="https://www.youtube.com/embed/dQw4w9WgXcQ?feature=oembed"></iframe><noscript><iframe src="https://www.youtube.com/embed/dQw4w9WgXcQ"></iframe></noscript>"#,
            BASE,
        );
        assert_eq!(html, format!("{STILL}<noscript></noscript>"));
    }

    #[test]
    fn a_youtube_still_in_a_link_is_cropped_but_not_linked_again() {
        let html = ready_article(
            r#"<a href="/x"><iframe src="https://www.youtube.com/embed/dQw4w9WgXcQ"></iframe></a>"#,
            BASE,
        );
        assert!(
            html.contains(r#"target="_blank"><img class="skvaller-video" src="https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg" alt=""></a>"#),
            "{html}"
        );
        assert_eq!(html.matches("<a ").count(), 1, "{html}");
    }

    #[test]
    fn a_feeds_still_of_the_articles_video_is_its_one_still() {
        let video = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
        let html = ready_article(
            r#"<figure><p><img src="https://i3.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg"></p></figure>"#,
            video,
        );
        assert_eq!(html, format!("<figure><p>{STILL}</p></figure>"));
        // Linked, by FreshRSS or anyone: cropped, and still the only one.
        let html = ready_article(
            r#"<a href="/x"><img src="https://img.youtube.com/vi/dQw4w9WgXcQ/0.jpg"></a>"#,
            video,
        );
        assert_eq!(html.matches("skvaller-video").count(), 1, "{html}");
        // An empty placeholder isn't the page's video.
        let html = ready_article(r#"<img src="" alt="x">"#, video);
        assert!(html.ends_with(r#"<img src="" alt="x">"#), "{html}");
        assert_eq!(html.matches("skvaller-video").count(), 1, "{html}");
    }

    #[test]
    fn another_videos_still_leaves_the_articles_own_lead() {
        let html = ready_article(
            r#"<iframe src="https://www.youtube.com/embed/AAAAAAAAAAA"></iframe>"#,
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
        );
        assert!(html.starts_with(&format!("<p>{STILL}</p>")), "{html}");
        assert!(html.contains("vi/AAAAAAAAAAA/"), "{html}");
    }

    #[test]
    fn a_youtube_videos_article_starts_with_its_still() {
        let html = ready_article("<p>About</p>", "https://youtu.be/dQw4w9WgXcQ");
        assert!(html.starts_with(&format!("<p>{STILL}</p>")), "{html}");
        assert!(html.ends_with("<p>About</p>"), "{html}");
        assert!(!ready_article("<p>About</p>", BASE).contains("skvaller-video"));
    }

    #[test]
    fn resources_become_absolute_https() {
        let html = ready_article(
            r#"<img src="img/a.png" srcset="http://cdn.example/a.png 2x, b.png 640w"><video src="http://v.example/v.mp4" autoplay></video>"#,
            BASE,
        );
        assert!(
            html.contains(r#"src="https://example.com/posts/img/a.png""#),
            "{html}"
        );
        assert!(
            html.contains(
                r#"srcset="https://cdn.example/a.png 2x, https://example.com/posts/b.png 640w""#
            ),
            "{html}"
        );
        assert!(html.contains(r#"src="https://v.example/v.mp4""#), "{html}");
        assert!(!html.contains("autoplay"), "{html}");
        assert!(html.contains(r#"preload="none""#), "{html}");
    }

    #[test]
    fn a_srcset_url_with_commas_keeps_them() {
        assert_eq!(
            ready_srcset("a.png?w=1,2 1x,b.png", Url::parse(BASE).ok().as_ref()).unwrap(),
            "https://example.com/posts/a.png?w=1,2 1x, https://example.com/posts/b.png"
        );
    }

    #[test]
    fn links_with_other_schemes_lose_their_href() {
        let html = ready_article(
            r#"<a href="javascript:alert(1)">j</a><a href="file:///etc/passwd">f</a><a href="mailto:a@example.com">m</a>"#,
            BASE,
        );
        assert!(html.contains("<a>j</a><a>f</a>"), "{html}");
        assert!(html.contains(r#"href="mailto:a@example.com""#), "{html}");
    }

    #[test]
    fn forms_are_unwrapped_and_their_fields_kept() {
        let html = ready_article(
            r#"<form action="https://evil.example/"><input name="q"><button>Go</button></form>"#,
            BASE,
        );
        assert_eq!(html, r#"<input name="q"><button>Go</button>"#);
    }

    #[test]
    fn scripts_handlers_and_page_controls_go() {
        // Each article, and what must not be left of it once readied and parsed again.
        let cases: &[(&str, &[&str])] = &[
            ("<script>alert(1)</script><p>x</p>", &["script", "alert"]),
            (
                "<p>x<script src=https://evil.example/a.js></script></p>",
                &["script", "evil"],
            ),
            ("<svg><script>alert(1)</script></svg>", &["script", "alert"]),
            (
                r#"<meta http-equiv="refresh" content="0;url=https://evil.example/">"#,
                &["meta", "evil"],
            ),
            (
                r#"<base href="https://evil.example/"><a href="a">a</a>"#,
                &["base", "evil"],
            ),
            (
                r#"<link rel="stylesheet" href="https://evil.example/a.css">"#,
                &["link", "evil"],
            ),
            (
                r#"<object data="file:///etc/passwd"><p>fallback</p></object>"#,
                &["object", "passwd"],
            ),
            (
                r#"<embed src="https://evil.example/a.swf">"#,
                &["embed", "evil"],
            ),
            (
                r#"<p onclick="alert(1)" OnMouseOver="alert(2)">x</p>"#,
                &["onclick", "onmouseover", "alert"],
            ),
            (
                r#"<svg onload="alert(1)"><circle onclick="alert(2)"/></svg>"#,
                &["onload", "onclick", "alert"],
            ),
            (
                r#"<body onload="alert(1)"><p>x</p></body>"#,
                &["onload", "alert"],
            ),
        ];
        for (html, gone) in cases {
            let readied = ready_article(html, BASE);
            let again = parse(&readied).unwrap().html().to_lowercase();
            for word in *gone {
                assert!(!again.contains(word), "{word} left in {again} from {html}");
            }
        }
    }

    #[test]
    fn script_and_other_scheme_urls_go_however_written() {
        let urls = [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            " \tjavascript:alert(1)",
            "java\tscript:alert(1)",
            "&#106;avascript:alert(1)",
            "&#x6A;ava&#x09;script:alert(1)",
            "vbscript:msgbox(1)",
            "data:text/html,<script>alert(1)</script>",
            "DATA:text/html;base64,PHNjcmlwdD4=",
            "file:///etc/passwd",
        ];
        for url in urls {
            let html = format!(
                r#"<img src="{url}" srcset="{url} 2x"><video poster="{url}" src="{url}"></video><a href="{url}">a</a><svg><a xlink:href="{url}"><text>t</text></a><image href="{url}"/><use xlink:href="{url}"/></svg>"#
            );
            let readied = ready_article(&html, BASE);
            let again = parse(&readied).unwrap().html().to_lowercase();
            // A srcset URL ends at whitespace: `java\tscript:` is the URL `java`.
            for word in ["javascript:", "vbscript:", "data:", "file:"] {
                assert!(!again.contains(word), "{word} left in {again} from {url}");
            }
            for name in [" src=", "poster=", "href="] {
                assert!(!again.contains(name), "{name} left in {again} from {url}");
            }
        }
    }

    #[test]
    fn data_images_web_urls_and_fragments_stay() {
        let html = ready_article(
            r##"<img src="data:image/png;base64,iVBORw0KGgo=" srcset="javascript:x 1x, a.png 2x"><svg><use href="#icon"/><image xlink:href="i.png"/></svg>"##,
            BASE,
        );
        assert!(
            html.contains(r#"src="data:image/png;base64,iVBORw0KGgo=""#),
            "{html}"
        );
        assert!(
            html.contains(r#"srcset="https://example.com/posts/a.png 2x""#),
            "{html}"
        );
        assert!(html.contains(r##"href="#icon""##), "{html}");
        assert!(
            html.contains(r#"href="https://example.com/posts/i.png""#),
            "{html}"
        );
        assert!(!html.contains("target"), "{html}");
    }

    #[test]
    fn templates_and_svg_animations_go() {
        let html = ready_article(
            r#"<template><script>alert(1)</script><a href="javascript:alert(1)">t</a></template><svg><set attributeName="href" to="javascript:alert(1)"/><animate attributeName="xlink:href" values="javascript:alert(1)"/><animateMotion/><animateTransform/><circle/></svg>"#,
            BASE,
        );
        assert_eq!(html, "<svg><circle></circle></svg>");
    }

    #[test]
    fn an_svg_link_keeps_one_readied_href() {
        for html in [
            r#"<svg><a href="https://ok.example/" xlink:href="javascript:alert(1)"><text>t</text></a></svg>"#,
            r#"<svg><a xlink:href="https://ok.example/" href="javascript:alert(1)"><text>t</text></a></svg>"#,
        ] {
            let readied = ready_article(html, BASE);
            assert_eq!(
                readied,
                r#"<svg><a href="https://ok.example/" target="_blank"><text>t</text></a></svg>"#
            );
        }
    }

    #[test]
    fn svg_resources_are_readied_as_resources() {
        let html = ready_article(
            r#"<svg><image href="data:image/png;base64,iVBORw0KGgo="/><image xlink:href="http://cdn.example/a.png"/><use href="mailto:a@example.com"/></svg>"#,
            BASE,
        );
        assert_eq!(
            html,
            r#"<svg><image href="data:image/png;base64,iVBORw0KGgo="></image><image href="https://cdn.example/a.png"></image><use></use></svg>"#
        );
    }

    #[test]
    fn an_objects_fallback_stays() {
        let html = ready_article(
            r#"<object data="a.swf"><param name="movie" value="a.swf"><embed src="a.swf"><img src="a.png" alt="chart"></object>"#,
            BASE,
        );
        assert_eq!(
            html,
            r#"<img src="https://example.com/posts/a.png" alt="chart">"#
        );
    }

    #[test]
    fn resources_that_cant_be_made_absolute_go() {
        let html = ready_article(r#"<img src="a.png" alt="a">"#, "");
        assert_eq!(html, r#"<img alt="a">"#);
    }

    #[test]
    fn styles_in_the_head_are_kept() {
        let html = ready_article("<style>p{color:red}</style><p>x</p>", BASE);
        assert_eq!(html, "<style>p{color:red}</style><p>x</p>");
    }
}
