//! The one local scheduler (P8.2).
//!
//! Snooze deadlines, reminder deadlines and queued-send deadlines are the same
//! problem: "wake up when this persisted instant arrives and recheck the
//! condition". They were three different mechanisms, so a message could be
//! late by a poll interval, and nothing agreed on what "now" meant. This
//! module owns all of them.
//!
//! What it deliberately does *not* own is provider sync. Inbox polling runs at
//! the cadence the user chose and has nothing to do with schedule precision: a
//! message scheduled for 08:00 leaves at 08:00 because the persisted deadline
//! woke the outbox, not because a poll happened to run.
//!
//! The loop is deadline-driven with a bounded fallback. The fallback is what
//! makes a manual clock change, a DST transition or a laptop that slept through
//! its alarm recover instead of stranding a deadline until the next launch.

use crate::app_state::AppState;
use crate::db::Db;
use crate::errors::SiftError;
use std::time::Duration;
use tauri::{AppHandle, Manager};

/// The longest the scheduler will sleep before re-reading the clock. The
/// nearest deadline wakes it earlier; this cap bounds how long a clock change
/// or a DST jump can strand due work.
pub const WATCH_FALLBACK_MS: i64 = 30_000;

/// The shortest sleep, so a deadline that is due *now* still yields the write
/// lane to whatever caused the wake.
pub const MIN_SLEEP_MS: i64 = 50;

/// What one tick did, for tests and diagnostics.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TickReport {
    pub snoozes_woken: usize,
    pub reminders_shown: usize,
    pub accounts_kicked: usize,
}

