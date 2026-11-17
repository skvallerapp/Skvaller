//! Pulling a list past its top or bottom, by trackpad or touch, stretches it away from
//! the edge, and it springs back when let go. Given a refresh, a pull at the top that
//! goes far enough runs it, if it began with the list at the top; the list waits there
//! until the refresh ends, or until it is scrolled down. A trackpad's momentum that
//! reaches an edge bounces the list off it, as macOS does. A mouse wheel doesn't stretch
//! it. With animations off the list stays put, and a long pull at the top refreshes at
//! once.
//!
//! A trackpad on macOS or Wayland says when the fingers lift, and which events are
//! momentum: the list springs back as they lift. Elsewhere the fingers are guessed to have
//! lifted when the events stop or start to shrink as momentum's do.
//!
//! This is the state alone: the list feeds it the events, scrolls what it says, and
//! draws the offset, which CSS transitions while the list springs.

use std::time::{Duration, Instant};

use dioxus_native::{BlitzScrollSource as Source, BlitzWheelPhase as Phase};

/// The rubber band's give: the further the pull, the less the list follows.
const GIVE: f64 = 0.55;
/// How far down a pull at the top must stretch the list to refresh.
const TRIGGER: f64 = 90.0;
/// Where the list waits while it refreshes.
pub(crate) const HOLD: f64 = 48.0;
/// A trackpad that doesn't say when the fingers lift sends no event once they and their
/// momentum stop: after this long without one, the pull is let go.
pub(crate) const WHEEL_IDLE: Duration = Duration::from_millis(120);
/// A trackpad's scrolling is accelerated, so a quick swipe goes far: it pulls this much
/// less than a finger on a touchscreen does.
const WHEEL_PULL: f64 = 0.4;
/// A trackpad accelerates a quick flick to huge events. One pulls this much at most, so
/// the pull follows how far the fingers go, not how fast.
const WHEEL_MAX: f64 = 40.0;
/// After the fingers lift, a trackpad's momentum goes on as events that shrink smoothly,
/// give or take a pixel or a doubled event. For a trackpad that doesn't say which events
/// are momentum, this many smaller ones are, not a hand.
const COASTING: u32 = 3;
/// Momentum shrinks by less than this from one event to the next. A hand slowing down
/// drops more at once.
const SMOOTH: f64 = 0.8;
/// Fingers back on the trackpad grow the events this many times in a row; momentum
/// never does.
const HAND_BACK: u32 = 2;
/// Momentum that reaches an edge bounces the list off it this far at most.
const BOUNCE_MAX: f64 = 60.0;
/// How long the list takes to spring back, and to bounce out.
const SETTLE: Duration = Duration::from_millis(400);
const BOUNCE: Duration = Duration::from_millis(120);

/// How far the list stretches for a pull of `x`, at most `d`. Signed, as `x` is.
fn stretch(x: f64, d: f64) -> f64 {
    x.signum() * (1.0 - 1.0 / ((x.abs() * GIVE) / d + 1.0)) * d
}

/// The pull that stretches the list `y`: the inverse of [`stretch`].
fn pull_for(y: f64, d: f64) -> f64 {
    y.signum() * (d / GIVE) * (1.0 / (1.0 - (y.abs() / d).min(0.99)) - 1.0)
}

/// Where the list is scrolled when an event comes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Edges {
    /// At its top.
    pub top: bool,
    /// At its end.
    pub bottom: bool,
    /// How tall the list shows, which is as far as it stretches.
    pub height: f64,
}

impl Edges {
    /// Whether the list is at the edge that a pull of `delta` (down positive) pulls from.
    fn at(self, delta: f64) -> bool {
        if delta > 0.0 { self.top } else { self.bottom }
    }
}

/// The ease of a [`Spring`], which CSS transitions: an ease-out cubic, as [`Spring::at`]
/// computes it.
pub(crate) const SPRING_EASE: &str = "cubic-bezier(0.33, 1, 0.68, 1)";

/// The list springing from one offset to another, eased out as [`SPRING_EASE`] is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Spring {
    pub from: f64,
    pub duration: Duration,
    /// Tells this spring from others, for the timer that ends it.
    pub id: u64,
    start: Instant,
    to: f64,
    then: Then,
}

