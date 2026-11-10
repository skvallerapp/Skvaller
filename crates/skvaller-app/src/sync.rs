//! The background sync of each account, on its schedule.
//!
//! Each account syncs on start and then as its [`SyncSchedule`] says, one sync at a
//! time, and after the computer wakes when the account asks for that or the sleep held
//! its schedule up. Every change of status is sent to [`App::subscribe`] listeners; a
//! status with `changed` set means the cached articles changed and lists need loading
//! again.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use skvaller_core::{
    Client, Error as CoreError, load_token, prefetch_full_text, refresh_local, sync,
};
use tokio::time::{Instant, sleep_until};
use tracing::Level;

use crate::app::{App, Runtime, lock};
use crate::config::{Account, AccountKind, SyncSchedule};
use crate::gate::Outcome;

/// An account's sync, as the UI shows it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts about one sync"
)]
pub struct SyncStatus {
    /// The account it is about.
    pub account_id: String,
    /// A sync is running.
    pub syncing: bool,
    /// When the last sync finished, in Unix milliseconds.
    pub last_sync: Option<i64>,
    /// Why the last sync failed.
    pub error: Option<String>,
    /// The server rejected the token: the user has to sign in again.
    pub unauthorized: bool,
    /// The sync that just finished changed the cached articles or their feeds.
    pub changed: bool,
    /// The cache is damaged.
    pub cache_broken: bool,
}

/// How a sync went, for the schedule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Ran {
    Synced,
    /// `retry`: worth trying again soon, as anything but a rejected sign-in or a damaged
    /// cache may be the network.
    Failed {
        retry: bool,
    },
    /// Another sync of the account was running.
    Busy,
    /// Signed out or removed.
    Skipped,
}

impl App {
    /// Starts every account's background sync on the current Tokio runtime.
    ///
    /// # Panics
    ///
    /// When called outside a Tokio runtime.
    pub fn start_sync(self: &Arc<Self>) {
        let _ = self.sync_runtime.set(tokio::runtime::Handle::current());
        for rt in self.runtimes() {
            self.start_loop(rt);
        }
        let app = Arc::clone(self);
        tokio::spawn(async move { app.watch_for_wake().await });
    }

    /// Starts the account's background sync, on the sync runtime. Before
    /// [`App::start_sync`] there is none, and nothing starts: that starts them all.
    pub(crate) fn start_loop(self: &Arc<Self>, rt: Arc<Runtime>) {
        if let Some(runtime) = self.sync_runtime.get() {
            let app = Arc::clone(self);
            runtime.spawn(async move { app.sync_loop(&rt).await });
        }
    }

    /// Syncs the account on the sync runtime, after any sync running now: that one
    /// started before a change it would miss, such as new feeds or a new token.
    pub(crate) fn sync_soon(self: &Arc<Self>, rt: Arc<Runtime>) {
        if let Some(runtime) = self.sync_runtime.get() {
            let app = Arc::clone(self);
            runtime.spawn(async move {
                drop(rt.gate.stop().await);
                app.run_sync(&rt).await
            });
        }
    }

    /// Tells every account's background sync when the computer wakes, as no system
    /// event says so on every platform. A timer doesn't run while the computer sleeps;
    /// the wall clock does.
    async fn watch_for_wake(&self) {
        let mut before = SystemTime::now();
        loop {
            tokio::time::sleep(WAKE_CHECK).await;
            let now = SystemTime::now();
            if slept(before, now, WAKE_CHECK) {
                for rt in self.runtimes() {
                    rt.woke.store(true, Ordering::Relaxed);
                    rt.wake.notify_one();
                }
            }
            before = now;
        }
    }

    /// Starts a sync of the account now, unless it is syncing already, on the sync
    /// runtime: parsing and storing a sync's feeds would hold up the UI's thread.
    ///
    /// # Errors
    ///
    /// [`crate::Error::NoAccount`] or [`crate::Error::Broken`] for an account without a
    /// cache. A failed sync is not an error here: its status says why.
    ///
    /// # Panics
    ///
    /// When [`App::start_sync`] hasn't run.
    pub fn sync_now(self: &Arc<Self>, account_id: &str) -> crate::Result<()> {
        let rt = self.runtime(account_id)?;
        let app = Arc::clone(self);
        self.sync_runtime
            .get()
            .expect("start_sync runs before any sync")
            .spawn(async move { app.run_sync(&rt).await });
        Ok(())
    }

