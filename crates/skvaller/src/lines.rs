//! Counts the lines a headline wraps to, so the article list knows its rows' heights
//! before drawing them.
//!
//! Rows are placed from computed heights, never by measuring the document, so the list
//! stays virtualized at 5,000+ rows. Laying out every headline with Parley, the engine
//! Blitz draws them with, takes about 30 µs each: 150 ms for a list of 5,000, again at
//! every width. Instead each character is shaped once, in the fonts Blitz has, and a
//! headline is cut once into pieces at the break opportunities of the line segmenter
//! Parley uses, each with its width. Wrapping those at a width is then arithmetic, so a
//! new width or a reload costs little.
//!
//! Shaping text whole can make it wider than its characters: some pairs kern apart, and
//! some fonts swap in wider glyphs in context, as Geist does for `->` and Inter for the
//! `x` of `1920x1080` and the `*` of `2 * 3`. So each pair of characters that shapes
//! wider than its two adds the difference, a piece with the symbols those swaps start
//! from is shaped whole as well, and so are a digit's `x` and an operator between
//! spaces in their context. Where such symbols meet at a break opportunity, as in `->`,
//! a font may join them into one glyph that doesn't break, as Inter and Geist do: if
//! the layout keeps them together, the pieces either side are taken as one.
//!
//! Kerning that narrows text is left out, so a count runs about 0.5% wide: a row may
//! get a line too many. The list clamps each headline to its count, so one that is
//! still too wide, as in a zoomed window or a font that falls back in a way this
//! doesn't see, loses its last line rather than showing half of it.

use std::collections::HashMap;
use std::rc::Rc;

use icu_segmenter::LineSegmenter;
use icu_segmenter::options::LineBreakOptions;
use parley::setting::Tag;
use parley::{FontContext, FontFamily, FontVariation, FontWeight, LayoutContext, StyleProperty};

/// The headline's font size in CSS px, as `.title` in app.css.
pub const SIZE: f32 = 14.5;

/// The headline's weight, as `.title` in app.css. Read headlines are lighter (500) but
/// are counted at this one, so marking an article read never changes its row's height.
const WEIGHT: f32 = 600.0;

/// The optical size Blitz shapes the headline at: its size, as CSS
/// `font-optical-sizing: auto` has it. Fonts with the axis, such as Source Serif 4, are
/// wider at smaller optical sizes than at their default.
const OPTICAL_SIZE: [FontVariation; 1] = [FontVariation {
    tag: Tag::new(b"opsz"),
    value: SIZE,
}];

/// Symbols fonts swap wider glyphs in for, in context: arrows such as `->` and `<=`,
/// runs such as `**`. A piece with one is shaped whole.
const CONTEXTUAL: [char; 14] = [
    '-', '<', '>', '=', '*', '+', '|', '~', '/', '\\', '#', '&', '^', '_',
];

/// The most headlines, pieces shaped whole, and runs and operators probed kept: about
/// the rows of a few long lists. Past it, the headlines kept are the list's, and the
/// others start over.
const KEPT: usize = 20_000;

/// A headline cut where it may wrap, in one font stack: what [`Headline::lines`] wraps.
#[derive(Debug, PartialEq)]
pub struct Headline(Box<[Piece]>);

/// A piece of a headline that doesn't wrap, as a word with its punctuation.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Piece {
    width: f64,
    /// What comes before it when it follows on the same line: spaces, or what it adds
    /// to the piece it touches.
    before: f64,
    /// Whether it touches the piece before it, so that `before` is in its glyphs and
    /// goes with it to a new line.
    touches: bool,
    /// The lines line and paragraph separators end before it, which it starts after.
    breaks: u32,
}

