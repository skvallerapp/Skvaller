//! Where a scrolled list shows its rows: which rows to render, how the article list
//! keeps its place when its rows change, and how far to scroll to show a row.
//!
//! Plain arithmetic on row offsets, without a document: the components read their
//! elements, ask here where to be, and scroll there.

use std::rc::Rc;

use skvaller_core::ArticleSummary;

/// Rows rendered above and below the visible ones, so a fast scroll doesn't show gaps
/// before the next render.
const OVERSCAN: usize = 8;

/// Where a highlight is drawn, in CSS pixels down its list's content.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Place {
    pub top: f64,
    pub height: f64,
}

/// Whether two sizes or positions differ by half a pixel or more: less is rounding, as a
/// pointer move within the same pixel.
pub fn differs(a: f64, b: f64) -> bool {
    (a - b).abs() >= 0.5
}

/// Where a list `height` tall, scrolled to `offset`, scrolls the least to show what
/// spans `top..bottom` of its content. None when it shows already. Something taller
/// than the list shows its top.
pub fn reveal(offset: f64, height: f64, top: f64, bottom: f64) -> Option<f64> {
    if top < offset {
        Some(top)
    } else if bottom > offset + height {
        Some((bottom - height).min(top))
    } else {
        None
    }
}

/// The rows to render, `first..last`, for a viewport scrolled to `top` that is `height`
/// tall, with `OVERSCAN` rows either side. `offsets` holds where each row starts, then
/// where the last ends.
pub fn visible_rows(offsets: &[f64], top: f64, height: f64) -> (usize, usize) {
    let len = offsets.len().saturating_sub(1);
    // The row that contains `y`: the last that starts at or above it.
    let row_at = |y: f64| offsets.partition_point(|&o| o <= y).saturating_sub(1);
    let first = row_at(top).saturating_sub(OVERSCAN).min(len);
    let last = (row_at(top + height) + 1 + OVERSCAN).min(len);
    (first.min(last), last)
}

/// Where the article list scrolls to bring the opened article `id` into view, if it
/// must. `top` is where the list is scrolled, and `waiting` where a waiting scroll
/// takes it. This scroll replaces that one, so the row is shown from there.
pub fn reveal_row(
    offsets: &[f64],
    rows: &[ArticleSummary],
    id: &str,
    top: f64,
    waiting: Option<f64>,
    height: f64,
) -> Option<f64> {
    let i = rows.iter().position(|a| a.id == id)?;
    let (row_top, row_bottom) = (*offsets.get(i)?, *offsets.get(i + 1)?);
    reveal(waiting.unwrap_or(top), height, row_top, row_bottom)
}

/// What the article list is in one render, for [`Keeper::place`].
pub struct ListView<'a> {
    /// Where each row starts, then where the last ends.
    pub offsets: &'a Rc<[f64]>,
    pub rows: &'a [ArticleSummary],
    /// Where the list is scrolled.
    pub top: f64,
    /// Where a scroll waiting for the layout takes it, if one is.
    pub waiting: Option<f64>,
    /// The viewport's height.
    pub height: f64,
    /// The open article's row.
    pub selected: Option<usize>,
}

/// Where one render draws the article list, from [`Keeper::place`].
#[derive(Debug, PartialEq)]
pub struct Placed {
    /// Where the rows are drawn from: the viewport, or where a hold takes it.
    pub top: f64,
    /// The furthest the list scrolls.
    pub end: f64,
    /// A scroll to start, which replaces any waiting: until it lands, `top` stays
    /// where the viewport is, and the renders draw the rows there too.
    pub scroll: Option<f64>,
    /// The rows to render; in a render that holds the list, also those where the
    /// viewport still is, which may be far from the others.
    pub shown: Vec<usize>,
}

/// Keeps the article list in place from one render to the next. Rows whose heights
/// changed, for a new width, font or thumbnail size, or rows synced in above, keep the
/// list where it was; a list scrolled past its end goes to the end.
#[derive(Debug, Default)]
pub struct Keeper {
    anchor: Option<Anchor>,
}

