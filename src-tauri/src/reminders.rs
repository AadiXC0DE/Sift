//! Reminders (P8.2): "Remind me…" leaves the message where it is.
//!
//! The distinction from Snooze is the whole point of the feature. Snooze hides
//! a thread until later and puts it back with its Inbox policy restored;
//! a reminder changes nothing about the mail — not Inbox membership, not
//! unread state, not `last_message_at`, not a single provider label. It only
//! records that the user wants the thread in front of them again at a chosen
//! time, and it says so once.
//!
//! Delivery is at-most-once and honest about failure:
//!
//! * the durable `delivered_at` mark is written **before** the banner is
//!   raised, so a crash between the two cannot notify twice;
//! * when notification permission is denied nothing is marked, so the reminder
//!   stays visibly `due` in the list instead of silently disappearing;
//! * the row is what the list and the thread indicator read, so a reminder the
//!   user never dismissed is still there after a restart.

use crate::db::reminders::ReminderRow;
use crate::db::Db;
use crate::dto::GestureTarget;
use crate::errors::SiftError;

/// A reminder must be in the future, and no further out than a year.
pub const MIN_LEAD_MS: i64 = 5_000;
pub const MAX_AHEAD_MS: i64 = 366 * 24 * 60 * 60 * 1000;

/// Where a reminder's notification takes the user when they click it.
#[derive(Debug, Clone, PartialEq)]
pub struct DueReminder {
    pub account_id: String,
    pub thread_id: String,
    pub title: String,
    pub subject: String,
}