fn db_error(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

/// The nearest instant at which *any* local deadline is due, across snoozes,
/// reminders and the outbox of every account.
pub async fn next_deadline(db: &Db) -> Result<Option<i64>, SiftError> {
    let snooze = crate::snooze::next_deadline(db).await?;
    let reminder = crate::reminders::next_deadline(db).await?;
    let outbox = db.outbox_next_deadline(None).await.map_err(db_error)?;
    Ok([snooze, reminder, outbox].into_iter().flatten().min())
}

/// How long to sleep before the next tick, bounded by the fallback.
pub fn sleep_for(deadline: Option<i64>, now: i64) -> i64 {
    match deadline {
        Some(when) => (when - now).clamp(MIN_SLEEP_MS, WATCH_FALLBACK_MS),
        None => WATCH_FALLBACK_MS,
    }
}

/// One tick: fire what is due, then let the caller sleep.
pub async fn tick(app: &AppHandle) -> anyhow::Result<TickReport> {
    let state = app.state::<AppState>();
    let host = crate::runtime::RuntimeHost::for_app(app);
    let now = crate::db::now_ms();

    // 1. Snoozes: the timer, the label change and the queued operation are one
    //    transaction per account, so a wake is never half-applied (P6.5).
    let snoozes_woken = crate::runtime::check_snoozes(app).await?;

    // 2. Reminders: at most one notification each, marked before it is shown.
    let reminders_shown = crate::reminders::deliver_due(&state.db, &host, now)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .len();

    // 3. Queued work whose deadline has arrived. Only accounts with due work
    //    are nudged, so an idle mailbox costs one indexed query per tick.
    let mut report = TickReport {
        snoozes_woken,
        reminders_shown,
        accounts_kicked: 0,
    };
    for account_id in state
        .db
        .outbox_due_accounts(now)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
    {
        state.kick_outbox(&account_id).await;
        report.accounts_kicked += 1;
    }
    Ok(report)
}

/// Run the scheduler forever. Started once by the supervisor.
pub async fn run(app: AppHandle) {
    loop {
        if let Err(e) = tick(&app).await {
            log::warn!("scheduler tick: {e}");
        }
        let deadline = {
            let state = app.state::<AppState>();
            next_deadline(&state.db).await.ok().flatten()
        };
        let wait = sleep_for(deadline, crate::db::now_ms());
        tokio::time::sleep(Duration::from_millis(wait as u64)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    async fn db() -> Db {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        std::mem::forget(dir);
        db
    }

    async fn account(db: &Db, id: &str) {
        let id = id.to_string();
        db.write(move |c| {
            c.execute(
                "INSERT OR IGNORE INTO accounts (id,email,created_at,sync_state) VALUES (?1,?1||'@x',0,'partial')",
                rusqlite::params![id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[test]
    fn p8_2_sleep_is_bounded_by_the_fallback() {
        assert_eq!(sleep_for(Some(1_000), 900), 100);
        assert_eq!(sleep_for(Some(900), 1_000), MIN_SLEEP_MS);
        assert_eq!(sleep_for(Some(1_000_000), 0), WATCH_FALLBACK_MS);
        assert_eq!(sleep_for(None, 0), WATCH_FALLBACK_MS);
    }

    #[tokio::test]
    async fn p8_2_one_deadline_covers_snooze_reminder_and_send() {
        let db = db().await;
        account(&db, "a").await;
        db.write(|c| {
            c.execute(
                "INSERT INTO threads (account_id,id,last_message_at,first_message_at) VALUES ('a','t1',0,0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let now = crate::db::now_ms();
        assert_eq!(next_deadline(&db).await.unwrap(), None);

        // A reminder 30 s out.
        db.reminder_upsert("a", "t1", now + 30_000).await.unwrap();
        assert_eq!(next_deadline(&db).await.unwrap(), Some(now + 30_000));

        // A snooze 10 s out wins.
        db.write(move |c| {
            c.execute(
                "INSERT INTO snoozes (account_id,thread_id,wake_at,state) VALUES ('a','t1',?1,'sleeping')",
                rusqlite::params![now + 10_000],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(next_deadline(&db).await.unwrap(), Some(now + 10_000));

        // A queued send at 20 s is behind both; a queued send at 5 s wins.
        db.outbox_enqueue("a", "modify_labels", "{}", None, now + 20_000)
            .await
            .unwrap();
        assert_eq!(next_deadline(&db).await.unwrap(), Some(now + 10_000));
        db.outbox_enqueue("a", "modify_labels", "{}", None, now + 5_000)
            .await
            .unwrap();
        assert_eq!(next_deadline(&db).await.unwrap(), Some(now + 5_000));

        // An operation that is already due reports "now", never a past time
        // that would make the loop sleep for zero.
        db.outbox_enqueue("a", "modify_labels", "{}", None, 0)
            .await
            .unwrap();
        let now = crate::db::now_ms();
        let d = next_deadline(&db).await.unwrap().unwrap();
        assert!(d >= now - 1_000 && d <= now + 1_000, "{d} vs {now}");
        assert_eq!(
            db.outbox_due_accounts(crate::db::now_ms()).await.unwrap(),
            vec!["a".to_string()]
        );
    }

    #[tokio::test]
    async fn p8_2_an_undelivered_reminder_keeps_its_deadline() {
        let db = db().await;
        account(&db, "a").await;
        db.write(|c| {
            c.execute(
                "INSERT INTO threads (account_id,id,last_message_at,first_message_at) VALUES ('a','t1',0,0)",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        db.reminder_upsert("a", "t1", 1).await.unwrap();
        // Still due, so the scheduler keeps seeing it: that is what keeps the
        // reminder visible when notifications are denied.
        assert_eq!(next_deadline(&db).await.unwrap(), Some(1));
        assert_eq!(
            db.reminders_due(crate::db::now_ms()).await.unwrap().len(),
            1
        );
        // Once delivery is recorded it stops being due but stays visible.
        db.reminder_mark_delivered("a", "t1").await.unwrap();
        assert_eq!(next_deadline(&db).await.unwrap(), None);
        assert_eq!(db.reminders_list(&[], false).await.unwrap().len(), 1);
    }
}