impl Keeper {
    /// Where to draw the list in `view`, and where to scroll it, if anywhere. Takes the
    /// anchor the next render holds by.
    pub fn place(&mut self, view: &ListView<'_>) -> Placed {
        let height = view.height;
        let total = view.offsets.last().copied().unwrap_or(0.0);
        // A list grown shorter, as by a smaller thumbnail size, shows its end, but Blitz
        // keeps the scroll position past it, so the list is scrolled back below.
        let end = (total - height).max(0.0);
        let mut top = view.top.min(end);
        // Where the list is scrolled, or where a waiting scroll takes it. That scroll
        // may be a hold's, or one set apart from the anchor, as to the open article or
        // a search's end; the anchor moves with it.
        let scroll = view.waiting.unwrap_or(view.top);
        let held = self
            .anchor
            .as_ref()
            .and_then(|a| a.hold(view.offsets, view.rows, top, scroll));
        let goal = held
            .map(|target| target.min(end).max(0.0))
            .or_else(|| (scroll > end + 0.5).then_some(end));
        // Where the viewport is until the list scrolls to its goal.
        let still_at = top;
        // Where the list is held, which the anchor is taken at: its goal, even when that
        // is under half a pixel off and not scrolled to, so steps that small don't add
        // up.
        let mut anchor_top = view.waiting.unwrap_or(top);
        let mut start = None;
        if let Some(target) = goal {
            anchor_top = target;
            if differs(target, scroll) {
                top = target;
                start = Some(target);
            }
        }
        let shown = {
            let (first, last) = visible_rows(view.offsets, top, height);
            let (was_first, was_last) = visible_rows(view.offsets, still_at, height);
            (first..last)
                .chain((was_first..was_last).filter(|i| !(first..last).contains(i)))
                .collect()
        };
        // A scroll started here replaces the one waiting.
        let scroll_after = start.or(view.waiting).unwrap_or(view.top);
        self.anchor = Anchor::at(
            view.offsets,
            view.rows,
            anchor_top,
            scroll_after,
            height,
            view.selected,
        );
        Placed {
            top,
            end,
            scroll: start,
            shown,
        }
    }
}

/// Where the article list was, by a row, so it stays in place when its rows' heights
/// change: the open article's row keeps its place in the viewport while it shows,
/// otherwise the top row keeps how far into it the viewport starts. A list at its top
/// stays there.
#[derive(Clone, Debug)]
struct Anchor {
    /// The offsets the list was placed by, so a change of them shows.
    offsets: Rc<[f64]>,
    /// Where the list was scrolled then, or where a hold waiting to scroll takes it:
    /// the scroll position, not where the viewport shows it, which a list grown
    /// shorter clamps.
    scroll: f64,
    id: String,
    index: usize,
    hold: Hold,
}

/// How an [`Anchor`]'s row keeps the list in place.
#[derive(Clone, Copy, Debug)]
enum Hold {
    /// The row starts this far below the viewport's top.
    Below(f64),
    /// The viewport starts this fraction of the way into the row.
    Into(f64),
}

impl Anchor {
    /// The anchor for a list placed by `offsets`, held with the viewport at `top` and
    /// scrolled to `scroll` ([`Anchor::scroll`]): by the open article's row,
    /// `selected`, while it shows, or else by the top row. None at the top of the list.
    fn at(
        offsets: &Rc<[f64]>,
        rows: &[ArticleSummary],
        top: f64,
        scroll: f64,
        height: f64,
        selected: Option<usize>,
    ) -> Option<Self> {
        if top <= 0.0 {
            return None;
        }
        let shows = |&i: &usize| offsets[i + 1] > top && offsets[i] < top + height;
        let (index, hold) = if let Some(i) = selected.filter(shows) {
            (i, Hold::Below(offsets[i] - top))
        } else {
            let i = offsets.partition_point(|&o| o <= top).saturating_sub(1);
            let row = offsets.get(i + 1)? - offsets[i];
            (i, Hold::Into((top - offsets[i]) / row.max(1.0)))
        };
        Some(Self {
            offsets: Rc::clone(offsets),
            scroll,
            id: rows.get(index)?.id.clone(),
            index,
            hold,
        })
    }

