//! Lets one sync run at a time, and lets signing in or out stop it instead of waiting
//! for it, which could take as long as the network does.

use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio::sync::{Mutex, MutexGuard, Notify};

#[derive(Default)]
pub struct SyncGate {
    running: Mutex<()>,
    stop: Notify,
    stopping: AtomicBool,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome<T> {
    /// Another sync was running, so this one didn't start.
    Busy,
    /// `stop` ended it before it finished.
    Stopped,
    Done(T),
}

impl SyncGate {
    pub fn is_running(&self) -> bool {
        self.running.try_lock().is_err()
    }

    /// Runs `work` unless a sync is running already. `stop` drops it at its next await.
    pub async fn run<T>(&self, work: impl Future<Output = T>) -> Outcome<T> {
        let Ok(_running) = self.running.try_lock() else {
            return Outcome::Busy;
        };
        // Listen before checking the flag, so a stop between the two is not missed.
        let stopped = self.stop.notified();
        tokio::pin!(stopped);
        stopped.as_mut().enable();
        if self.stopping.load(Ordering::SeqCst) {
            return Outcome::Stopped;
        }
        tokio::select! {
            result = work => Outcome::Done(result),
            () = stopped => Outcome::Stopped,
        }
    }

    /// Stops a running sync and keeps others from starting while the guard lives.
    pub async fn stop(&self) -> MutexGuard<'_, ()> {
        self.stopping.store(true, Ordering::SeqCst);
        self.stop.notify_waiters();
        let guard = self.running.lock().await;
        self.stopping.store(false, Ordering::SeqCst);
        guard
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn stop_ends_a_running_sync_without_waiting_for_it() {
        let gate = Arc::new(SyncGate::default());
        let sync = tokio::spawn({
            let gate = gate.clone();
            async move { gate.run(std::future::pending::<()>()).await }
        });
        while !gate.is_running() {
            tokio::task::yield_now().await;
        }

        let guard = tokio::time::timeout(Duration::from_secs(1), gate.stop())
            .await
            .expect("stop waited for the sync");

        assert!(gate.is_running(), "the guard keeps others out");
        drop(guard);
        assert_eq!(sync.await.unwrap(), Outcome::Stopped);
    }

    #[tokio::test]
    async fn a_stop_while_idle_does_not_stop_the_next_sync() {
        let gate = SyncGate::default();
        drop(gate.stop().await);

        assert_eq!(gate.run(async { 1 }).await, Outcome::Done(1));
    }

    #[tokio::test]
    async fn only_one_sync_runs_at_a_time() {
        let gate = SyncGate::default();
        let _guard = gate.stop().await;

        assert_eq!(gate.run(async { 1 }).await, Outcome::Busy);
    }
}
