//! Untrusted article HTML, readied for showing in an isolated reader document.
//!
//! The article still renders without scripts; this only makes what it loads and links
//! to behave: resources and links are made absolute against the article's own URL
//! (the reader document's base is the app's, not the article's), resources are asked
//! for over https, and media waits to be asked for.

use reqwest::Url;

use crate::html::parse;

/// `html` readied for the reader, with relative URLs resolved against `base`.
///
/// - Links to other pages become absolute and get `target="_blank"`, so they leave the
///   reader's frame for the browser; one that can't be made absolute loses its `href`,
///   so it can't resolve against the app. Links within the article (`#…`) stay in it.
/// - `src`, `poster` and `srcset` URLs become absolute, and `http:` becomes `https:`:
///   most hosts that once served http now serve https too.
/// - Audio and video don't autoplay or preload, and show their controls: without
///   scripts, controls are the only way to play.
///
/// An article nested too deeply to parse safely gives an empty string.
#[must_use]
pub fn ready_article(html: &str, base: &str) -> String {
    let Some(doc) = parse(html) else {
        return String::new();
    };
    let base = Url::parse(base).ok();
    for link in doc.select("a[href], area[href]").nodes() {
        let href = link.attr("href").unwrap_or_default();
        let href = href.trim();
        if href.starts_with('#') {
            link.remove_attr("target");
            continue;
        }
        match absolute(href, base.as_ref()) {
            Some(url) => {
                link.set_attr("href", url.as_str());
                link.set_attr("target", "_blank");
            }
            None => link.remove_attr("href"),
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
    format!("{head}{body}")
}

fn absolute(url: &str, base: Option<&Url>) -> Option<Url> {
    match base {
        Some(base) => base.join(url).ok(),
        None => Url::parse(url).ok(),
    }
}

/// `url` made absolute against `base`, and asked for over https. Left as it is when it
/// can't be made absolute: there's no better guess.
fn resource(url: &str, base: Option<&Url>) -> String {
    let url = url.trim();
    let absolute = absolute(url, base).map_or_else(|| url.to_owned(), String::from);
    match absolute.get(..7) {
        Some(scheme) if scheme.eq_ignore_ascii_case("http://") => {
            format!("https://{}", &absolute[7..])
        }
        _ => absolute,
    }
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
    fn styles_in_the_head_are_kept() {
        let html = ready_article("<style>p{color:red}</style><p>x</p>", BASE);
        assert_eq!(html, "<style>p{color:red}</style><p>x</p>");
    }
}