impl Headline {
    /// The lines, at most `max`, the headline wraps to at `width` CSS px.
    ///
    /// A line is taken as full a pixel early, as the document's layout may round
    /// differently: a row may get a line too many, never too few.
    pub fn lines(&self, width: f64, max: u32) -> u32 {
        let room = width - 1.0;
        let mut lines = 1;
        // The width the current line has taken.
        let mut x = 0.0;
        for piece in &self.0 {
            if piece.breaks > 0 {
                lines += piece.breaks;
                x = 0.0;
            }
            // A piece wider than the line, such as a long word, takes a line of its own:
            // the layout doesn't break it, but lets it overflow.
            if x > 0.0 && x + piece.before + piece.width > room {
                lines += 1;
                x = piece.width;
                if piece.touches {
                    x += piece.before;
                }
            } else if x > 0.0 {
                x += piece.before + piece.width;
            } else {
                x = piece.width;
            }
            if lines >= max {
                return max;
            }
        }
        lines
    }
}

/// Cuts headlines into [`Headline`]s in one font stack, and keeps them.
pub struct Lines {
    fonts: FontContext,
    layouts: LayoutContext<()>,
    /// The CSS font stack everything kept is for.
    stack: String,
    /// The headlines cut, by their text.
    headlines: HashMap<String, Rc<Headline>>,
    /// Each character's advance in CSS px, shaped on its own.
    advances: HashMap<char, f64>,
    /// What each pair of characters shapes wider than its two advances, or 0.
    pairs: HashMap<(char, char), f64>,
    /// The widths of pieces with [`CONTEXTUAL`] symbols, shaped whole.
    pieces: HashMap<String, f64>,
    /// Whether the layout keeps a run together where two pieces with [`CONTEXTUAL`]
    /// symbols meet, by the run and where they meet in it.
    joined: HashMap<(String, usize), bool>,
    /// The width an operator, the middle one, takes between spaces after and before
    /// the others, shaped together.
    spaced: HashMap<(char, char, char), f64>,
    /// What an `x` between digits adds, as in `1920x1080`.
    times: Option<f64>,
}

impl Lines {
    /// A counter that shapes in `fonts`, a copy of the app's (`fonts::font_context`).
    pub fn new(fonts: FontContext) -> Self {
        Self {
            fonts,
            layouts: LayoutContext::new(),
            stack: String::new(),
            headlines: HashMap::new(),
            advances: HashMap::new(),
            pairs: HashMap::new(),
            pieces: HashMap::new(),
            joined: HashMap::new(),
            spaced: HashMap::new(),
            times: None,
        }
    }