    /// On start, then every so many minutes after the last automatic sync, by the
    /// wall clock so a sleep doesn't push it back, and after waking as [`after_wake`]
    /// says. Ends when the account is gone.
    async fn sync_loop(&self, rt: &Runtime) {
        let mut history = History::default();
        while let Some(account) = self.account(&rt.id) {
            if rt.woke.swap(false, Ordering::Relaxed) {
                history.wake_sync = after_wake(&account, &history, SystemTime::now());
            }
            // Sync on wake turned off meanwhile: the sync after waking it asked for stops.
            if !account.sync_on_wake {
                history.wake_sync = history.wake_sync.filter(|w| !w.asked);
            }
            let wait = async {
                match due(account.sync, &history, SystemTime::now()) {
                    Some(due) => sleep_until(Instant::now() + due).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                () = wait => {
                    let ran = self.run_sync(rt).await;
                    // Another sync running: a sync after waking waits for its turn.
                    if ran == Ran::Busy && history.wake_sync.is_some() {
                        continue;
                    }
                    history.ran(ran, SystemTime::now());
                }
                () = rt.wake.notified() => {}
            }
        }
    }

    /// One sync of the account, reported to the listeners.
    async fn run_sync(&self, rt: &Runtime) -> Ran {
        let work = async {
            let account = self.account(&rt.id)?;
            // `None` for the local account, which fetches its feeds itself.
            let client = match account.kind {
                AccountKind::Greader { .. } => match self.client(rt).await {
                    Ok(Some(client)) => Some(client),
                    Ok(None) => return None,
                    Err(e) => return Some(self.finish(rt, &Err(e), false)),
                },
                AccountKind::Local => None,
            };
            self.emit(SyncStatus {
                syncing: true,
                ..rt.status()
            });
            let (now, keep_read) = (now_secs(), account.keep_read_days.max(1));
            let synced = if let Some(client) = &client {
                sync(client, &rt.db, now, keep_read).await
            } else {
                let keep_unread = Some(account.keep_unread_days).filter(|&d| d > 0);
                refresh_local(&rt.db, now, keep_read, keep_unread).await
            };
            if synced.is_ok() && client.is_none() {
                self.back_up_feeds(rt);
            }
            let changed = synced
                .as_ref()
                .is_ok_and(skvaller_core::SyncReport::changed_articles);
            Some(self.finish(rt, &synced.map(drop), changed))
        };
        match rt.gate.run(work).await {
            Outcome::Done(Some(ran)) => {
                if ran == Ran::Synced {
                    self.prefetch(rt);
                }
                ran
            }
            Outcome::Done(None) => Ran::Skipped,
            Outcome::Busy => Ran::Busy,
        }
    }

    /// Records how the sync went, logs a new error once, and tells the listeners.
    fn finish(&self, rt: &Runtime, result: &Result<(), CoreError>, changed: bool) -> Ran {
        let error = result.as_ref().err().map(ToString::to_string);
        if let Err(e) = result {
            if e.is_corrupt() {
                rt.corrupt.store(true, Ordering::Relaxed);
            }
            if matches!(e, CoreError::Unauthorized) {
                rt.token_rejected.store(true, Ordering::Relaxed);
                *lock(&rt.client) = None;
            }
            // Once per error, not on every try while offline.
            if lock(&rt.last_error).as_ref() != error.as_ref() {
                tracing::event!(
                    name: "sync.run.failure",
                    Level::WARN,
                    account.id = %rt.id,
                    error.message = %e,
                    "syncing account {{account.id}} failed: {{error.message}}",
                );
            }
        }
        lock(&rt.last_error).clone_from(&error);
        // Sent from inside the gate, which `status` reads as still syncing.
        self.emit(SyncStatus {
            syncing: false,
            changed,
            ..rt.status()
        });
        match result {
            Ok(()) => Ran::Synced,
            Err(e) => Ran::Failed {
                retry: !matches!(e, CoreError::Unauthorized) && !e.is_corrupt(),
            },
        }
    }

    fn emit(&self, status: SyncStatus) {
        // No listener yet is fine: a window that opens later reads the status itself.
        let _ = self.events.send(status);
    }

    /// The account's API client, built from the keychain token on first use. `None`
    /// without a token, or after the server rejected it.
    async fn client(&self, rt: &Runtime) -> Result<Option<Client>, CoreError> {
        if let Some(client) = lock(&rt.client).clone() {
            return Ok(Some(client));
        }
        if rt.token_rejected.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let Some(account) = self.account(&rt.id) else {
            return Ok(None);
        };
        let AccountKind::Greader { server, .. } = &account.kind else {
            return Ok(None);
        };
        let key = account
            .kind
            .keychain_key()
            .expect("a sync account has a keychain entry");
        // The keychain blocks, and may wait for the user to unlock it.
        let token = tokio::task::spawn_blocking(move || load_token(&key))
            .await
            .expect("reading the keychain doesn't panic")?;
        let Some(token) = token else { return Ok(None) };
        let client = Client::new(server, &token)?;
        *lock(&rt.client) = Some(client.clone());
        Ok(Some(client))
    }
}

impl App {
    /// Fetches Reader View pages of new articles in the background, which can take
    /// minutes. Skipped while the last sync's prefetch still runs; removing the account
    /// aborts it.
    fn prefetch(&self, rt: &Runtime) {
        let mut running = lock(&rt.prefetch);
        // Removed meanwhile: its removal takes this lock only after taking it from
        // the runtimes, so a prefetch started here is one it aborts.
        if self.runtime(&rt.id).is_err() || running.as_ref().is_some_and(|task| !task.is_finished())
        {
            return;
        }
        let (db, id) = (Arc::clone(&rt.db), rt.id.clone());
        let task = tokio::spawn(async move {
            if let Err(e) = prefetch_full_text(&db).await {
                tracing::event!(
                    name: "full_text.prefetch.failure",
                    Level::WARN,
                    account.id = %id,
                    error.message = %e,
                    "prefetching Reader View for account {{account.id}} failed: {{error.message}}",
                );
            }
        });
        *running = Some(task.abort_handle());
    }
}

fn now_secs() -> i64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
}

