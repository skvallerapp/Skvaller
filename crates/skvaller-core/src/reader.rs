//! Untrusted article HTML, readied for showing in an isolated reader document.
//!
//! The article still renders without scripts; this only makes what it loads and links
//! to behave: resources and links are made absolute against the article's own URL
//! (the reader document's base is the app's, not the article's), resources are asked
//! for over https, media waits to be asked for, and forms can't be sent.

use std::fmt::Write;

use reqwest::Url;

use dom_query::Document;

use crate::html::{escape, parse};
use crate::net::absolute;
use crate::thumbnail::youtube_video;

/// `html` readied for the reader, with relative URLs resolved against `base`.
///
/// - Web addresses written out in the text become links, outside links already.
/// - Links to other pages become absolute and get `target="_blank"`, so they leave the
///   reader's frame for the browser; one that can't be made absolute loses its `href`,
///   so it can't resolve against the app. Links within the article (`#…`) stay in it.
///   Only http, https and mailto links are kept: no other scheme should leave the frame.
/// - `src`, `poster` and `srcset` URLs become absolute, and `http:` becomes `https:`:
///   most hosts serve https too.
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
    for link in doc.select("a[href], area[href]").nodes() {
        let href = link.attr("href").unwrap_or_default();
        let href = href.trim();
        if href.starts_with('#') {
            link.remove_attr("target");
            continue;
        }
        match absolute(base.as_ref(), href).filter(leaves_frame) {
            Some(url) => {
                link.set_attr("href", url.as_str());
                link.set_attr("target", "_blank");
            }
            None => link.remove_attr("href"),
        }
    }
    doc.root().strip_elements(&["form"]);
    let mut stills: Vec<String> = Vec::new();
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
        stills.push(id);
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
                el.set_attr(name, &resource(&value, base.as_ref()));
            }
        }
        if let Some(srcset) = el.attr("srcset") {
            el.set_attr("srcset", &ready_srcset(&srcset, base.as_ref()));
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

/// Each iframe left as a link to what it embeds: see [`ready_article`].
fn link_embeds(doc: &Document, base: Option<&Url>) {
    let mut embeds: Vec<Url> = Vec::new();
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
        embeds.push(url);
    }
}

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

/// `url` made absolute against `base`, and asked for over https. Left as it is when it
/// can't be made absolute: there's no better guess.
fn resource(url: &str, base: Option<&Url>) -> String {
    let Some(mut url) = absolute(base, url) else {
        return url.trim().to_owned();
    };
    if url.scheme() == "http" {
        // Infallible between two special schemes.
        let _ = url.set_scheme("https");
    }
    url.into()
}

/// A `srcset` with each candidate's URL readied by [`resource`]. As in HTML, a URL runs
/// to the next whitespace, less trailing commas, and its descriptors to the next comma.
fn ready_srcset(srcset: &str, base: Option<&Url>) -> String {
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
        let url = resource(url, base);
        candidates.push(if descriptors.is_empty() {
            url
        } else {
            format!("{url} {descriptors}")
        });
    }
    candidates.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "https://example.com/posts/1";

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
            ready_srcset("a.png?w=1,2 1x,b.png", Url::parse(BASE).ok().as_ref()),
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
    fn styles_in_the_head_are_kept() {
        let html = ready_article("<style>p{color:red}</style><p>x</p>", BASE);
        assert_eq!(html, "<style>p{color:red}</style><p>x</p>");
    }
}
