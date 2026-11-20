//! Pulling the lists past their ends: the rubber band and pull-to-refresh.

use super::*;

/// The CSS transition that moves a springing list and its refresh icon; empty while it
/// doesn't spring.
pub(super) fn spring_style(pulled: Shown) -> String {
    pulled.spring.map_or(String::new(), |s| {
        let ms = s.duration.as_millis();
        format!("transition: transform {ms}ms {SPRING_EASE}, opacity {ms}ms {SPRING_EASE};")
    })
}

/// A list's style as it is stretched.
pub(super) fn stretch_style(pulled: Shown) -> String {
    if pulled.offset == 0.0 && pulled.spring.is_none() {
        String::new()
    } else {
        format!(
            "transform: translateY({}px); {}",
            pulled.offset,
            spring_style(pulled)
        )
    }
}

/// Where a scrolling list is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Scrolled {
    /// Where it is scrolled.
    pub(super) top: f64,
    /// The furthest it scrolls.
    pub(super) end: f64,
    /// How tall it shows.
    pub(super) height: f64,
}

/// What pulling the article list acts on besides the list: its rows follow its scroll,
/// and a pull refreshes.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct ListPull {
    /// Where the list is scrolled, which its rows are drawn from.
    pub(super) top: Signal<f64>,
    pub(super) app: CopyValue<Arc<App>>,
    pub(super) reading: Reading,
}

/// A list's [`Overscroll`], with what it acts on.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct Pull {
    pub(super) state: CopyValue<Overscroll>,
    /// What the list draws of it.
    pub(super) shown: Signal<Shown>,
    /// Where the scrolling element is, as last seen.
    pub(super) at: CopyValue<Scrolled>,
    /// The scrolling element.
    pub(super) mounted: Signal<Option<Rc<MountedData>>>,
    pub(super) live: Live,
    /// The article list's, which refreshes; `None` for the sidebar's feeds.
    pub(super) list: Option<ListPull>,
}

impl Pull {
    pub(super) fn new(
        live: &Live,
        mounted: Signal<Option<Rc<MountedData>>>,
        list: Option<&ListPull>,
    ) -> Self {
        let animate = live.settings.peek().animations;
        Self {
            state: CopyValue::new(Overscroll::new(animate, list.is_some())),
            shown: Signal::new(Shown::default()),
            // Until measured, at the top of a long list.
            at: CopyValue::new(Scrolled {
                top: 0.0,
                end: f64::INFINITY,
                height: 600.0,
            }),
            mounted,
            live: *live,
            list: list.copied(),
        }
    }

    /// Which ends the list is at, as last seen, and how tall it shows.
    pub(super) fn edges(self) -> Edges {
        let at = *self.at.peek();
        Edges {
            top: at.top < 1.0,
            bottom: at.top >= at.end - END_SLACK,
            height: at.height,
        }
    }

    /// Reads where the scrolling element is, for a list that doesn't know it itself.
    pub(super) async fn measure(mut self) {
        let Some(el) = self.mounted.peek().clone() else {
            return;
        };
        if let (Ok(view), Ok(offset), Ok(range)) = (
            el.get_client_rect().await,
            el.get_scroll_offset().await,
            el.get_scroll_size().await,
        ) && view.height() > 0.0
        {
            self.at.set(Scrolled {
                top: offset.y,
                end: range.height,
                height: view.height(),
            });
        }
    }