/// How often the clock is checked for a sleep.
const WAKE_CHECK: Duration = Duration::from_secs(30);
/// More time than this past a check's wait means the computer slept. Large, so a
/// stalled process, as under App Nap, or a clock set forward isn't taken for a sleep;
/// a sleep shorter than this needs no sync anyway.
const WAKE_SLACK: Duration = Duration::from_secs(5 * 60);
/// The network takes a moment to come back after a wake.
const WAKE_DELAY: Duration = Duration::from_secs(15);
/// Retries of a sync after waking that failed, likely before the network was back.
const WAKE_RETRIES: u32 = 2;

/// Whether the wall clock moved on so much more than a wait of `waited` that the
/// computer must have slept.
fn slept(before: SystemTime, now: SystemTime, waited: Duration) -> bool {
    now.duration_since(before)
        .is_ok_and(|passed| passed > waited + WAKE_SLACK)
}

/// What a background sync remembers of its account's syncs.
#[derive(Clone, Copy, Debug, Default)]
struct History {
    /// The last automatic sync, and the last that worked.
    last: Option<SystemTime>,
    last_ok: Option<SystemTime>,
    /// A sync after waking is due.
    wake_sync: Option<WakeSync>,
}

/// A sync after waking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WakeSync {
    /// Retries left if it fails.
    retries: u32,
    /// Only the account's sync on wake asked for it, not a schedule the sleep held up.
    asked: bool,
}

impl History {
    fn ran(&mut self, ran: Ran, now: SystemTime) {
        self.last = Some(now);
        self.wake_sync = match ran {
            Ran::Synced => {
                self.last_ok = Some(now);
                None
            }
            // Likely the network, not back yet after waking: again in a moment.
            Ran::Failed { retry: true } => self.wake_sync.and_then(|w| {
                Some(WakeSync {
                    retries: w.retries.checked_sub(1)?,
                    ..w
                })
            }),
            Ran::Failed { retry: false } | Ran::Busy | Ran::Skipped => None,
        };
    }
}

/// The sync to make after waking, if any: when the account's sync on wake asks for it
/// and none worked lately, or when the sleep held up the schedule, or its own sync ran
/// on waking and failed, before the network was back.
fn after_wake(account: &Account, history: &History, now: SystemTime) -> Option<WakeSync> {
    // A time ahead of now, after the clock was set back, counts as long ago.
    let ago = |t: SystemTime| now.duration_since(t).unwrap_or(Duration::MAX);
    let overdue = match (account.sync, history.last) {
        (SyncSchedule::Every(minutes), Some(last)) => {
            ago(last) >= Duration::from_secs(u64::from(minutes) * 60)
        }
        _ => false,
    };
    let synced_lately = history.last_ok.is_some_and(|t| ago(t) < WAKE_SLACK);
    let failed_lately =
        history.last.is_some_and(|t| ago(t) < WAKE_SLACK) && history.last != history.last_ok;
    let asked = account.sync_on_wake && !synced_lately;
    let held_up = overdue || failed_lately;
    (asked || held_up).then_some(WakeSync {
        retries: WAKE_RETRIES,
        asked: !held_up,
    })
}

