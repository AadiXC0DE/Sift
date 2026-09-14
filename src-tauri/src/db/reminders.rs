//! Reminder rows (P8.2).
//!
//! A reminder is not a snooze. It never touches Inbox membership, unread state
//! or `last_message_at`; it only records that the user wants the thread back in
//! front of them at a chosen time. The row therefore stores nothing about the
//! message's state, which is also why it cannot corrupt it.
//!
//! Deletion is handled by the schema, deliberately: the composite foreign key
//! to `threads(account_id, id)` means a thread that disappears takes its
//! reminder with it, and removing the account cascades the same way. Removing
//! the account through `accounts_remove` also deletes the rows explicitly, so
//! the cleanup does not depend on a cascade firing.

use super::Db;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// One reminder as stored, plus the thread context the compact Reminders view
/// needs without a second query.
#[derive(Debug, Clone, serde::Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReminderRow {
    pub account_id: String,
    pub thread_id: String,
    pub remind_at: i64,
    pub delivered_at: Option<i64>,
    pub completed_at: Option<i64>,
    /// `scheduled` until the deadline passes, then `due` until delivery is
    /// recorded, then `delivered`, and `completed` once dismissed.
    pub state: String,
    /// Whether the undelivered reminder is overdue: the notify path failed (for
    /// instance the user denied notifications), so the thread keeps its row
    /// indicator instead of silently vanishing.
    pub due: bool,
    pub subject: Option<String>,
    pub from_name: Option<String>,
    pub unread: bool,
}

pub fn state_of(
    remind_at: i64,
    delivered_at: Option<i64>,
    completed_at: Option<i64>,
    now: i64,
) -> &'static str {
    if completed_at.is_some() {
        "completed"
    } else if delivered_at.is_some() {
        "delivered"
    } else if remind_at <= now {
        "due"
    } else {
        "scheduled"
    }
}

/// Insert or move one reminder. Rescheduling is the same statement: the row is
/// keyed by (account, thread), so there is exactly one reminder per thread and
/// a second "remind me" replaces the first rather than stacking.
pub(crate) fn upsert_conn(
    c: &Connection,
    account_id: &str,
    thread_id: &str,
    remind_at: i64,
    now: i64,
) -> Result<()> {
    c.execute(
        "INSERT INTO reminders (account_id, thread_id, remind_at, delivered_at, completed_at, created_at) \
         VALUES (?1, ?2, ?3, NULL, NULL, ?4) \
         ON CONFLICT(account_id, thread_id) DO UPDATE SET remind_at=?3, delivered_at=NULL, completed_at=NULL",
        params![account_id, thread_id, remind_at, now],
    )?;
    Ok(())
}

/// The next open deadline across every account, for the scheduler.
///
/// A reminder that has been delivered is no longer a reason to wake: without
/// the `delivered_at` clause the loop would find the same past instant forever
/// and spin on it. A *due but undelivered* reminder deliberately stays in the
/// result — that is what keeps it visible when notifications are denied.
pub(crate) fn next_deadline_conn(c: &Connection) -> rusqlite::Result<Option<i64>> {
    c.query_row(
        "SELECT MIN(remind_at) FROM reminders \
         WHERE completed_at IS NULL AND delivered_at IS NULL",
        [],
        |r| r.get(0),
    )
}

