//! Lets one sync of an account run at a time.
//!
//! A second sync asked for while one runs, such as a scheduled one during a manual one,
//! doesn't queue: it reports [`Outcome::Busy`] and the caller decides what to do.

// Rust guideline compliant 2026-10-09

use tokio::sync::Mutex;

/// Runs one piece of work at a time; others asked for meanwhile don't start.
#[derive(Debug, Default)]
pub(crate) struct Gate {
    running: Mutex<()>,
}

/// How a run through the [`Gate`] went.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome<T> {
    /// Another run was going, so this one didn't start.
    Busy,
    /// The work ran and gave this.
    Done(T),
}

impl Gate {
    /// Whether work is running now.
    pub(crate) fn is_running(&self) -> bool {
        self.running.try_lock().is_err()
    }

    /// Runs `work` unless other work is running already.
    pub(crate) async fn run<T>(&self, work: impl Future<Output = T>) -> Outcome<T> {
        let Ok(_running) = self.running.try_lock() else {
            return Outcome::Busy;
        };
        Outcome::Done(work.await)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn only_one_run_goes_at_a_time() {
        let gate = Gate::default();
        let inner = gate.run(async { gate.run(async { 1 }).await }).await;
        assert_eq!(inner, Outcome::Done(Outcome::Busy));
        assert!(!gate.is_running());
        assert_eq!(gate.run(async { 2 }).await, Outcome::Done(2));
    }
}