/// How long until the next sync, or `None` when none is due until something changes.
fn due(schedule: SyncSchedule, history: &History, now: SystemTime) -> Option<Duration> {
    if history.wake_sync.is_some() {
        return Some(WAKE_DELAY);
    }
    match (schedule, history.last) {
        (SyncSchedule::Manual, _) | (SyncSchedule::OnStart, Some(_)) => None,
        (_, None) => Some(Duration::ZERO),
        (SyncSchedule::Every(minutes), Some(last)) => {
            let since = now.duration_since(last).unwrap_or_default();
            Some(Duration::from_secs(u64::from(minutes) * 60).saturating_sub(since))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINUTE: Duration = Duration::from_secs(60);

    fn account(sync: SyncSchedule, sync_on_wake: bool) -> Account {
        Account {
            id: "1".into(),
            description: String::new(),
            kind: AccountKind::Local,
            sync,
            keep_read_days: 30,
            keep_unread_days: 180,
            sync_on_wake,
        }
    }

    fn history(last: Option<Duration>, last_ok: Option<Duration>, now: SystemTime) -> History {
        History {
            last: last.map(|ago| now - ago),
            last_ok: last_ok.map(|ago| now - ago),
            wake_sync: None,
        }
    }

    #[test]
    fn only_a_gap_well_past_the_wait_is_a_sleep() {
        let before = UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert!(!slept(before, before + WAKE_CHECK + MINUTE, WAKE_CHECK));
        assert!(slept(before, before + WAKE_CHECK + 6 * MINUTE, WAKE_CHECK));
        // A clock set back is not a sleep.
        assert!(!slept(before, before - MINUTE, WAKE_CHECK));
    }

    #[test]
    fn waking_syncs_when_asked_or_when_the_schedule_fell_behind() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let hour_ago = Some(60 * MINUTE);
        let wake = |a: &Account, h: &History| after_wake(a, h, now);

        let asked = account(SyncSchedule::Manual, true);
        assert_eq!(
            wake(&asked, &history(hour_ago, hour_ago, now)),
            Some(WakeSync {
                retries: WAKE_RETRIES,
                asked: true
            })
        );
        // Synced a moment ago: no need.
        let moment = Some(MINUTE);
        assert_eq!(wake(&asked, &history(moment, moment, now)), None);

        let every_15 = account(SyncSchedule::Every(15), false);
        // Held up by the sleep: not the sync on wake's, so turning that off keeps it.
        assert!(wake(&every_15, &history(hour_ago, hour_ago, now)).is_some_and(|w| !w.asked));
        assert_eq!(wake(&every_15, &history(moment, moment, now)), None);
        // The schedule's own sync ran on waking and failed.
        assert!(wake(&every_15, &history(moment, hour_ago, now)).is_some());

        assert_eq!(
            wake(
                &account(SyncSchedule::Manual, false),
                &history(hour_ago, hour_ago, now)
            ),
            None
        );

        // A sync "in the future", from before the clock was set back, isn't recent.
        let ahead = History {
            last: Some(now + 60 * MINUTE),
            last_ok: Some(now + 60 * MINUTE),
            wake_sync: None,
        };
        assert!(wake(&asked, &ahead).is_some());
    }

    #[test]
    fn a_failed_sync_after_waking_retries_twice_then_gives_up() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mut h = History {
            wake_sync: Some(WakeSync {
                retries: WAKE_RETRIES,
                asked: true,
            }),
            ..History::default()
        };
        assert_eq!(due(SyncSchedule::Manual, &h, now), Some(WAKE_DELAY));
        h.ran(Ran::Failed { retry: true }, now);
        h.ran(Ran::Failed { retry: true }, now);
        assert!(h.wake_sync.is_some());
        h.ran(Ran::Failed { retry: true }, now);
        assert_eq!(h.wake_sync, None);
        assert_eq!(due(SyncSchedule::Manual, &h, now), None);
    }

    #[test]
    fn the_schedule_counts_from_the_last_automatic_sync() {
        let now = UNIX_EPOCH + Duration::from_secs(1_000_000);
        let fresh = History::default();
        assert_eq!(
            due(SyncSchedule::OnStart, &fresh, now),
            Some(Duration::ZERO)
        );
        let ten_ago = history(Some(10 * MINUTE), Some(10 * MINUTE), now);
        assert_eq!(
            due(SyncSchedule::Every(15), &ten_ago, now),
            Some(5 * MINUTE)
        );
        assert_eq!(due(SyncSchedule::OnStart, &ten_ago, now), None);
    }
}
