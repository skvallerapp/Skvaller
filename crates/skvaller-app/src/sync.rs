//! The background sync of each account, on its schedule.
//!
//! Each account syncs on start and then as its [`SyncSchedule`] says, one sync at a
//! time. Every change of status is sent to [`App::subscribe`] listeners; a status with
//! `changed` set means the cached articles changed and lists need loading again.

// Rust guideline compliant 2026-10-09

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use skvaller_core::{Client, Error as CoreError, load_token, refresh_local, sync};
use tokio::time::{Instant, sleep_until};
use tracing::Level;

use crate::app::{App, Runtime, lock};
use crate::config::{AccountKind, SyncSchedule};
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
    Failed,
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
        for rt in self.runtimes.values() {
            let (app, rt) = (Arc::clone(self), Arc::clone(rt));
            tokio::spawn(async move { app.sync_loop(&rt).await });
        }
    }

    /// Syncs the account now, unless it is syncing already.
    ///
    /// # Errors
    ///
    /// [`crate::Error::NoAccount`] or [`crate::Error::Broken`] for an account without a
    /// cache. A failed sync is not an error here: its status says why.
    pub async fn sync_now(&self, account_id: &str) -> crate::Result<()> {
        let rt = self.runtime(account_id)?;
        self.run_sync(&rt).await;
        Ok(())
    }

    /// On start, then every so many minutes after the last automatic sync, by the
    /// wall clock so a sleep doesn't push it back.
    async fn sync_loop(&self, rt: &Runtime) {
        let mut last: Option<SystemTime> = None;
        while let Some(account) = self.account(&rt.id) {
            let due = match (account.sync, last) {
                (SyncSchedule::Manual, _) | (SyncSchedule::OnStart, Some(_)) => break,
                (_, None) => Instant::now(),
                (SyncSchedule::Every(minutes), Some(last)) => {
                    let since = SystemTime::now().duration_since(last).unwrap_or_default();
                    let every = Duration::from_secs(u64::from(minutes) * 60);
                    Instant::now() + every.saturating_sub(since)
                }
            };
            sleep_until(due).await;
            if self.run_sync(rt).await != Ran::Busy {
                last = Some(SystemTime::now());
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
            Outcome::Done(Some(ran)) => ran,
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
        if result.is_ok() {
            Ran::Synced
        } else {
            Ran::Failed
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

fn now_secs() -> i64 {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    i64::try_from(since.as_secs()).unwrap_or(i64::MAX)
}