    pub(super) fn on_wheel(self, e: &Event<WheelData>) {
        let (pixels, (dx, dy)) = match e.delta() {
            WheelDelta::Pixels(d) => (true, (d.x, d.y)),
            WheelDelta::Lines(d) => (false, (d.x, d.y)),
            WheelDelta::Pages(d) => (false, (d.x, d.y)),
        };
        // Dioxus's wheel event has no phase or source: Blitz's has.
        let (phase, source) = e
            .data()
            .downcast::<NativeWheelData>()
            .map_or((BlitzWheelPhase::Moved, BlitzScrollSource::Unknown), |d| {
                (d.phase(), d.source())
            });
        // A wheel that turns in lines is a wheel, whatever the platform says.
        let source = if pixels {
            source
        } else {
            BlitzScrollSource::Wheel
        };
        let zoom = e.modifiers().ctrl();
        if self.run(|o, now, edges| o.wheel(now, (dx, dy), zoom, (phase, source), edges)) {
            e.prevent_default();
        }
    }

    pub(super) fn on_touch_start(self, e: &Event<TouchData>) {
        let y = only_touch(e);
        self.run(|o, _, edges| o.touch_start(y, edges));
    }

    pub(super) fn on_touch_move(self, e: &Event<TouchData>) {
        let y = only_touch(e);
        if self.run(|o, now, edges| o.touch_move(now, y, edges)) {
            e.prevent_default();
        }
    }

    pub(super) fn on_touch_end(self, e: &Event<TouchData>) {
        let left = e.touches().len();
        self.run(|o, now, edges| o.touch_end(now, left, edges));
    }

    pub(super) fn on_touch_cancel(self, e: &Event<TouchData>) {
        let left = e.touches().len();
        self.run(|o, now, edges| o.touch_cancel(now, left, edges));
    }

    /// Feeds an event to the overscroll with `f` and does what it says. Returns whether
    /// to keep the event from scrolling the list.
    pub(super) fn run(mut self, f: impl FnOnce(&mut Overscroll, Instant, Edges) -> Out) -> bool {
        let edges = self.edges();
        let (out, shown) = {
            let mut state = self.state.write();
            state.animate = self.live.settings.peek().animations;
            let out = f(&mut state, Instant::now(), edges);
            (out, state.shown())
        };
        if *self.shown.peek() != shown {
            self.shown.set(shown);
        }
        self.act(out)
    }

    /// Does what the overscroll said, apart from drawing it: not generic, as the tasks
    /// it starts feed it again.
    pub(super) fn act(mut self, out: Out) -> bool {
        if let Some(wheel) = out.idle {
            spawn(async move {
                tokio::time::sleep(WHEEL_IDLE).await;
                self.run(|o, now, edges| o.idle(now, wheel, edges));
            });
        }
        if let Some((id, duration)) = out.spring {
            spawn(async move {
                tokio::time::sleep(duration).await;
                self.run(|o, now, _| o.sprung(now, id));
            });
        }
        if out.refresh
            && let Some(list) = self.list
        {
            spawn(async move {
                list.refresh().await;
                self.run(|o, now, _| o.refreshed(now));
            });
        }
        if out.scroll_by != 0.0
            && let Some(el) = self.mounted.peek().clone()
        {
            // Code-driven scrolls send no scroll event: the list follows here.
            let at = *self.at.peek();
            let to = (at.top + out.scroll_by).clamp(0.0, at.end);
            self.at.write().top = to;
            if let Some(mut list) = self.list {
                list.top.set(to);
            }
            spawn(async move {
                let to = dioxus_html::geometry::PixelsVector2D::new(0.0, to);
                if let Err(e) = el.scroll(to, ScrollBehavior::Instant).await {
                    tracing::warn!("scrolling the list failed: {e}");
                }
            });
        }
        out.prevent
    }
}

impl ListPull {
    /// Syncs the shown account and waits until no sync of it runs: one that was already
    /// running ends the wait when it does.
    pub(super) async fn refresh(self) {
        let app = self.app.peek().clone();
        if let Some(synced) = self.reading.sync(&app) {
            synced.await;
        }
    }
}

/// Where a touch is, if it's the only one.
pub(super) fn only_touch(e: &Event<TouchData>) -> Option<f64> {
    match e.touches().as_slice() {
        [touch] => Some(touch.client_coordinates().y),
        _ => None,
    }
}