    /// A list's `titles`, each cut where it may wrap, in `stack`.
    ///
    /// Past [`KEPT`] headlines, only the list's are kept, so a list longer than that
    /// is cut once still.
    pub fn headlines<'a>(
        &mut self,
        stack: &str,
        titles: impl IntoIterator<Item = &'a str>,
    ) -> Rc<[Rc<Headline>]> {
        self.use_stack(stack);
        let titles: Vec<&str> = titles.into_iter().collect();
        let list: Rc<[_]> = titles.iter().map(|t| self.kept(t)).collect();
        if self.headlines.len() > KEPT {
            self.headlines = titles
                .iter()
                .zip(list.iter())
                .map(|(&t, h)| (t.to_owned(), Rc::clone(h)))
                .collect();
        }
        list
    }

    /// `text` cut where it may wrap, in `stack`.
    #[cfg(test)]
    fn headline(&mut self, stack: &str, text: &str) -> Rc<Headline> {
        self.use_stack(stack);
        self.kept(text)
    }

    /// `text` cut where it may wrap, in the stack in use, as kept or cut now.
    fn kept(&mut self, text: &str) -> Rc<Headline> {
        if let Some(headline) = self.headlines.get(text) {
            return Rc::clone(headline);
        }
        let headline = Rc::new(self.cut(text));
        self.headlines.insert(text.to_owned(), Rc::clone(&headline));
        headline
    }

    /// Makes `stack` the one shaped in, forgetting what was kept for another.
    fn use_stack(&mut self, stack: &str) {
        if stack != self.stack {
            stack.clone_into(&mut self.stack);
            self.headlines.clear();
            self.advances.clear();
            self.pairs.clear();
            self.pieces.clear();
            self.joined.clear();
            self.spaced.clear();
            self.times = None;
        }
    }

    /// `text` cut into pieces at its break opportunities.
    fn cut(&mut self, text: &str) -> Headline {
        let mut pieces: Vec<Piece> = Vec::new();
        // What the next piece follows: spaces, or the piece it touches. Spaces collapse
        // to one, unless a zero-width character keeps two apart. Nothing at the start.
        let mut spaces = 0_u32;
        // The last piece cut, where it starts and its own width, while the next one
        // may touch it.
        let mut touched: Option<(usize, f64)> = None;
        // The last character of the last piece, and the operator piece after it while
        // spaces are on both its sides so far: its index, the character before it and
        // itself.
        let mut last_char = None;
        let mut operator: Option<(usize, char, char)> = None;
        // Lines ended by separators since the last piece.
        let mut breaks = 0_u32;
        // Spaces a zero-width mark keeps at the start, as a direction mark does: the
        // first piece follows them on its line.
        let mut lead = 0.0;
        let mut start = 0;
        let xs = digit_xs(text);
        // Parley's own segmenter, as it has it with the `complex-scripts` feature: CSS
        // `line-break: auto`, with dictionaries for Thai and the like.
        let segmenter = LineSegmenter::new_dictionary(LineBreakOptions::default());
        for end in segmenter.segment_str(text).skip(1) {
            // The text up to the next break opportunity: a word and its trailing
            // spaces, or a piece of one, as of a URL or Chinese.
            let piece = text.get(start..end).unwrap_or_default();
            let at = start;
            start = end;
            let content = piece.trim_end_matches(collapsible);
            let trailing = content.len() < piece.len();
            let mut width = self.width(content);
            let content_xs = xs
                .iter()
                .filter(|&&x| (at..at + content.len()).contains(&x))
                .count();
            if content_xs > 0 {
                width += f64::from(u32::try_from(content_xs).unwrap_or(u32::MAX)) * self.times();
            }
            if width > 0.0 {
                let first = content.chars().find(|&c| !ignorable(c));
                // Set only while no spaces or separator came after it.
                if let Some((from, touched_width)) = touched {
                    let pair = boundary(text, from, at, content);
                    let join = self.join(pair, text, from, at, content, touched_width);
                    let fused =
                        pair.is_some_and(ligature) && self.unbroken(text, from, at, content);
                    match pieces.last_mut() {
                        // The font may join the two into a glyph that doesn't break.
                        Some(last) if fused => last.width += join + width,
                        _ => pieces.push(Piece {
                            width,
                            before: join,
                            touches: true,
                            breaks,
                        }),
                    }
                    operator = None;
                } else {
                    // An operator between spaces, as in `2 * 3`, may take a wider glyph
                    // in its context.
                    if let (Some((index, p, op)), Some(n), true) = (operator, first, spaces > 0) {
                        let extra = self.spaced(p, op, n);
                        if let Some(piece) = pieces.get_mut(index) {
                            piece.width = piece.width.max(extra);
                        }
                    }
                    let before = f64::from(spaces) * self.advance(' ');
                    pieces.push(Piece {
                        width: width + std::mem::take(&mut lead),
                        before,
                        touches: false,
                        breaks,
                    });
                    let mut chars = content.chars().filter(|&c| !ignorable(c));
                    operator = match (last_char, chars.next(), chars.next(), spaces > 0) {
                        (Some(p), Some(op), None, true)
                            if op == 'x' || CONTEXTUAL.contains(&op) =>
                        {
                            Some((pieces.len() - 1, p, op))
                        }
                        _ => None,
                    };
                }
                touched = Some((at, width));
                last_char = content.chars().rev().find(|&c| !ignorable(c));
                spaces = 0;
                breaks = 0;
            } else if !content.is_empty() && trailing {
                if pieces.is_empty() {
                    lead += self.advance(' ');
                } else if spaces > 0 {
                    spaces += 1;
                }
            }
            if trailing && !pieces.is_empty() {
                spaces = spaces.max(1);
                touched = None;
            } else if width > 0.0 {
                // An operator touched on its right isn't between spaces.
                operator = None;
            }
            // A line or paragraph separator ends its line, even an empty one.
            let separators = content.chars().filter(|&c| separator(c)).count();
            if separators > 0 {
                breaks += u32::try_from(separators).unwrap_or(u32::MAX);
                spaces = 0;
                touched = None;
                operator = None;
            }
        }
        if breaks > 0 {
            pieces.push(Piece {
                width: 0.0,
                before: 0.0,
                touches: false,
                breaks,
            });
        }
        Headline(pieces.into())
    }

    /// What `next`, at `at` in `text`, adds to the piece it touches, which was cut at
    /// `from` and is `touched_width` wide, the two meeting at `pair` ([`boundary`]):
    /// what the pair kerns apart, or, at a symbol a font may swap a wider glyph in for
    /// across the two, as Geist does for `-->`, what the two shaped whole add.
    fn join(
        &mut self,
        pair: Option<(char, char)>,
        text: &str,
        from: usize,
        at: usize,
        next: &str,
        touched_width: f64,
    ) -> f64 {
        let Some((a, b)) = pair else {
            return 0.0;
        };
        let pair = self.pair(a, b);
        if !CONTEXTUAL.contains(&a) && !CONTEXTUAL.contains(&b) {
            return pair;
        }
        let run = text.get(from..at + next.len()).unwrap_or_default();
        let whole = self.width(run) - touched_width - self.width(next);
        pair.max(whole)
    }

    /// Whether the layout keeps `next`, at `at` in `text`, on the line of the piece it
    /// touches, which was cut at `from`, however narrow the line: the font joins the
    /// two into a glyph that doesn't break.
    fn unbroken(&mut self, text: &str, from: usize, at: usize, next: &str) -> bool {
        let run = text.get(from..at + next.len()).unwrap_or_default();
        let key = (run.to_owned(), at - from);
        if let Some(&unbroken) = self.joined.get(&key) {
            return unbroken;
        }
        let layout = layout(self, run, Some(0.0));
        let unbroken = !layout
            .lines()
            .any(|line| line.text_range().start == at - from);
        if self.joined.len() >= KEPT {
            self.joined.clear();
        }
        self.joined.insert(key, unbroken);
        unbroken
    }

    /// The width of `piece` on one line: its characters' advances and what its pairs
    /// add; or, for a piece with [`CONTEXTUAL`] symbols, its width shaped whole if that
    /// is more.
    fn width(&mut self, piece: &str) -> f64 {
        let mut width = 0.0;
        let mut prev = None;
        for c in piece.chars() {
            width += self.advance(c);
            // Fonts kern across the invisible.
            if ignorable(c) {
                continue;
            }
            if let Some(p) = prev {
                width += self.pair(p, c);
            }
            prev = Some(c);
        }
        if piece.contains(CONTEXTUAL) {
            let whole = if let Some(&whole) = self.pieces.get(piece) {
                whole
            } else {
                let whole = f64::from(layout(self, piece, None).full_width());
                if self.pieces.len() >= KEPT {
                    self.pieces.clear();
                }
                self.pieces.insert(piece.to_owned(), whole);
                whole
            };
            width = width.max(whole);
        }
        width
    }

    /// What the `x` between two digits adds to the three shaped apart, as Inter swaps in
    /// a wider glyph there.
    fn times(&mut self) -> f64 {
        if let Some(times) = self.times {
            return times;
        }
        let together = f64::from(layout(self, "1x1", None).full_width());
        let apart = 2.0 * self.advance('1') + self.advance('x');
        let times = (together - apart).max(0.0);
        self.times = Some(times);
        times
    }

    /// The width operator `op` takes between spaces after `p` and before `n`, shaped
    /// in that context, as Inter swaps in a wider glyph for the `*` of `2 * 3`.
    fn spaced(&mut self, p: char, op: char, n: char) -> f64 {
        if let Some(&width) = self.spaced.get(&(p, op, n)) {
            return width;
        }
        let mut whole = |text: String| f64::from(layout(self, &text, None).full_width());
        let width =
            whole(format!("{p} {op} {n}")) - whole(format!("{p} ")) - whole(format!(" {n}"));
        if self.spaced.len() >= KEPT {
            self.spaced.clear();
        }
        self.spaced.insert((p, op, n), width);
        width
    }

    /// What `a` and `b` shaped together add to their two advances, if they kern apart.
    ///
    /// Not for Chinese, Japanese or Korean, which don't kern to speak of: a list of them
    /// would shape each of its many pairs.
    fn pair(&mut self, a: char, b: char) -> f64 {
        if cjk(a) || cjk(b) {
            return 0.0;
        }
        if let Some(&extra) = self.pairs.get(&(a, b)) {
            return extra;
        }
        let mut buf = [0; 8];
        let len = a.encode_utf8(&mut buf).len();
        let len = len + b.encode_utf8(&mut buf[len..]).len();
        let text = std::str::from_utf8(&buf[..len]).unwrap_or_default();
        let together = f64::from(layout(self, text, None).full_width());
        let extra = (together - self.advance(a) - self.advance(b)).max(0.0);
        self.pairs.insert((a, b), extra);
        extra
    }

    /// `c`'s advance in CSS px, shaped on its own in the stack's fonts.
    ///
    /// A soft hyphen is taken as the hyphen it shows as at a break: a row may get a
    /// line too many, never too few.
    fn advance(&mut self, c: char) -> f64 {
        if let Some(&advance) = self.advances.get(&c) {
            return advance;
        }
        let shown = if c == '\u{ad}' { '-' } else { c };
        let mut buf = [0; 4];
        let layout = layout(self, shown.encode_utf8(&mut buf), None);
        let advance = f64::from(layout.full_width());
        self.advances.insert(c, advance);
        advance
    }
}

