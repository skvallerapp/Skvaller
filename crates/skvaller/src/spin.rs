//! When the sync icon turns: while a sync runs, and after it until a whole turn ends,
//! one at least from when it began or was asked for, so a quick sync still shows and the
//! icon never stops askew. With Settings → Animations off it stops as the sync ends.
//!
//! Plain state, without a document: the sidebar feeds it the sync status and the syncs
//! asked for, and stops the icon when it says.

use std::time::{Duration, Instant};

/// One turn of the sync icon: app.css's `spin` on `.sync.spinning`.
const SPIN_TURN: Duration = Duration::from_millis(900);

/// The sync icon's state.
#[derive(Debug)]
pub struct Spin {
    /// When the icon began turning; `None` while it stands still.
    began: Option<Instant>,
    /// The syncs asked for, as last seen.
    asked: u64,
    /// Counts the changes followed, so a stop planned before a sync started again
    /// doesn't stop it.
    change: u64,
}

/// A stop [`Spin::follow`] planned: after `wait`, unless something changed meanwhile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stop {
    pub wait: Duration,
    change: u64,
}

impl Spin {
    /// Standing still, with `asked` syncs asked for so far.
    pub fn new(asked: u64) -> Self {
        Self {
            began: None,
            asked,
            change: 0,
        }
    }

    pub fn turning(&self) -> bool {
        self.began.is_some()
    }

    /// Follows a change of whether the shown account syncs, or of the syncs asked for
    /// here. A sync asked for may already have ended, its running status unseen: it turns
    /// the icon too. Returns when to stop, unless the sync runs.
    pub fn follow(
        &mut self,
        syncing: bool,
        asked: u64,
        now: Instant,
        animate: bool,
    ) -> Option<Stop> {
        self.change += 1;
        let new_ask = std::mem::replace(&mut self.asked, asked) != asked;
        if (syncing || new_ask) && self.began.is_none() {
            self.began = Some(now);
        }
        let began = self.began.filter(|_| !syncing)?;
        let wait = if animate {
            rest_of_turn(now.saturating_duration_since(began))
        } else {
            Duration::ZERO
        };
        Some(Stop {
            wait,
            change: self.change,
        })
    }

    /// Stops the icon as `stop` planned, unless something changed since. Returns whether
    /// it stopped.
    pub fn stop(&mut self, stop: Stop) -> bool {
        if stop.change != self.change || self.began.is_none() {
            return false;
        }
        self.began = None;
        true
    }
}

/// How much longer a spin that began `elapsed` ago turns to end a whole [`SPIN_TURN`],
/// one at least.
fn rest_of_turn(elapsed: Duration) -> Duration {
    let turn = SPIN_TURN.as_nanos();
    let turns = elapsed.as_nanos().div_ceil(turn).max(1);
    let rest = (turns * turn).saturating_sub(elapsed.as_nanos());
    Duration::from_nanos(u64::try_from(rest).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    #[test]
    fn a_spin_ends_on_a_whole_turn() {
        assert_eq!(rest_of_turn(ms(0)), ms(900));
        assert_eq!(rest_of_turn(ms(200)), ms(700));
        assert_eq!(rest_of_turn(ms(900)), ms(0));
        assert_eq!(rest_of_turn(ms(1000)), ms(800));
    }

    #[test]
    fn a_sync_that_ends_at_once_still_turns_once() {
        let t = Instant::now();
        let mut spin = Spin::new(0);
        // Asked for, its running status never seen.
        let stop = spin.follow(false, 1, t, true).unwrap();
        assert!(spin.turning());
        assert_eq!(stop.wait, SPIN_TURN);
        assert!(spin.stop(stop));
        assert!(!spin.turning());
    }

    #[test]
    fn a_sync_asked_for_while_one_runs_turns_until_that_one_ends() {
        let t = Instant::now();
        let mut spin = Spin::new(0);
        assert_eq!(spin.follow(true, 0, t, true), None);
        // Busy: the running one goes on.
        assert_eq!(spin.follow(true, 1, t + ms(300), true), None);
        assert!(spin.turning());
        let stop = spin.follow(false, 1, t + ms(1200), true).unwrap();
        assert_eq!(stop.wait, ms(600), "from when the running one began");
        assert!(spin.stop(stop));
    }

    #[test]
    fn a_second_sync_before_the_icon_stops_keeps_it_turning() {
        let t = Instant::now();
        let mut spin = Spin::new(0);
        spin.follow(true, 0, t, true);
        let first = spin.follow(false, 0, t + ms(100), true).unwrap();
        spin.follow(true, 0, t + ms(500), true);
        assert!(!spin.stop(first), "the first's stop is stale");
        assert!(spin.turning());
        let second = spin.follow(false, 0, t + ms(1000), true).unwrap();
        assert_eq!(second.wait, ms(800), "on the turns the first began");
        assert!(spin.stop(second));
        // Standing still, a status without a sync plans nothing.
        assert_eq!(spin.follow(false, 0, t + ms(2000), true), None);
    }

    #[test]
    fn without_animations_the_icon_stops_as_the_sync_ends() {
        let t = Instant::now();
        let mut spin = Spin::new(3);
        spin.follow(true, 3, t, false);
        let stop = spin.follow(false, 3, t + ms(100), false).unwrap();
        assert_eq!(stop.wait, Duration::ZERO);
    }
}