/// Overdue, undelivered reminders.
pub(crate) fn due_conn(c: &Connection, now: i64) -> rusqlite::Result<Vec<(String, String, i64)>> {
    let mut s = c.prepare(
        "SELECT account_id, thread_id, remind_at FROM reminders \
         WHERE completed_at IS NULL AND delivered_at IS NULL AND remind_at<=? \
         ORDER BY remind_at, account_id, thread_id",
    )?;
    let rows = s
        .query_map(params![now], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Record delivery **before** anything is shown to the user.
///
/// The write is conditional on the row still being undelivered and not
/// completed, so two scheduler ticks racing for the same reminder produce one
/// winner and therefore one notification.
pub(crate) fn mark_delivered_conn(
    c: &Connection,
    account_id: &str,
    thread_id: &str,
    now: i64,
) -> rusqlite::Result<bool> {
    let changed = c.execute(
        "UPDATE reminders SET delivered_at=?3 \
         WHERE account_id=?1 AND thread_id=?2 AND delivered_at IS NULL AND completed_at IS NULL",
        params![account_id, thread_id, now],
    )?;
    Ok(changed == 1)
}

impl Db {
    pub async fn reminder_upsert(
        &self,
        account_id: &str,
        thread_id: &str,
        remind_at: i64,
    ) -> Result<()> {
        let (a, t) = (account_id.to_string(), thread_id.to_string());
        let now = super::now_ms();
        self.write(move |c| upsert_conn(c, &a, &t, remind_at, now))
            .await
    }

    /// Bulk upsert, one transaction per account, so one gesture cannot
    /// half-apply.
    pub async fn reminders_upsert(
        &self,
        targets: &[(String, String)],
        remind_at: i64,
    ) -> Result<()> {
        let targets = targets.to_vec();
        let now = super::now_ms();
        self.write_tx(move |tx| {
            for (a, t) in &targets {
                upsert_conn(tx, a, t, remind_at, now)?;
            }
            Ok(())
        })
        .await
    }

    pub async fn reminder_remove(&self, account_id: &str, thread_id: &str) -> Result<()> {
        let (a, t) = (account_id.to_string(), thread_id.to_string());
        self.write(move |c| {
            c.execute(
                "DELETE FROM reminders WHERE account_id=? AND thread_id=?",
                params![a, t],
            )?;
            Ok(())
        })
        .await
    }

    /// Mark a reminder done. The row stays: a completed reminder is what stops
    /// a restored thread from looking like it still has one.
    pub async fn reminder_complete(&self, account_id: &str, thread_id: &str) -> Result<bool> {
        let (a, t) = (account_id.to_string(), thread_id.to_string());
        let now = super::now_ms();
        self.write(move |c| {
            let changed = c.execute(
                "UPDATE reminders SET completed_at=?3 WHERE account_id=?1 AND thread_id=?2 AND completed_at IS NULL",
                params![a, t, now],
            )?;
            Ok(changed == 1)
        })
        .await
    }

    /// Hand a claimed reminder back after its banner was refused.
    ///
    /// Without this a failed banner would be recorded as a delivered reminder,
    /// so it would silently vanish from the list and never be retried — the
    /// opposite of what [`Delivery::Denied`](crate::notify::Delivery) promises
    /// the caller.
    pub async fn reminder_reopen(&self, account_id: &str, thread_id: &str) -> Result<bool> {
        let (a, t) = (account_id.to_string(), thread_id.to_string());
        self.write(move |c| {
            let changed = c.execute(
                "UPDATE reminders SET delivered_at=NULL \
                 WHERE account_id=?1 AND thread_id=?2 AND completed_at IS NULL",
                params![a, t],
            )?;
            Ok(changed == 1)
        })
        .await
    }

    pub async fn reminder_next_deadline(&self) -> Result<Option<i64>> {
        self.read(move |c| Ok(next_deadline_conn(c)?)).await
    }

    pub async fn reminders_due(&self, now: i64) -> Result<Vec<(String, String, i64)>> {
        self.read(move |c| Ok(due_conn(c, now)?)).await
    }

    /// Claim delivery for one reminder; `true` means this caller owns it.
    pub async fn reminder_mark_delivered(&self, account_id: &str, thread_id: &str) -> Result<bool> {
        let (a, t) = (account_id.to_string(), thread_id.to_string());
        let now = super::now_ms();
        self.write(move |c| Ok(mark_delivered_conn(c, &a, &t, now)?))
            .await
    }

    /// The reminder indicator for a set of threads, keyed by
    /// `(account_id, thread_id)`. Only an open reminder counts: a completed
    /// one is history, not a row badge.
    pub async fn reminders_for_threads(
        &self,
        account_id: &str,
        thread_ids: Vec<String>,
    ) -> Result<Vec<(String, i64)>> {
        let a = account_id.to_string();
        self.read(move |c| {
            if thread_ids.is_empty() {
                return Ok(vec![]);
            }
            let holes = vec!["?"; thread_ids.len()].join(",");
            let sql = format!(
                "SELECT thread_id, remind_at FROM reminders \
                 WHERE account_id=? AND completed_at IS NULL AND thread_id IN ({holes})"
            );
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(a)];
            for t in &thread_ids {
                binds.push(Box::new(t.clone()));
            }
            let rows = c
                .prepare(&sql)?
                .query_map(
                    rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())),
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?
                .collect::<Result<Vec<(String, i64)>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// The compact Reminders view: open reminders first, newest deadline last,
    /// joined to the thread it points at.
    pub async fn reminders_list(
        &self,
        account_ids: &[String],
        include_completed: bool,
    ) -> Result<Vec<ReminderRow>> {
        let accounts = account_ids.to_vec();
        let now = super::now_ms();
        self.read(move |c| {
            let mut where_sql = String::from("1=1");
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![];
            if !accounts.is_empty() {
                let holes = vec!["?"; accounts.len()].join(",");
                where_sql.push_str(&format!(" AND r.account_id IN ({holes})"));
                for a in &accounts {
                    binds.push(Box::new(a.clone()));
                }
            }
            if !include_completed {
                where_sql.push_str(" AND r.completed_at IS NULL");
            }
            let sql = format!(
                "SELECT r.account_id, r.thread_id, r.remind_at, r.delivered_at, r.completed_at, \
                        t.subject, t.unread_count \
                 FROM reminders r \
                 LEFT JOIN threads t ON t.account_id=r.account_id AND t.id=r.thread_id \
                 WHERE {where_sql} \
                 ORDER BY (r.completed_at IS NOT NULL), r.remind_at, r.account_id, r.thread_id"
            );
            let rows = c
                .prepare(&sql)?
                .query_map(
                    rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())),
                    |r| {
                        let remind_at: i64 = r.get(2)?;
                        let delivered_at: Option<i64> = r.get(3)?;
                        let completed_at: Option<i64> = r.get(4)?;
                        let unread_count: Option<i64> = r.get(6)?;
                        Ok(ReminderRow {
                            account_id: r.get(0)?,
                            thread_id: r.get(1)?,
                            remind_at,
                            delivered_at,
                            completed_at,
                            state: state_of(remind_at, delivered_at, completed_at, now).to_string(),
                            due: completed_at.is_none()
                                && delivered_at.is_none()
                                && remind_at <= now,
                            subject: r.get(5)?,
                            from_name: None,
                            unread: unread_count.unwrap_or(0) > 0,
                        })
                    },
                )?
                .collect::<Result<Vec<ReminderRow>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// How many open reminders exist for a selection: the saved view is only
    /// offered when this is non-zero.
    pub async fn reminders_open_count(&self, account_ids: &[String]) -> Result<i64> {
        let accounts = account_ids.to_vec();
        self.read(move |c| {
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![];
            let mut where_sql = String::from("completed_at IS NULL");
            if !accounts.is_empty() {
                let holes = vec!["?"; accounts.len()].join(",");
                where_sql.push_str(&format!(" AND account_id IN ({holes})"));
                for a in &accounts {
                    binds.push(Box::new(a.clone()));
                }
            }
            let n: i64 = c.query_row(
                &format!("SELECT count(*) FROM reminders WHERE {where_sql}"),
                rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())),
                |r| r.get(0),
            )?;
            Ok(n)
        })
        .await
    }

    /// The from-name of a thread's latest message, for the notification body.
    pub async fn reminder_context(
        &self,
        account_id: &str,
        thread_id: &str,
    ) -> Result<(Option<String>, Option<String>)> {
        let (a, t) = (account_id.to_string(), thread_id.to_string());
        self.read(move |c| {
            let row: Option<(Option<String>, String)> = c
                .query_row(
                    "SELECT from_name, subject FROM messages \
                     WHERE account_id=? AND thread_id=? AND is_draft=0 \
                     ORDER BY internal_date DESC, id DESC LIMIT 1",
                    params![a, t],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            Ok(match row {
                Some((name, subject)) => (name, Some(subject)),
                None => (None, None),
            })
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use crate::db::Db;

    async fn db() -> Db {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        std::mem::forget(dir);
        db
    }

    async fn seed(db: &Db, account: &str, thread: &str, at: i64) {
        let (account, thread) = (account.to_string(), thread.to_string());
        db.write(move |c| {
            c.execute(
                "INSERT OR IGNORE INTO accounts (id,email,created_at) VALUES (?1,?1||'@x',0)",
                rusqlite::params![account],
            )?;
            c.execute(
                "INSERT OR IGNORE INTO threads (account_id,id,last_message_at,first_message_at) VALUES (?1,?2,?3,?3)",
                rusqlite::params![account, thread, at],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn p8_2_reminder_roundtrip_and_one_per_thread() {
        let db = db().await;
        seed(&db, "a", "t1", 0).await;
        seed(&db, "a", "t2", 0).await;
        let now = crate::db::now_ms();
        let first = now + 60_000;
        db.reminders_upsert(
            &[("a".into(), "t1".into()), ("a".into(), "t2".into())],
            first,
        )
        .await
        .unwrap();
        assert_eq!(db.reminder_next_deadline().await.unwrap(), Some(first));
        // Rescheduling replaces rather than stacks.
        let later = now + 120_000;
        db.reminder_upsert("a", "t1", later).await.unwrap();
        let rows = db.reminders_list(&[], false).await.unwrap();
        assert_eq!(rows.len(), 2, "one reminder per thread, not one per set");
        assert_eq!(rows[1].remind_at, later);
        assert_eq!(rows[0].state, "scheduled");
        assert!(!rows[0].due);
    }

    #[tokio::test]
    async fn p8_2_delivery_is_claimed_once_and_survives_denial() {
        let db = db().await;
        seed(&db, "a", "t1", 0).await;
        db.reminder_upsert("a", "t1", 1).await.unwrap();
        let due = db.reminders_due(crate::db::now_ms()).await.unwrap();
        assert_eq!(due.len(), 1);
        assert!(db.reminder_mark_delivered("a", "t1").await.unwrap());
        // A second tick (or a second scheduler) must not claim it again.
        assert!(!db.reminder_mark_delivered("a", "t1").await.unwrap());
        assert!(db
            .reminders_due(crate::db::now_ms())
            .await
            .unwrap()
            .is_empty());
        // Denial means nothing was delivered: the row stays visible and due.
        db.reminder_upsert("a", "t1", 1).await.unwrap();
        let rows = db.reminders_list(&[], false).await.unwrap();
        assert_eq!(rows[0].state, "due");
        assert!(rows[0].due);
    }

    #[tokio::test]
    async fn p8_2_deleting_the_target_cancels_the_reminder() {
        let db = db().await;
        seed(&db, "a", "t1", 0).await;
        db.reminder_upsert("a", "t1", 9_000).await.unwrap();
        db.write(|c| {
            c.execute("DELETE FROM threads WHERE account_id='a' AND id='t1'", [])?;
            Ok(())
        })
        .await
        .unwrap();
        assert_eq!(db.reminder_next_deadline().await.unwrap(), None);
        assert!(db.reminders_list(&[], true).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn p8_2_completed_reminder_does_not_resurrect() {
        let db = db().await;
        seed(&db, "a", "t1", 0).await;
        db.reminder_upsert("a", "t1", 1).await.unwrap();
        assert!(db.reminder_complete("a", "t1").await.unwrap());
        // Restoring from Trash only flips thread flags; it must not clear the
        // completion, and a completed row is never due again.
        db.reminder_mark_delivered("a", "t1").await.unwrap();
        assert!(db.reminders_due(i64::MAX / 2).await.unwrap().is_empty());
        let rows = db.reminders_list(&[], true).await.unwrap();
        assert_eq!(rows[0].state, "completed");
        assert!(!rows[0].due);
    }
}
