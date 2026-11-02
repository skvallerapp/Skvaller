//! Plain-text previews of untrusted article HTML for the article list.

/// Elements whose contents are never text.
const SKIPPED: [&str; 6] = ["script", "style", "template", "noscript", "svg", "head"];
/// Elements that sit inside a line of text. Every other tag separates words.
const INLINE: [&str; 15] = [
    "a", "abbr", "b", "cite", "code", "em", "i", "mark", "q", "s", "small", "span", "strong",
    "sub", "sup",
];

/// Text of `html` with tags dropped, common entities decoded and whitespace collapsed,
/// cut to at most `max` characters. Good enough for a two-line preview; not a parser.
pub(crate) fn plain_text(html: &str, max: usize) -> String {
    let mut out = String::new();
    let mut count = 0;
    let mut space = false;
    let mut rest = html;

    let mut push = |c: char, out: &mut String, space: &mut bool| {
        if c.is_whitespace() {
            *space = true;
            return true;
        }
        let gap = *space && !out.is_empty();
        if count + usize::from(gap) + 1 > max {
            return false;
        }
        if gap {
            out.push(' ');
            count += 1;
        }
        *space = false;
        out.push(c);
        count += 1;
        true
    };

    while let Some(c) = rest.chars().next() {
        if c == '<' {
            let name = tag_name(&rest[1..]);
            let end = rest.find('>').map_or(rest.len(), |i| i + 1);
            // Only `<svg>` hides what follows, not `</svg>` or `<svg/>`.
            let opens = !rest[1..].starts_with('/') && !rest[..end].ends_with("/>");
            rest = &rest[end..];
            if opens && SKIPPED.contains(&name.as_str()) && !rest.is_empty() {
                rest = skip_past_close(rest, &name);
            }
            if !INLINE.contains(&name.as_str()) {
                space = true;
            }
            continue;
        }
        let (decoded, len) = if c == '&' {
            entity(rest)
        } else {
            (c, c.len_utf8())
        };
        rest = &rest[len..];
        if !push(decoded, &mut out, &mut space) {
            out.push('…');
            break;
        }
    }
    out
}

/// Lowercase name of the tag that starts right after `<`, without a leading `/`.
fn tag_name(s: &str) -> String {
    s.trim_start_matches('/')
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// The rest of `s` after the closing tag `</name>`, or nothing if it never closes.
/// Compares in place: lowercasing a copy per skipped tag made pages of many `<svg>`s
/// quadratic.
fn skip_past_close<'a>(s: &'a str, name: &str) -> &'a str {
    let mut from = 0;
    while let Some(i) = s[from..].find("</") {
        let start = from + i;
        let tag = start + 2..start + 2 + name.len();
        if s.as_bytes()
            .get(tag)
            .is_some_and(|b| b.eq_ignore_ascii_case(name.as_bytes()))
        {
            return s[start..].find('>').map_or("", |j| &s[start + j + 1..]);
        }
        from = start + 2;
    }
    ""
}

/// Decodes the entity at the start of `s` (which starts with `&`). Returns the
/// character and how many bytes it used; an unknown entity is a literal `&`.
fn entity(s: &str) -> (char, usize) {
    let Some(end) = s[1..].find(';').filter(|&i| i <= 10) else {
        return ('&', 1);
    };
    let name = &s[1..=end];
    let c = match name {
        "amp" => Some('&'),
        "lt" => Some('<'),
        "gt" => Some('>'),
        "quot" => Some('"'),
        "apos" => Some('\''),
        "nbsp" => Some(' '),
        "hellip" => Some('…'),
        "mdash" => Some('—'),
        "ndash" => Some('–'),
        "lsquo" => Some('‘'),
        "rsquo" => Some('’'),
        "ldquo" => Some('“'),
        "rdquo" => Some('”'),
        _ => name.strip_prefix('#').and_then(|num| {
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok(),
                None => num.parse().ok(),
            };
            code.and_then(char::from_u32)
        }),
    };
    match c {
        Some(c) => (c, end + 2),
        None => ('&', 1),
    }
}

#[cfg(test)]
mod tests {
    use super::plain_text;

    #[test]
    fn strips_tags_and_collapses_whitespace() {
        let html = "<p>Hello <b>big</b>\n  world</p><p>Next</p><br>line";
        assert_eq!(plain_text(html, 100), "Hello big world Next line");
    }

    #[test]
    fn drops_scripts_and_styles() {
        let html = "<style>p{}</style><SCRIPT>alert(1)</SCRIPT>Text<script>never closed";
        assert_eq!(plain_text(html, 100), "Text");
    }

    #[test]
    fn decodes_entities() {
        assert_eq!(
            plain_text("A &amp; B &#8217; &#x2014; &bogus; &", 100),
            "A & B ’ — &bogus; &"
        );
    }

    #[test]
    fn truncates_by_characters() {
        assert_eq!(plain_text("<p>åäö åäö</p>", 5), "åäö å…");
        assert_eq!(plain_text("åäö", 3), "åäö");
        // The space before a word counts, so a word that starts one short of the limit
        // is cut too.
        assert_eq!(plain_text("aaa bbbb", 4), "aaa…");
    }

    #[test]
    fn a_stray_or_self_closing_skipped_tag_hides_nothing() {
        assert_eq!(
            plain_text("<p>a</p></svg><p>Real text</p>", 100),
            "a Real text"
        );
        assert_eq!(plain_text("<svg/><p>Real text</p>", 100), "Real text");
    }
}