impl Spring {
    /// Where the list is `now` on its way.
    fn at(&self, now: Instant) -> f64 {
        let t =
            (now.duration_since(self.start).as_secs_f64() / self.duration.as_secs_f64()).min(1.0);
        self.from + (self.to - self.from) * (1.0 - (1.0 - t).powi(3))
    }
}

/// What follows a spring.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Then {
    Rest,
    /// A bounce springs back.
    SpringBack,
    /// The refresh is over.
    EndRefresh,
}

/// What the list shows, for its render.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Shown {
    /// How far the list is stretched from its edge, in px, down from the top positive;
    /// where it springs to while it springs.
    pub offset: f64,
    /// The list springs to `offset`.
    pub spring: Option<Spring>,
    /// A refresh runs, with the list held at [`HOLD`], or the list springs back after
    /// one.
    pub refreshing: bool,
    /// The list refreshes, and this gesture began with it at the top, so pulling down
    /// can refresh.
    pub from_top: bool,
}

impl Shown {
    /// How far a pull at the top is towards a refresh, 0 to 1.
    pub fn progress(&self) -> f64 {
        (self.offset.max(0.0) / TRIGGER).min(1.0)
    }

    /// Letting go refreshes, or a refresh runs.
    pub fn armed(&self) -> bool {
        self.progress() >= 1.0 || self.refreshing
    }

    /// The refresh icon shows in the gap above the list, while a pull at the top or a
    /// refresh pulls it down, and as it springs back from that.
    pub fn indicator(&self) -> bool {
        let down = self.offset > 0.0 || self.spring.is_some_and(|s| s.from > 0.0);
        down && (self.from_top || self.refreshing)
    }
}

/// What the list does after an event.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
#[must_use]
pub(crate) struct Out {
    /// Keep the event from scrolling the list.
    pub prevent: bool,
    /// Start the refresh, then call [`Overscroll::refreshed`].
    pub refresh: bool,
    /// Scroll the list this far down itself: a touch that began as a pull isn't
    /// scrolled by Blitz.
    pub scroll_by: f64,
    /// Call [`Overscroll::idle`] with this after [`WHEEL_IDLE`].
    pub idle: Option<u64>,
    /// Call [`Overscroll::sprung`] with the spring's id once it has run.
    pub spring: Option<(u64, Duration)>,
}

/// A list's overscroll.
#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a gesture's flags, each independent of the others"
)]
pub(crate) struct Overscroll {
    /// Settings → Animations.
    pub animate: bool,
    /// A pull at the top can refresh the list.
    refreshes: bool,
    offset: f64,
    spring: Option<Spring>,
    springs: u64,
    refreshing: bool,
    from_top: bool,
    /// The pull so far, before the rubber band.
    pull: f64,
    /// With animations off, this gesture already refreshed.
    spent: bool,
    /// When the last wheel event came.
    wheel_at: Option<Instant>,
    /// Counts the wheel events, so only the last one's idle timer lets go.
    wheels: u64,
    /// The size of the last wheel event.
    wheel_size: f64,
    /// How many wheel events in a row shrank smoothly. An equal one doesn't end the run.
    shrinking: u32,
    /// How many wheel events in a row were larger than the one before.
    growing: u32,
    /// The wheel events are momentum: the fingers have lifted.
    coasting: bool,
    /// This momentum already bounced the list off an edge, or sprang it back.
    bounced: bool,
    /// The pull of that run: momentum, once it shows as such.
    coasted: f64,
    touch_y: Option<f64>,
    /// This touch was kept from scrolling the list, so it scrolls it here instead.
    touch_owned: bool,
    /// Gathered for the event's [`Out`].
    out: Out,
}

impl Overscroll {
    pub fn new(animate: bool, refreshes: bool) -> Self {
        Self {
            animate,
            refreshes,
            offset: 0.0,
            spring: None,
            springs: 0,
            refreshing: false,
            from_top: false,
            pull: 0.0,
            spent: false,
            wheel_at: None,
            wheels: 0,
            wheel_size: 0.0,
            shrinking: 0,
            growing: 0,
            coasting: false,
            bounced: false,
            coasted: 0.0,
            touch_y: None,
            touch_owned: false,
            out: Out::default(),
        }
    }