/// The characters either side of where `next`, at `at` in `text`, meets the piece
/// before it, which starts at `from`, leaving out invisible ones.
fn boundary(text: &str, from: usize, at: usize, next: &str) -> Option<(char, char)> {
    let touched = text.get(from..at)?;
    let a = touched.chars().rev().find(|&c| !ignorable(c))?;
    let b = next.chars().find(|&c| !ignorable(c))?;
    Some((a, b))
}

/// Where `text` has an `x` between digits, with spaces or without, as in `1920x1080`,
/// `2x 16GB` or `1920 x 1080`, which Inter draws wider ([`Lines::times`]).
fn digit_xs(text: &str) -> Vec<usize> {
    if !text.contains('x') {
        return Vec::new();
    }
    let visible: Vec<(usize, char)> = text
        .char_indices()
        .filter(|&(_, c)| !collapsible(c) && !ignorable(c))
        .collect();
    visible
        .windows(3)
        .filter(|w| w[1].1 == 'x' && w[0].1.is_ascii_digit() && w[2].1.is_ascii_digit())
        .map(|w| w[1].0)
        .collect()
}

/// Whether two pieces meet at `(a, b)` with [`CONTEXTUAL`] symbols either side, or a
/// dash and one, as the `-` and `>` of `->` or the `—` and `>` of `—>`: some fonts join
/// those into one glyph that doesn't break ([`Lines::unbroken`]).
fn ligature((a, b): (char, char)) -> bool {
    let joins = |c: char| CONTEXTUAL.contains(&c) || matches!(c, '–' | '—');
    joins(a) && joins(b) && (CONTEXTUAL.contains(&a) || CONTEXTUAL.contains(&b))
}