fn db_error(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

/// Validate a reminder deadline against the clock as it is now.
pub fn validate(remind_at: i64, now: i64) -> Result<(), SiftError> {
    if remind_at < now + MIN_LEAD_MS {
        return Err(SiftError::app(
            "reminder_invalid",
            "Pick a time a little further ahead for this reminder.",
            false,
        ));
    }
    if remind_at > now + MAX_AHEAD_MS {
        return Err(SiftError::app(
            "reminder_invalid",
            "Sift can set a reminder up to a year ahead.",
            false,
        ));
    }
    Ok(())
}

/// Create or move reminders for a set of threads.
///
/// The write is one transaction: either every target has its reminder or none
/// does, and nothing about the threads themselves is touched.
pub async fn set(
    db: &Db,
    targets: &[GestureTarget],
    remind_at: i64,
) -> Result<Vec<ReminderRow>, SiftError> {
    validate(remind_at, crate::db::now_ms())?;
    let pairs: Vec<(String, String)> = targets
        .iter()
        .map(|t| (t.account_id.clone(), t.thread_id.clone()))
        .collect();
    // A target whose thread no longer exists is a typed failure, not a foreign
    // key error the UI cannot explain.
    for (account, thread) in &pairs {
        let exists = db
            .read({
                let (a, t) = (account.clone(), thread.clone());
                move |c| {
                    Ok(c.query_row(
                        "SELECT EXISTS(SELECT 1 FROM threads WHERE account_id=? AND id=?)",
                        rusqlite::params![a, t],
                        |r| r.get::<_, bool>(0),
                    )?)
                }
            })
            .await
            .map_err(db_error)?;
        if !exists {
            return Err(SiftError::NotFound("thread".into()));
        }
    }
    db.reminders_upsert(&pairs, remind_at)
        .await
        .map_err(db_error)?;
    let accounts: Vec<String> = pairs.iter().map(|(a, _)| a.clone()).collect();
    list(db, &accounts, false).await
}

/// Remove reminders for a set of threads.
pub async fn clear(db: &Db, targets: &[GestureTarget]) -> Result<(), SiftError> {
    for t in targets {
        db.reminder_remove(&t.account_id, &t.thread_id)
            .await
            .map_err(db_error)?;
    }
    Ok(())
}

/// Mark reminders done. The row stays (completed) so a restart, a re-sync or a
/// restore from Trash cannot resurrect it.
pub async fn complete(db: &Db, targets: &[GestureTarget]) -> Result<(), SiftError> {
    for t in targets {
        db.reminder_complete(&t.account_id, &t.thread_id)
            .await
            .map_err(db_error)?;
    }
    Ok(())
}

pub async fn list(
    db: &Db,
    account_ids: &[String],
    include_completed: bool,
) -> Result<Vec<ReminderRow>, SiftError> {
    db.reminders_list(account_ids, include_completed)
        .await
        .map_err(db_error)
}

/// Reminders whose deadline has passed and that have not been reported yet.
pub async fn due(db: &Db, now: i64) -> Result<Vec<DueReminder>, SiftError> {
    let rows = db.reminders_due(now).await.map_err(db_error)?;
    let mut out = Vec::with_capacity(rows.len());
    for (account_id, thread_id, _) in rows {
        let (from_name, subject) = db
            .reminder_context(&account_id, &thread_id)
            .await
            .map_err(db_error)?;
        out.push(DueReminder {
            account_id,
            thread_id,
            title: from_name.unwrap_or_else(|| "this conversation".into()),
            subject: subject.unwrap_or_else(|| "Reminder".into()),
        });
    }
    Ok(out)
}

/// Deliver every due reminder once.
///
/// Returns the reminders that were actually shown, so the caller can emit the
/// row/store events. A denied permission leaves the item out of that list —
/// the reminder is still there, still due, and still visible.
pub async fn deliver_due(
    db: &Db,
    host: &crate::runtime::RuntimeHost,
    now: i64,
) -> Result<Vec<DueReminder>, SiftError> {
    let due = due(db, now).await?;
    let mut shown = Vec::with_capacity(due.len());
    for item in due {
        let delivery = crate::notify::deliver_reminder(
            db,
            host,
            &item.account_id,
            &item.thread_id,
            &item.title,
            &item.subject,
        )
        .await;
        if delivery == crate::notify::Delivery::Shown {
            (host.emit)(
                "reminder:due",
                serde_json::json!({
                    "account_id": item.account_id,
                    "thread_id": item.thread_id,
                    "remind_at": now,
                }),
            );
            shown.push(item);
        }
    }
    Ok(shown)
}

/// The nearest open reminder deadline, for the scheduler.
pub async fn next_deadline(db: &Db) -> Result<Option<i64>, SiftError> {
    db.reminder_next_deadline().await.map_err(db_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p8_2_deadlines_are_validated() {
        let now = 1_000_000;
        assert!(validate(now + 60_000, now).is_ok());
        assert_eq!(
            validate(now - 1, now).unwrap_err().code(),
            "reminder_invalid"
        );
        assert_eq!(
            validate(now + MAX_AHEAD_MS + 1, now).unwrap_err().code(),
            "reminder_invalid"
        );
    }

    #[tokio::test]
    async fn p8_2_setting_a_reminder_changes_nothing_about_the_mail() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        db.write(|c| {
            c.execute(
                "INSERT INTO accounts (id,email,created_at) VALUES ('a','a@x',0)",
                [],
            )?;
            c.execute(
                "INSERT INTO threads (account_id,id,subject,last_message_at,first_message_at,unread_count,in_inbox,label_ids) \
                 VALUES ('a','t1','Hi',500,500,1,1,'[\"INBOX\",\"UNREAD\"]')",
                [],
            )?;
            c.execute(
                "INSERT INTO messages (id,account_id,thread_id,internal_date,is_unread,label_ids) \
                 VALUES ('m1','a','t1',500,1,'[\"INBOX\",\"UNREAD\"]')",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let before: (i64, i64, i64, String) = db
            .read(|c| {
                Ok(c.query_row(
                    "SELECT last_message_at, unread_count, in_inbox, label_ids FROM threads WHERE id='t1'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?)
            })
            .await
            .unwrap();
        let targets = vec![GestureTarget {
            account_id: "a".into(),
            thread_id: "t1".into(),
        }];
        let rows = set(&db, &targets, crate::db::now_ms() + 60_000)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].state, "scheduled");
        let after: (i64, i64, i64, String) = db
            .read(|c| {
                Ok(c.query_row(
                    "SELECT last_message_at, unread_count, in_inbox, label_ids FROM threads WHERE id='t1'",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(before, after, "a reminder must not touch the mail");
        // And nothing was queued for the provider.
        let ops: i64 = db
            .read(|c| Ok(c.query_row("SELECT count(*) FROM outbox_ops", [], |r| r.get(0))?))
            .await
            .unwrap();
        assert_eq!(ops, 0, "a reminder is local: no provider operation");
    }

    #[tokio::test]
    async fn p8_2_missing_target_is_a_typed_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let err = set(
            &db,
            &[GestureTarget {
                account_id: "a".into(),
                thread_id: "gone".into(),
            }],
            crate::db::now_ms() + 60_000,
        )
        .await
        .unwrap_err();
        assert!(matches!(err, SiftError::NotFound(_)));
    }
}