    pub fn shown(&self) -> Shown {
        Shown {
            offset: self.offset,
            spring: self.spring,
            refreshing: self.refreshing,
            from_top: self.can_refresh(),
        }
    }

    fn done(&mut self) -> Out {
        std::mem::take(&mut self.out)
    }

    /// Where the list is now, springing or not.
    fn now_at(&self, now: Instant) -> f64 {
        self.spring.map_or(self.offset, |s| s.at(now))
    }

    /// A wheel event: `dy` as Blitz signs it, positive toward the top. A `source` of
    /// [`Source::Wheel`] includes any event in lines rather than pixels.
    pub fn wheel(
        &mut self,
        now: Instant,
        (dx, dy): (f64, f64),
        zoom: bool,
        (phase, source): (Phase, Source),
        edges: Edges,
    ) -> Out {
        if !zoom {
            match source {
                Source::Wheel => {}
                Source::Finger => self.on_finger(now, phase, dx, dy, edges),
                Source::Momentum => {
                    if dy.abs() > dx.abs() {
                        self.left_top(now, edges);
                        self.coast(now, dy, edges);
                    }
                }
                Source::Unknown => self.on_wheel(now, dx, dy, edges),
            }
        }
        self.done()
    }

    /// Fingers on a trackpad that says when they land and lift.
    fn on_finger(&mut self, now: Instant, phase: Phase, dx: f64, dy: f64, edges: Edges) {
        match phase {
            Phase::Started => self.began(edges),
            Phase::Moved => {}
            Phase::Ended => {
                // The fingers lifted: a stretched list springs back, or refreshes, at
                // once. The momentum that follows neither stretches it nor scrolls it.
                if self.pull != 0.0 {
                    self.release(now, edges);
                }
                return;
            }
        }
        if dy.abs() <= dx.abs() {
            return;
        }
        self.left_top(now, edges);
        let pull = dy.signum() * dy.abs().min(WHEEL_MAX) * WHEEL_PULL;
        if self.take(now, pull, edges) {
            self.out.prevent = true;
        }
    }

    /// A trackpad that doesn't say when the fingers lift, or which events are momentum.
    fn on_wheel(&mut self, now: Instant, dx: f64, dy: f64, edges: Edges) {
        // A pause in the events starts a new gesture.
        if self
            .wheel_at
            .is_none_or(|at| now.duration_since(at) > WHEEL_IDLE)
        {
            self.began(edges);
        }
        self.wheel_at = Some(now);
        if dy.abs() <= dx.abs() {
            return;
        }
        self.left_top(now, edges);
        let size = dy.abs();
        let shrank = size < self.wheel_size && size >= self.wheel_size * SMOOTH;
        // Trackpads repeat a size exactly: an event of the same size goes on a run.
        #[expect(clippy::float_cmp, reason = "the sizes come as they are, not computed")]
        let same = size == self.wheel_size;
        // This event may be momentum's: it goes on a run of smoothly shrinking ones.
        let momentum = shrank || (same && self.shrinking > 0);
        if shrank {
            self.shrinking += 1;
        } else if !same {
            self.shrinking = 0;
            self.coasted = 0.0;
        }
        self.growing = if size > self.wheel_size {
            self.growing + 1
        } else {
            0
        };
        self.wheel_size = size;
        if self.shrinking >= COASTING && !self.coasting {
            self.coasting = true;
            // The momentum before it showed as such doesn't count towards a refresh.
            self.pull -= self.coasted;
        }
        // The fingers are back, as a new gesture.
        if self.coasting && self.growing >= HAND_BACK {
            self.began(edges);
        }
        if self.coasting {
            self.coast(now, dy, edges);
            return;
        }
        let pull = dy.signum() * size.min(WHEEL_MAX) * WHEEL_PULL;
        if !self.take(now, pull, edges) {
            return;
        }
        if momentum {
            self.coasted += pull;
        }
        self.out.prevent = true;
        self.wheels += 1;
        self.out.idle = Some(self.wheels);
    }