/// Whether `c` is invisible and takes no room, as a zero-width space or a direction
/// mark.
fn ignorable(c: char) -> bool {
    matches!(c, '\u{200b}'..='\u{200f}' | '\u{2060}' | '\u{feff}')
}

/// Whether `c` is a line or paragraph separator, which ends its line.
fn separator(c: char) -> bool {
    matches!(c, '\u{2028}' | '\u{2029}')
}

/// Whether CSS collapses `c`: the spaces after a word, which a line may end in.
fn collapsible(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r' | '\u{c}')
}

/// Whether `c` is Chinese, Japanese or Korean, or their punctuation or full-width forms.
fn cjk(c: char) -> bool {
    matches!(c,
        '\u{1100}'..='\u{11ff}'     // Hangul Jamo
        | '\u{3000}'..='\u{30ff}'   // CJK punctuation, Hiragana and Katakana
        | '\u{3130}'..='\u{318f}'   // Hangul compatibility Jamo
        | '\u{3400}'..='\u{4dbf}'   // CJK extension A
        | '\u{4e00}'..='\u{9fff}'   // CJK unified ideographs
        | '\u{ac00}'..='\u{d7af}'   // Hangul syllables
        | '\u{f900}'..='\u{faff}'   // CJK compatibility ideographs
        | '\u{ff00}'..='\u{ffef}'   // Half- and full-width forms
        | '\u{20000}'..='\u{2fa1f}' // CJK extensions B and on
    )
}