    /// Where the viewport starts to keep the list in place now that it is placed by
    /// `offsets` and has `rows`, if either moved its row, and the row is still there.
    /// None while the viewport shows the list at its top, `top`, which stays there.
    ///
    /// `scroll` is where the list is scrolled now, or where a scroll waiting takes it: a
    /// scroll since the anchor was taken, as in the render that brings rows synced in,
    /// moves the list too. A hold's own wait is where the anchor was taken, so it adds
    /// nothing. Under half a pixel is taken as rounding.
    fn hold(
        &self,
        offsets: &Rc<[f64]>,
        rows: &[ArticleSummary],
        top: f64,
        scroll: f64,
    ) -> Option<f64> {
        let in_place = rows.get(self.index).is_some_and(|a| a.id == self.id);
        // Offsets kept as they were may still hold other rows, as after a sync that
        // added one above and dropped one below.
        if (Rc::ptr_eq(&self.offsets, offsets) && in_place) || top <= 0.0 {
            return None;
        }
        let index = if in_place {
            self.index
        } else {
            rows.iter().position(|a| a.id == self.id)?
        };
        let (start, end) = (offsets[index], offsets[index + 1]);
        let held = match self.hold {
            Hold::Below(below) => start - below,
            Hold::Into(into) => start + into * (end - start),
        };
        let since = if differs(scroll, self.scroll) {
            scroll - self.scroll
        } else {
            0.0
        };
        Some(held + since)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn articles(n: usize) -> Vec<ArticleSummary> {
        (0..n)
            .map(|i| ArticleSummary {
                id: i.to_string(),
                feed_id: String::new(),
                title: String::new(),
                author: String::new(),
                url: String::new(),
                published: 0,
                read: false,
                starred: false,
                thumbnail: false,
            })
            .collect()
    }

    /// Offsets for rows of these heights.
    fn offsets(heights: impl IntoIterator<Item = f64>) -> Rc<[f64]> {
        let mut at = 0.0;
        std::iter::once(0.0)
            .chain(heights.into_iter().map(|h| {
                at += h;
                at
            }))
            .collect()
    }

    /// `n` rows, each `h` tall.
    fn even(n: usize, h: f64) -> Rc<[f64]> {
        offsets(std::iter::repeat_n(h, n))
    }

    const HEIGHT: f64 = 600.0;

    fn view<'a>(
        offsets: &'a Rc<[f64]>,
        rows: &'a [ArticleSummary],
        top: f64,
        waiting: Option<f64>,
    ) -> ListView<'a> {
        ListView {
            offsets,
            rows,
            top,
            waiting,
            height: HEIGHT,
            selected: None,
        }
    }

    #[test]
    fn reveal_scrolls_the_least() {
        assert_eq!(reveal(100.0, 300.0, 150.0, 200.0), None);
        assert_eq!(reveal(100.0, 300.0, 50.0, 90.0), Some(50.0));
        assert_eq!(reveal(100.0, 300.0, 380.0, 450.0), Some(150.0));
        // Taller than the list: its top.
        assert_eq!(reveal(0.0, 300.0, 400.0, 1_000.0), Some(400.0));
    }

    #[test]
    fn the_list_stays_in_place_when_its_rows_change_height() {
        let list = articles(100);
        let narrow = even(100, 200.0);
        let wide = even(100, 150.0);
        // Halfway into row 10, the top row: halfway into it still.
        let top = 10.0 * 200.0 + 100.0;
        let anchor = Anchor::at(&narrow, &list, top, top, HEIGHT, None).unwrap();
        assert_eq!(anchor.hold(&narrow, &list, top, anchor.scroll), None);
        assert_eq!(
            anchor.hold(&wide, &list, top, anchor.scroll),
            Some(10.0 * 150.0 + 75.0)
        );
        // The open article's row, showing, keeps its place in the viewport.
        let anchor = Anchor::at(&narrow, &list, top, top, HEIGHT, Some(11)).unwrap();
        let below = narrow[11] - top;
        assert_eq!(
            anchor.hold(&wide, &list, top, anchor.scroll),
            Some(wide[11] - below)
        );
        // One out of view doesn't, and the top of the list stays there.
        let anchor = Anchor::at(&narrow, &list, top, top, HEIGHT, Some(90)).unwrap();
        assert_eq!(anchor.id, "10");
        assert!(Anchor::at(&narrow, &list, 0.0, 0.0, HEIGHT, Some(11)).is_none());
        // A row synced in above moves the anchor's row down.
        let mut synced = articles(101);
        synced.rotate_right(1);
        synced[0].id = "new".into();
        let moved = even(101, 200.0);
        let anchor = Anchor::at(&narrow, &list, top, top, HEIGHT, None).unwrap();
        assert_eq!(
            anchor.hold(&moved, &synced, top, anchor.scroll),
            Some(top + 200.0)
        );
        // Also when the rows are as high as before, and the offsets stay as they were.
        synced.pop();
        assert_eq!(
            anchor.hold(&narrow, &synced, top, anchor.scroll),
            Some(top + 200.0)
        );
        // A scroll since the anchor was taken moves the list too, by the scroll
        // position: not by where the viewport shows, which a shorter list clamps.
        assert_eq!(
            anchor.hold(&wide, &list, top, top + 50.0),
            Some(10.0 * 150.0 + 75.0 + 50.0)
        );
        // Deep in the list, rows grown shorter make the viewport clamp to the end.
        let short = even(100, 40.0);
        let deep = 60.0 * 200.0 + 100.0;
        let anchor = Anchor::at(&narrow, &list, deep, deep, HEIGHT, None).unwrap();
        let clamped = short[100] - HEIGHT;
        assert!(clamped < deep);
        assert_eq!(
            anchor.hold(&short, &list, clamped, deep),
            Some(60.0 * 40.0 + 20.0)
        );
    }

    #[test]
    fn rows_grown_above_the_viewport_scroll_it_down_by_as_much() {
        let list = articles(100);
        let before = even(100, 100.0);
        let mut keeper = Keeper::default();
        let top = 5_050.0;
        let placed = keeper.place(&view(&before, &list, top, None));
        assert_eq!(placed.scroll, None);
        assert_eq!(placed.top, top);
        // Rows 0 to 9 grow by 20 px each: the list holds halfway into row 50.
        let after = offsets((0..100).map(|i| if i < 10 { 120.0 } else { 100.0 }));
        let placed = keeper.place(&view(&after, &list, top, None));
        assert_eq!(placed.scroll, Some(top + 200.0));
        assert_eq!(placed.top, top + 200.0);
        // Until it scrolls, the rows where the viewport still is render too.
        let (from, to) = visible_rows(&after, top, HEIGHT);
        assert!((from..to).all(|i| placed.shown.contains(&i)));
        // While the scroll waits, the next render holds there and starts none.
        let placed = keeper.place(&view(&after, &list, top, Some(top + 200.0)));
        assert_eq!((placed.scroll, placed.top), (None, top));
        // Once it lands, nothing moves.
        let placed = keeper.place(&view(&after, &list, top + 200.0, None));
        assert_eq!((placed.scroll, placed.top), (None, top + 200.0));
    }

    #[test]
    fn heights_changing_back_while_a_hold_waits_scroll_back() {
        let list = articles(100);
        let before = even(100, 100.0);
        let after = even(100, 120.0);
        let mut keeper = Keeper::default();
        let top = 5_050.0;
        keeper.place(&view(&before, &list, top, None));
        let placed = keeper.place(&view(&after, &list, top, None));
        let target = placed.scroll.unwrap();
        assert_eq!(target, 50.0 * 120.0 + 60.0);
        // The anchor was taken at the hold's target, not at the viewport: changing back
        // returns to where the list was.
        let placed = keeper.place(&view(&before, &list, top, Some(target)));
        assert_eq!(placed.scroll, Some(top));
    }

    #[test]
    fn opening_an_article_off_screen_scrolls_to_its_row() {
        let list = articles(100);
        let rows = even(100, 100.0);
        // Rows 10 to 15 show from 1,000.
        assert_eq!(reveal_row(&rows, &list, "12", 1_000.0, None, HEIGHT), None);
        // Below: its bottom at the viewport's bottom.
        assert_eq!(
            reveal_row(&rows, &list, "30", 1_000.0, None, HEIGHT),
            Some(3_100.0 - HEIGHT)
        );
        // Above: its top at the viewport's top.
        assert_eq!(
            reveal_row(&rows, &list, "3", 1_000.0, None, HEIGHT),
            Some(300.0)
        );
        // Gone from the list: nothing.
        assert_eq!(reveal_row(&rows, &list, "x", 1_000.0, None, HEIGHT), None);
    }

    #[test]
    fn a_reveal_replacing_a_waiting_scroll_starts_from_its_target() {
        let list = articles(100);
        let rows = even(100, 100.0);
        // The list shows row 12, but a scroll waiting takes it to row 50: the row is
        // brought back into view.
        assert_eq!(
            reveal_row(&rows, &list, "12", 1_000.0, Some(5_000.0), HEIGHT),
            Some(1_200.0)
        );
        // Row 52 is off screen now but shows once the scroll lands: nothing more.
        assert_eq!(
            reveal_row(&rows, &list, "52", 1_000.0, Some(5_000.0), HEIGHT),
            None
        );
    }

    #[test]
    fn a_list_scrolled_past_its_end_goes_to_the_end() {
        let list = articles(20);
        let rows = even(20, 100.0);
        let mut keeper = Keeper::default();
        // Blitz keeps a scroll position past the end of a list grown shorter.
        let placed = keeper.place(&view(&rows, &list, 3_000.0, None));
        assert_eq!(placed.end, 2_000.0 - HEIGHT);
        assert_eq!(placed.scroll, Some(placed.end));
        assert_eq!(placed.top, placed.end);
        // A hold beyond the end stops there too: halfway into row 16 would be at 1,650.
        let mut keeper = Keeper::default();
        let tall = even(20, 200.0);
        keeper.place(&view(&tall, &list, 3_300.0, None));
        let placed = keeper.place(&view(&rows, &list, 3_300.0, None));
        assert_eq!(placed.scroll, Some(1_400.0));
        assert_eq!(placed.top, 1_400.0);
        // The end itself, and a list shorter than the viewport, stay.
        let placed = keeper.place(&view(&rows, &list, 1_400.0, None));
        assert_eq!(placed.scroll, None);
        let short = even(3, 100.0);
        let placed = Keeper::default().place(&view(&short, &list[..3], 0.0, None));
        assert_eq!((placed.end, placed.scroll), (0.0, None));
    }
}