    /// A momentum event, `dy` as Blitz signs it.
    fn coast(&mut self, now: Instant, dy: f64, edges: Edges) {
        // While it refreshes, the list stays held: momentum may scroll it, but neither
        // bounces nor releases it.
        if self.refreshing {
            return;
        }
        let at = self.now_at(now);
        if self.pull != 0.0 || (at != 0.0 && edges.at(at)) {
            // The fingers lifted: a stretched list springs back at once, as it does
            // natively, and the momentum neither stretches it nor scrolls it.
            self.out.prevent = true;
            self.bounced = true;
            if self.spring.is_none() {
                self.release(now, edges);
            }
        } else if !self.bounced && edges.at(dy) && self.animate {
            // Momentum reached an edge: it bounces the list off it.
            self.bounced = true;
            self.out.prevent = true;
            let out = dy.signum() * dy.abs().min(BOUNCE_MAX);
            self.settle(now, out, BOUNCE, Then::SpringBack);
        }
    }

    /// No wheel event came for a while after the one `wheel` counted.
    pub fn idle(&mut self, now: Instant, wheel: u64, edges: Edges) -> Out {
        if wheel == self.wheels {
            self.release(now, edges);
        }
        self.done()
    }

    /// A touch starts, at `y` if it's the only one.
    pub fn touch_start(&mut self, y: Option<f64>, edges: Edges) -> Out {
        self.touch_y = y;
        self.touch_owned = false;
        self.began(edges);
        self.done()
    }

    /// The touch moves, to `y` if it's the only one.
    pub fn touch_move(&mut self, now: Instant, y: Option<f64>, edges: Edges) -> Out {
        let Some(y) = y else {
            return self.done();
        };
        // A finger left alone by a second one lifting has no start yet: it starts here.
        let delta = self.touch_y.map_or(0.0, |from| y - from);
        self.touch_y = Some(y);
        self.left_top(now, edges);
        if self.take(now, delta, edges) {
            self.out.prevent = true;
            self.touch_owned = true;
        } else if self.touch_owned {
            self.out.scroll_by = -delta;
            self.out.prevent = true;
        }
        self.done()
    }

    /// A touch ends, with `left` touches still on.
    pub fn touch_end(&mut self, now: Instant, left: usize, edges: Edges) -> Out {
        if left == 0 {
            self.touch_y = None;
            self.touch_owned = false;
            self.release(now, edges);
        }
        self.done()
    }

    /// The system took the touch over, as for a gesture of its own: no refresh.
    pub fn touch_cancel(&mut self, now: Instant, left: usize, edges: Edges) -> Out {
        self.from_top = false;
        self.touch_end(now, left, edges)
    }

    /// The refresh ended: the list springs back.
    pub fn refreshed(&mut self, now: Instant) -> Out {
        self.settle(now, 0.0, SETTLE, Then::EndRefresh);
        self.done()
    }

    /// The spring `id` has run its course.
    pub fn sprung(&mut self, now: Instant, id: u64) -> Out {
        if let Some(spring) = self.spring.filter(|s| s.id == id) {
            self.spring = None;
            self.then(now, spring.then);
        }
        self.done()
    }

    /// Takes `delta` px of pull (down positive), if the list is at that edge or already
    /// stretched. Returns whether it took it.
    fn take(&mut self, now: Instant, delta: f64, edges: Edges) -> bool {
        if self.refreshing || delta == 0.0 {
            return false;
        }
        let d = edges.height.max(1.0);
        if let Some(spring) = self.spring.take() {
            // Caught while springing back at its edge, the pull goes on from where the
            // list is. Away from it, as after a refresh the list was scrolled from, it's
            // done.
            let at = spring.at(now);
            if edges.at(at) {
                self.offset = at;
                self.pull = pull_for(at, d);
            } else {
                self.offset = 0.0;
            }
        }
        if self.pull == 0.0 && !edges.at(delta) {
            return false;
        }
        let next = self.pull + delta;
        // Pushed back past the edge, the list rests there.
        self.pull = if self.pull != 0.0 && next.signum() != self.pull.signum() {
            0.0
        } else {
            next
        };
        if self.animate {
            self.offset = stretch(self.pull, d);
        } else if stretch(self.pull - self.coasted, d) >= TRIGGER
            && self.can_refresh()
            && !self.spent
        {
            // Momentum before it shows as such doesn't count, as when the list stretches.
            self.spent = true;
            self.start(now);
        }
        true
    }