/// `text` laid out in `lines`'s stack as Blitz lays out the headline, wrapped at `wrap`
/// px if given.
fn layout(lines: &mut Lines, text: &str, wrap: Option<f32>) -> parley::Layout<()> {
    let stack = FontFamily::Source(lines.stack.as_str().into());
    let mut builder = lines
        .layouts
        .ranged_builder(&mut lines.fonts, text, 1.0, true);
    builder.push_default(StyleProperty::FontFamily(stack));
    builder.push_default(StyleProperty::FontSize(SIZE));
    builder.push_default(StyleProperty::FontWeight(FontWeight::new(WEIGHT)));
    builder.push_default(StyleProperty::FontVariations((&OPTICAL_SIZE).into()));
    builder.push_default(StyleProperty::WhiteSpaceCollapse(
        parley::WhiteSpaceCollapse::Collapse,
    ));
    let mut layout = builder.build(text);
    layout.break_all_lines(wrap);
    layout
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fonts;
    use skvaller_app::Font;

    const STACK: &str = "'Inter', system-ui, sans-serif";

    fn count(lines: &mut Lines, stack: &str, text: &str, width: f64, max: u32) -> u32 {
        lines.headline(stack, text).lines(width, max)
    }

    #[test]
    fn a_list_longer_than_the_cache_stays_cut() {
        let mut lines = Lines::new(fonts::font_context());
        let titles: Vec<String> = (0..KEPT + 10).map(|i| format!("Headline {i}")).collect();
        let first = lines.headlines(STACK, titles.iter().map(String::as_str));
        let again = lines.headlines(STACK, titles.iter().map(String::as_str));
        assert!(
            first
                .iter()
                .zip(again.iter())
                .all(|(a, b)| Rc::ptr_eq(a, b))
        );
    }

    #[test]
    fn a_headline_wraps_at_the_width_up_to_the_most_lines() {
        let mut lines = Lines::new(fonts::font_context());
        let text = "A headline long enough to wrap to more than one line in a narrow list";
        assert_eq!(count(&mut lines, STACK, "Short", 300.0, 3), 1);
        assert_eq!(count(&mut lines, STACK, "", 300.0, 3), 1);
        assert_eq!(count(&mut lines, STACK, "  Short  ", 300.0, 3), 1);
        assert_eq!(count(&mut lines, STACK, text, 2000.0, 3), 1);
        assert_eq!(count(&mut lines, STACK, text, 100.0, 3), 3);
        // A word wider than the line overflows it.
        let word = "Donaudampfschifffahrtsgesellschaftskapitän";
        assert_eq!(count(&mut lines, STACK, word, 60.0, 99), 1);
        assert_eq!(
            count(&mut lines, STACK, &format!("{word} {word}"), 60.0, 99),
            2
        );
        // Line and paragraph separators end their lines, even empty ones.
        assert_eq!(count(&mut lines, STACK, "a\u{2028}\u{2029}b", 300.0, 9), 3);
        assert_eq!(count(&mut lines, STACK, "Short\u{2028}", 300.0, 9), 2);
        // Cut once, wrapped at any width.
        assert!(Rc::ptr_eq(
            &lines.headline(STACK, text),
            &lines.headline(STACK, text)
        ));
    }

    /// The lines Parley lays `text` out in at `width`, built as Blitz builds an
    /// element's text: with a tree builder, in the headline's style.
    fn laid_out(lines: &mut Lines, stack: &str, text: &str, width: f32) -> u32 {
        let style = parley::TextStyle {
            font_family: FontFamily::Source(stack.into()),
            font_size: SIZE,
            font_weight: FontWeight::new(WEIGHT),
            font_variations: (&OPTICAL_SIZE).into(),
            line_height: parley::LineHeight::FontSizeRelative(1.3),
            white_space_collapse: parley::WhiteSpaceCollapse::Collapse,
            ..Default::default()
        };
        let mut builder = lines
            .layouts
            .tree_builder(&mut lines.fonts, 1.0, true, &style);
        builder.push_text(text);
        let (mut layout, _) = builder.build();
        layout.break_all_lines(Some(width));
        u32::try_from(layout.len()).unwrap_or(u32::MAX)
    }

    /// Laid out by Parley in the headline's style, as Blitz sets it: optical size
    /// included. A count is never short and seldom over. Over six lines a little width
    /// counted on each can add up to more than a line, as for the `iiiiiiiiii WWWWWWWWWW`
    /// headline at one width or two, but hardly ever.
    #[test]
    fn counts_never_fall_short_of_the_layout() {
        let mut lines = Lines::new(fonts::font_context());
        let texts = [
            "Apple unveils new MacBook Pro with M5 chip, says it's the fastest yet",
            "WAVE: To Vote, Type AV Yo Tw, kerning-heavy capitals at the edge",
            "Rust 1.90 released: what's new for async traits and const generics",
            "Österreich: Regierung kündigt Reformpaket für Pensionen an",
            "Pourquoi la réforme des retraites est-elle bloquée ? Le gouvernement répond : pas maintenant",
            "Le budget atteint 10\u{a0}000\u{a0}€ ; « une hausse » selon\u{a0}le ministre",
            "Range 1/2 and and/or with slashes / spaced / out in a headline that is long enough",
            "COVID-19 cases rise in 2024-25 as temperatures fall to -5 degrees across the region",
            "https://example.com/a/very/long/path/to/an/article/that/goes/on",
            "soft\u{ad}hyph\u{ad}en\u{ad}ated words in a head\u{ad}line that keeps going",
            "zero\u{200b}width\u{200b}spaces\u{200b}between\u{200b}words\u{200b}here",
            "東京都で新しい公園が開園しました。「人々」が集まっています",
            "รัฐบาลประกาศมาตรการใหม่เพื่อช่วยเหลือเกษตรกรในภาคตะวันออกเฉียงเหนือ",
            "Emoji 🎉🎉 and 👩‍👩‍👧 families with combining a\u{301}ccents",
            // Shaped together, these are wider than their characters.
            "C++ <-> Rust interop: a -> b -> c -> d -> e -> f -> g -> h -> i -> j",
            "Rust tip: map Option<T> -> Result<T, E> -> Vec<T> with the ? operator",
            "**Breaking**: **Major** update to **Firefox** fixes **critical** bugs",
            "iiiiiiiiii WWWWWWWWWW iiiiiiiiii WWWWWWWWWW iiiiiiiiii WWWWWWWWWW",
            "a/b/c/d/e/f/g/h/i/j/k/l/m/n/o/p/q/r/s/t/u/v/w/x/y/z/a/b/c/d/e/f/g",
            "a-->b c<--d e==>f g<==h i-->j k<--l m==>n o<==p q-->r s<--t u==>v w<==x",
            "Converting Option->Result in Rust: why foo->bar->baz chains read better than nested matches",
            "Migrating x86->ARM: what Apple's M1->M2->M3->M4 transitions taught developers",
            "legacy <– rust learned <– rust x—>y migration lessons learned new — and more —> here",
            "Monitor shootout: 3840x2160 vs 2560x1440 vs 1920x1080 vs 1280x720 panels compared at 144Hz",
            "The Steam Deck runs at 1920 x 1080 with a big catch, for under $500 this week",
            "Dell's laptop ships with 2x 16GB RAM and costs just $299, ahead of the holidays",
            "Quick maths: 2 * 3 = 6, 4 * 5 = 20, 6 * 7 = 42, 8 * 9 = 72, A * B = C and more",
            "Matrix math: 2 x 2, 3 x 3, 4 x 4, 5 x 5, 6 x 6, 7 x 7 and 8 x 8 grids explained simply",
            // Joined in some fonts, broken in others.
            "foo->bar->baz->qux->quux->corge->grault->garply->waldo->fred->plugh->xyzzy->thud",
            "idle->loading->ready->error->retry->idle->loading and more words follow here",
            "data|>parse|>validate|>transform|>enrich|>store|>index|>publish|>notify|>done",
            "Short\u{2028}second line of the headline, which goes on",
            " \u{200e} leading lrm then words that go on and on and on and on and on and on",
            "W\u{200b}W\u{200b}W\u{200b}W\u{200b}W\u{200b}W\u{200b}W\u{200b}W\u{200b}W\u{200b}W\u{200b}W\u{200b}W\u{200b}W\u{200b}W\u{200b}W\u{200b}W\u{200b}W\u{200b}W\u{200b}W",
            // A zero-width mark between two spaces keeps both.
            "lorem \u{200e} ipsum \u{200e} dolor \u{200d} sit \u{200e} amet \u{200e} consectetur \u{200e} adipiscing \u{200e} elit \u{200e} sed",
        ];
        // The bundled fonts: System and Serif are the machine's, which may fall back in
        // ways the count doesn't see.
        let stacks =
            [Font::Inter, Font::Geist, Font::SourceSerif, Font::Literata].map(fonts::stack);
        let (mut checked, mut over, mut far) = (0_u32, 0_u32, 0_u32);
        for stack in &stacks {
            for text in texts {
                let headline = lines.headline(stack, text);
                for px in (140_u16..=520).step_by(5) {
                    let laid = laid_out(&mut lines, stack, text, f32::from(px));
                    let laid = laid.min(crate::ui::TITLE_LINES);
                    let counted = headline.lines(f64::from(px), crate::ui::TITLE_LINES);
                    assert!(
                        counted >= laid,
                        "{stack}: {text:?} at {px}: counted {counted}, laid out {laid}"
                    );
                    checked += 1;
                    over += u32::from(counted > laid);
                    far += u32::from(counted > laid + 1);
                }
            }
        }
        assert!(over * 20 < checked, "{over} of {checked} counted over");
        assert!(
            far * 1_000 < checked,
            "{far} of {checked} counted 2+ lines over"
        );
    }

    /// Run with `cargo test -p skvaller --release five_thousand -- --ignored --nocapture`:
    /// in a debug build the timing says little. Cutting runs when headlines first show;
    /// wrapping at every new width, as in a drag, so it has to fit in a frame.
    #[test]
    #[ignore = "a timing, for release builds"]
    fn five_thousand_headlines_count_quickly() {
        let mut lines = Lines::new(fonts::font_context());
        // Every fifth with a hyphenated or slashed piece of its own, which is shaped whole.
        let titles: Vec<String> = (0..5_000)
            .map(|i| match i % 5 {
                0 => format!("Model-{i} launches with a well-known and/or long-awaited feature"),
                1 => format!("{i} things happened today, and here is what we know so far"),
                2 => format!("Headline {i}: something happened somewhere, and here is more"),
                3 => format!("Why the {i}th release matters more than you would think"),
                _ => format!("Stocks rise {i}% as markets close higher for the week"),
            })
            .collect();
        let start = std::time::Instant::now();
        let headlines: Vec<_> = titles.iter().map(|t| lines.headline(STACK, t)).collect();
        let cut = start.elapsed();
        eprintln!("5,000 headlines cut: {cut:?}");
        assert!(cut.as_millis() < 50, "{cut:?}");
        for width in [300.0, 400.0] {
            let start = std::time::Instant::now();
            for headline in &headlines {
                std::hint::black_box(
                    headline.lines(std::hint::black_box(width), crate::ui::TITLE_LINES),
                );
            }
            let wrapped = start.elapsed();
            eprintln!("5,000 headlines wrapped: {wrapped:?}");
            assert!(wrapped.as_millis() < 5, "{wrapped:?}");
        }
    }
}