    fn release(&mut self, now: Instant, edges: Edges) {
        self.wheels += 1;
        let pulled = stretch(self.pull, edges.height.max(1.0));
        self.pull = 0.0;
        if self.refreshing {
            return;
        }
        // With animations off, a long enough pull already refreshed.
        if pulled >= TRIGGER && self.can_refresh() && self.animate {
            self.start(now);
        } else {
            self.settle(now, 0.0, SETTLE, Then::Rest);
        }
    }

    fn start(&mut self, now: Instant) {
        self.refreshing = true;
        self.out.refresh = true;
        self.settle(now, HOLD, SETTLE, Then::Rest);
    }

    /// Springs the list to `to`, then does `then`. With animations off, the list stays
    /// put.
    fn settle(&mut self, now: Instant, to: f64, duration: Duration, then: Then) {
        let from = self.now_at(now);
        self.spring = None;
        if !self.animate || (from - to).abs() < 0.01 {
            self.offset = if self.animate { to } else { 0.0 };
            self.then(now, then);
            return;
        }
        self.springs += 1;
        self.offset = to;
        self.spring = Some(Spring {
            from,
            duration,
            id: self.springs,
            start: now,
            to,
            then,
        });
        self.out.spring = Some((self.springs, duration));
    }

    fn then(&mut self, now: Instant, then: Then) {
        match then {
            Then::Rest => {}
            Then::SpringBack => self.settle(now, 0.0, SETTLE, Then::Rest),
            Then::EndRefresh => self.refreshing = false,
        }
    }

    /// A pull far enough in this gesture runs the refresh.
    fn can_refresh(&self) -> bool {
        self.refreshes && self.from_top
    }

    /// A gesture begins: a touch, or the wheel after a pause or with the fingers back.
    fn began(&mut self, edges: Edges) {
        self.from_top = edges.top;
        self.spent = false;
        self.wheel_size = 0.0;
        self.shrinking = 0;
        self.growing = 0;
        self.coasting = false;
        self.bounced = false;
        self.coasted = 0.0;
    }

    /// A gesture that scrolled the list away from the top can't refresh. Scrolled away
    /// while it refreshes, the list lets go of its hold, which would keep its end out of
    /// sight.
    fn left_top(&mut self, now: Instant, edges: Edges) {
        if edges.top {
            return;
        }
        self.from_top = false;
        if self.refreshing && self.offset != 0.0 {
            self.settle(now, 0.0, SETTLE, Then::Rest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOP: Edges = Edges {
        top: true,
        bottom: false,
        height: 600.0,
    };
    const MIDDLE: Edges = Edges {
        top: false,
        bottom: false,
        height: 600.0,
    };
    const END: Edges = Edges {
        top: false,
        bottom: true,
        height: 600.0,
    };

    fn ms(at: Instant, ms: u64) -> Instant {
        at + Duration::from_millis(ms)
    }

    const GUESSED: (Phase, Source) = (Phase::Moved, Source::Unknown);
    const FINGERS: (Phase, Source) = (Phase::Moved, Source::Finger);
    const MOMENTUM: (Phase, Source) = (Phase::Moved, Source::Momentum);

    /// A finger pulls the list down (up, if negative) `steps` times 10 px.
    fn drag(o: &mut Overscroll, at: Instant, steps: i32, edges: Edges) -> Vec<Out> {
        let _ = o.touch_start(Some(0.0), edges);
        (1..=steps.abs())
            .map(|i| o.touch_move(at, Some(f64::from(i * 10 * steps.signum())), edges))
            .collect()
    }

    #[test]
    fn stretch_and_pull_for_invert() {
        for x in [1.0, 50.0, 300.0, -120.0] {
            assert!((pull_for(stretch(x, 600.0), 600.0) - x).abs() < 1e-6);
        }
    }

    #[test]
    fn a_short_pull_springs_back() {
        let (mut o, at) = (Overscroll::new(true, true), Instant::now());
        let outs = drag(&mut o, at, 6, TOP);
        assert!(outs.iter().all(|out| out.prevent));
        assert!(o.shown().offset > 0.0 && o.shown().indicator());
        let out = o.touch_end(at, 0, TOP);
        assert!(!out.refresh);
        let (id, _) = out.spring.expect("springs back");
        assert_eq!(o.shown().offset, 0.0);
        assert!(o.shown().indicator(), "shows while springing back");
        let _ = o.sprung(ms(at, 400), id);
        assert_eq!((o.shown().offset, o.shown().spring), (0.0, None));
        assert!(!o.shown().indicator());
    }

    #[test]
    fn a_long_pull_refreshes_and_holds() {
        let (mut o, at) = (Overscroll::new(true, true), Instant::now());
        let _ = drag(&mut o, at, 30, TOP);
        assert!(o.shown().armed());
        let out = o.touch_end(at, 0, TOP);
        assert!(out.refresh);
        assert_eq!(o.shown().offset, HOLD);
        let (id, _) = out.spring.unwrap();
        let _ = o.sprung(ms(at, 400), id);
        // Held while it refreshes: a pull doesn't move it.
        assert!(!drag(&mut o, at, 5, TOP).iter().any(|out| out.prevent));
        assert_eq!(o.shown().offset, HOLD);
        let (id, _) = o.refreshed(ms(at, 1000)).spring.unwrap();
        assert!(o.shown().refreshing && o.shown().indicator());
        let _ = o.sprung(ms(at, 1400), id);
        assert_eq!((o.shown().offset, o.shown().refreshing), (0.0, false));
        assert!(!o.shown().indicator());
    }

    #[test]
    fn the_finger_left_after_a_second_lifts_still_pulls() {
        let (mut o, at) = (Overscroll::new(true, true), Instant::now());
        let _ = o.touch_start(Some(0.0), TOP);
        // A second finger lands, then lifts.
        let _ = o.touch_start(None, TOP);
        let _ = o.touch_end(at, 1, TOP);
        let first = o.touch_move(at, Some(0.0), TOP);
        assert_eq!(o.shown().offset, 0.0, "the first move only anchors");
        assert!(!first.refresh);
        let out = o.touch_move(at, Some(40.0), TOP);
        assert!(out.prevent && o.shown().offset > 0.0);
    }

    #[test]
    fn a_pull_from_below_the_top_does_not_refresh() {
        let (mut o, at) = (Overscroll::new(true, true), Instant::now());
        let _ = o.touch_start(Some(0.0), MIDDLE);
        // Scrolled up to the top by the same touch, then pulled on.
        let _ = o.touch_move(at, Some(10.0), MIDDLE);
        for i in 2..40 {
            let _ = o.touch_move(at, Some(f64::from(i) * 10.0), TOP);
        }
        assert!(o.shown().offset > TRIGGER);
        assert!(!o.shown().indicator());
        assert!(!o.touch_end(at, 0, TOP).refresh);
    }

    #[test]
    fn the_end_stretches_without_refreshing() {
        let (mut o, at) = (Overscroll::new(true, true), Instant::now());
        let outs = drag(&mut o, at, -30, END);
        assert!(outs.iter().all(|out| out.prevent));
        assert!(o.shown().offset < 0.0 && !o.shown().indicator());
        assert!(!o.touch_end(at, 0, END).refresh);
    }

    #[test]
    fn a_touch_pulled_back_scrolls_the_list_itself() {
        let (mut o, at) = (Overscroll::new(true, true), Instant::now());
        let _ = drag(&mut o, at, 2, TOP);
        // Back to the edge, and on up: the list scrolls by the touch, not by Blitz.
        let _ = o.touch_move(at, Some(0.0), TOP);
        let out = o.touch_move(at, Some(-15.0), TOP);
        assert_eq!((out.prevent, out.scroll_by), (true, 15.0));
    }

    #[test]
    fn a_mouse_wheel_and_mid_list_scrolls_pass_through() {
        let (mut o, at) = (Overscroll::new(true, true), Instant::now());
        let wheel = (Phase::Moved, Source::Wheel);
        assert_eq!(o.wheel(at, (0.0, 3.0), false, wheel, TOP), Out::default());
        // A mouse that scrolls smoothly, in pixels.
        assert_eq!(o.wheel(at, (0.0, 30.0), false, wheel, TOP), Out::default());
        assert_eq!(
            o.wheel(at, (0.0, 30.0), false, GUESSED, MIDDLE),
            Out::default()
        );
        assert_eq!(o.wheel(at, (0.0, 30.0), true, GUESSED, TOP), Out::default());
    }

    #[test]
    fn a_trackpad_pull_refreshes_once_idle() {
        let (mut o, at) = (Overscroll::new(true, true), Instant::now());
        let mut idle = None;
        for i in 0..40 {
            let out = o.wheel(ms(at, i * 8), (0.0, 20.0), false, GUESSED, TOP);
            assert!(out.prevent);
            idle = out.idle;
        }
        // An older event's timer doesn't let go.
        assert_eq!(o.idle(ms(at, 400), idle.unwrap() - 1, TOP), Out::default());
        let out = o.idle(ms(at, 450), idle.unwrap(), TOP);
        assert!(out.refresh);
    }

    #[test]
    fn momentum_into_an_edge_bounces() {
        let (mut o, at) = (Overscroll::new(true, true), Instant::now());
        // A flick's momentum, shrinking smoothly, coasts up into the top.
        let mut t = 0;
        let mut out = Out::default();
        for size in [50.0, 45.0, 41.0, 37.0, 34.0] {
            t += 8;
            out = o.wheel(
                ms(at, t),
                (0.0, size),
                false,
                GUESSED,
                if t < 40 { MIDDLE } else { TOP },
            );
        }
        assert!(out.prevent);
        assert!(o.shown().offset > 0.0, "bounced out");
        let (id, _) = out.spring.unwrap();
        let (back, _) = o.sprung(ms(at, t + 120), id).spring.expect("springs back");
        let _ = o.sprung(ms(at, t + 520), back);
        assert_eq!(o.shown().offset, 0.0);
    }

    /// Fingers on a trackpad that says when they land and lift pull the list down `steps`
    /// events of 20 px, 8 ms apart from `at`.
    fn finger_pull(o: &mut Overscroll, at: Instant, steps: u64, edges: Edges) -> Vec<Out> {
        let mut outs = vec![o.wheel(
            at,
            (0.0, 0.0),
            false,
            (Phase::Started, Source::Finger),
            edges,
        )];
        for i in 1..=steps {
            outs.push(o.wheel(ms(at, i * 8), (0.0, 20.0), false, FINGERS, edges));
        }
        outs
    }

    #[test]
    fn fingers_that_lift_let_go_at_once() {
        let (mut o, at) = (Overscroll::new(true, true), Instant::now());
        let outs = finger_pull(&mut o, at, 10, TOP);
        assert!(
            outs[1..]
                .iter()
                .all(|out| out.prevent && out.idle.is_none())
        );
        // Held still, the fingers keep the list stretched: no timer lets it go.
        assert!(o.shown().offset > 0.0 && o.shown().spring.is_none());
        let out = o.wheel(
            ms(at, 500),
            (0.0, 0.0),
            false,
            (Phase::Ended, Source::Finger),
            TOP,
        );
        assert!(!out.refresh);
        let (id, _) = out.spring.expect("springs back as the fingers lift");
        // The momentum that follows, growing at first and shrinking unevenly, doesn't
        // stretch the list or scroll it.
        for (i, size) in [24.0, 30.0, 18.0, 19.0, 9.0, 3.0, 2.0, 1.0]
            .into_iter()
            .enumerate()
        {
            let out = o.wheel(
                ms(at, 510 + i as u64 * 8),
                (0.0, size),
                false,
                MOMENTUM,
                TOP,
            );
            assert!(out.prevent && out.spring.is_none(), "momentum event {i}");
            assert_eq!(o.shown().offset, 0.0);
        }
        let _ = o.sprung(ms(at, 900), id);
        assert_eq!((o.shown().offset, o.shown().spring), (0.0, None));
    }

    #[test]
    fn fingers_that_lift_from_a_long_pull_refresh() {
        let (mut o, at) = (Overscroll::new(true, true), Instant::now());
        let _ = finger_pull(&mut o, at, 40, TOP);
        assert!(o.shown().armed());
        let out = o.wheel(
            ms(at, 400),
            (0.0, 0.0),
            false,
            (Phase::Ended, Source::Finger),
            TOP,
        );
        assert!(out.refresh);
        assert_eq!(o.shown().offset, HOLD);
    }

    #[test]
    fn momentum_said_as_such_bounces_off_an_edge() {
        let (mut o, at) = (Overscroll::new(true, true), Instant::now());
        let _ = finger_pull(&mut o, at, 3, MIDDLE);
        let _ = o.wheel(
            ms(at, 40),
            (0.0, 0.0),
            false,
            (Phase::Ended, Source::Finger),
            MIDDLE,
        );
        let out = o.wheel(ms(at, 50), (0.0, 30.0), false, MOMENTUM, MIDDLE);
        assert_eq!(out, Out::default(), "momentum mid-list scrolls it");
        // Its first event at the top bounces, however its sizes ran.
        let out = o.wheel(ms(at, 58), (0.0, 31.0), false, MOMENTUM, TOP);
        assert!(out.prevent && out.spring.is_some() && o.shown().offset > 0.0);
        let again = o.wheel(ms(at, 66), (0.0, 20.0), false, MOMENTUM, TOP);
        assert!(again.prevent && again.spring.is_none(), "bounces once");
    }

    #[test]
    fn scrolled_while_it_refreshes_the_list_lets_go_of_its_hold() {
        let (mut o, at) = (Overscroll::new(true, true), Instant::now());
        let _ = drag(&mut o, at, 40, TOP);
        assert!(o.touch_end(at, 0, TOP).refresh);
        assert_eq!(o.shown().offset, HOLD);
        // Still at the top, the list stays held.
        let out = o.wheel(ms(at, 500), (0.0, -20.0), false, FINGERS, TOP);
        assert!(!out.prevent && out.spring.is_none());
        assert_eq!(o.shown().offset, HOLD);
        // Scrolled down, it springs back, and goes on refreshing.
        let out = o.wheel(ms(at, 510), (0.0, -20.0), false, FINGERS, MIDDLE);
        assert!(!out.prevent);
        let (id, _) = out.spring.expect("springs back");
        assert_eq!(o.shown().offset, 0.0);
        let _ = o.sprung(ms(at, 910), id);
        assert!(o.shown().refreshing);
        let _ = o.refreshed(ms(at, 1000));
        assert!(!o.shown().refreshing);
    }

    #[test]
    fn without_a_refresh_the_top_only_stretches() {
        let (mut o, at) = (Overscroll::new(true, false), Instant::now());
        let outs = drag(&mut o, at, 30, TOP);
        assert!(outs.iter().all(|out| out.prevent));
        assert!(o.shown().offset > TRIGGER && !o.shown().indicator());
        let out = o.touch_end(at, 0, TOP);
        assert!(!out.refresh && out.spring.is_some());
    }

    #[test]
    fn without_animations_a_long_pull_refreshes_at_once() {
        let (mut o, at) = (Overscroll::new(false, true), Instant::now());
        let outs = drag(&mut o, at, 40, TOP);
        assert_eq!(outs.iter().filter(|out| out.refresh).count(), 1);
        assert_eq!(o.shown().offset, 0.0);
        assert!(!o.touch_end(at, 0, TOP).refresh);
        assert!(o.shown().refreshing);
        let _ = o.refreshed(at);
        assert!(!o.shown().refreshing);
    }
}
