//! Phase 8 P8.2 (reminders) and P8.4 (notifications).
//!
//! Both features make the same promise in different words: the user is told
//! once, and the state stays honest when the OS refuses. These tests drive the
//! real delivery functions with a host that counts what it was asked to show.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use sift::db::Db;
use sift::dto::GestureTarget;
use sift::notify::{deliver_new_mail, Delivery, Notice, Policy};
use sift::runtime::RuntimeHost;

/// A host that records every notification it was asked to raise.
struct Recorder {
    host: RuntimeHost,
    shown: Arc<AtomicUsize>,
    emitted: Arc<AtomicUsize>,
}

impl Recorder {
    /// `available` is what the OS reports about notification permission, and
    /// `accept` is whether the banner itself is delivered.
    fn new(available: bool, accept: bool) -> Self {
        let shown = Arc::new(AtomicUsize::new(0));
        let emitted = Arc::new(AtomicUsize::new(0));
        let counter = shown.clone();
        let events = emitted.clone();
        Self {
            host: RuntimeHost {
                emit: Arc::new(move |_, _| {
                    events.fetch_add(1, Ordering::SeqCst);
                }),
                notify_os: Arc::new(move |_| {
                    if accept {
                        counter.fetch_add(1, Ordering::SeqCst);
                    }
                    accept
                }),
                focused: Arc::new(|| true),
                notifications_available: Arc::new(move || available),
                badge: Arc::new(|_| {}),
            },
            shown,
            emitted,
        }
    }

    fn shown(&self) -> usize {
        self.shown.load(Ordering::SeqCst)
    }

    fn emitted(&self) -> usize {
        self.emitted.load(Ordering::SeqCst)
    }
}

async fn fixture() -> (tempfile::TempDir, Db, String) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("ada@x.com", None, None).await.unwrap();
    let id = acc.id.clone();
    // One thread with one message, so a reminder has real context to show.
    db.write(move |c| {
        c.execute(
            "INSERT INTO threads (account_id,id,last_message_at,first_message_at,in_inbox,unread_count,label_ids) \
             VALUES (?1,'t1',100,100,1,1,'[\"INBOX\",\"UNREAD\"]')",
            rusqlite::params![acc.id],
        )?;
        c.execute(
            "INSERT INTO messages (id,account_id,thread_id,internal_date,from_name,from_email,subject,label_ids) \
             VALUES ('m1',?1,'t1',100,'Ada','ada@x.com','Lunch?','[\"INBOX\",\"UNREAD\"]')",
            rusqlite::params![acc.id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    (dir, db, id)
}

fn target(account_id: &str) -> GestureTarget {
    GestureTarget {
        account_id: account_id.to_string(),
        thread_id: "t1".into(),
    }
}

/// Inbox membership, unread count and last activity of the thread — the three
/// things a reminder must never change.
async fn thread_state(db: &Db, account_id: &str) -> (i64, i64, i64, String) {
    let a = account_id.to_string();
    db.read(move |c| {
        Ok(c.query_row(
            "SELECT in_inbox, unread_count, last_message_at, label_ids FROM threads WHERE account_id=? AND id='t1'",
            rusqlite::params![a],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )?)
    })
    .await
    .unwrap()
}

// -- P8.2 reminders ---------------------------------------------------------

/// P8.2: a due reminder is shown once, and asking again — a second tick, a
/// restart — shows nothing.
#[tokio::test]
async fn p8_2_a_due_reminder_is_delivered_exactly_once() {
    let (_dir, db, acc) = fixture().await;
    let recorder = Recorder::new(true, true);
    let now = sift::db::now_ms();
    // The deadline has arrived: this is the row the UI shows as due.
    db.reminders_upsert(&[(acc.clone(), "t1".to_string())], now - 1_000)
        .await
        .unwrap();

    let due_at = now;
    let delivered = sift::reminders::deliver_due(&db, &recorder.host, due_at)
        .await
        .unwrap();
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].thread_id, "t1");
    assert_eq!(delivered[0].title, "Ada");
    assert_eq!(delivered[0].subject, "Lunch?");
    assert_eq!(
        db.reminders_list(std::slice::from_ref(&acc), false)
            .await
            .unwrap()[0]
            .state,
        "delivered"
    );
    assert_eq!(recorder.shown(), 1);
    assert_eq!(
        recorder.emitted(),
        1,
        "the store is told which thread changed"
    );

    // The durable mark was written before the banner, so a second pass — a
    // restart, a racing scheduler — is silent.
    let again = sift::reminders::deliver_due(&db, &recorder.host, due_at + 60_000)
        .await
        .unwrap();
    assert!(again.is_empty());
    assert_eq!(recorder.shown(), 1);

    // A reminder is not a snooze: the mail itself is untouched.
    assert_eq!(
        thread_state(&db, &acc).await,
        (1, 1, 100, "[\"INBOX\",\"UNREAD\"]".into()),
        "a reminder does not archive, unread or bump the thread"
    );
}

/// P8.2: when notifications are denied the reminder stays due and visible, and
/// nothing is marked as delivered.
#[tokio::test]
async fn p8_2_a_denied_reminder_stays_visible_and_undelivered() {
    let (_dir, db, acc) = fixture().await;
    let recorder = Recorder::new(false, false);
    let now = sift::db::now_ms();
    db.reminders_upsert(&[(acc.clone(), "t1".to_string())], now - 1_000)
        .await
        .unwrap();

    let due_at = now;
    let denied = sift::reminders::deliver_due(&db, &recorder.host, due_at)
        .await
        .unwrap();
    assert!(denied.is_empty(), "nothing was shown");
    assert_eq!(recorder.shown(), 0);

    // Still due, still listed, still flagged on the thread.
    let rows = sift::reminders::list(&db, std::slice::from_ref(&acc), false)
        .await
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].state, "due");
    assert!(rows[0].due);
    let indicator = db
        .reminders_for_threads(&acc, vec!["t1".into()])
        .await
        .unwrap();
    assert_eq!(indicator.len(), 1, "the row keeps its reminder indicator");
    assert_eq!(
        db.reminders_due(due_at).await.unwrap().len(),
        1,
        "a denied reminder is still a due reminder"
    );

    // Once the OS accepts one, that same reminder is delivered exactly once.
    let accepting = Recorder::new(true, true);
    let shown = sift::reminders::deliver_due(&db, &accepting.host, due_at)
        .await
        .unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(accepting.shown(), 1);
    let rows = sift::reminders::list(&db, std::slice::from_ref(&acc), false)
        .await
        .unwrap();
    assert_eq!(rows[0].state, "delivered");
    assert!(!rows[0].due);
}

/// P8.2: the OS refusing the banner (permission granted but delivery failed) is
/// treated exactly like a denial: the reminder is not marked delivered.
#[tokio::test]
async fn p8_2_a_banner_the_os_refuses_is_not_marked_delivered() {
    let (_dir, db, acc) = fixture().await;
    let recorder = Recorder::new(true, false);
    let now = sift::db::now_ms();
    db.reminders_upsert(&[(acc.clone(), "t1".to_string())], now - 1_000)
        .await
        .unwrap();
    let outcome =
        sift::notify::deliver_reminder(&db, &recorder.host, &acc, "t1", "Ada", "Lunch?").await;
    assert_eq!(outcome, Delivery::Denied);
    let rows = sift::reminders::list(&db, std::slice::from_ref(&acc), false)
        .await
        .unwrap();
    assert!(rows[0].due, "a refused banner leaves the reminder due");
}

/// P8.2: deleting the target, or removing the account, cancels its reminder.
#[tokio::test]
async fn p8_2_deleting_the_target_or_the_account_cancels_the_reminder() {
    let (_dir, db, acc) = fixture().await;
    let now = sift::db::now_ms();
    // The UI path: a time the user picked, validated and stored.
    sift::reminders::set(&db, &[target(&acc)], now + 60_000)
        .await
        .unwrap();
    assert_eq!(
        sift::reminders::next_deadline(&db).await.unwrap(),
        Some(now + 60_000)
    );
    // A deadline already behind the clock is refused by the same path.
    assert_eq!(
        sift::reminders::set(&db, &[target(&acc)], now - 60_000)
            .await
            .unwrap_err()
            .code(),
        "reminder_invalid"
    );

    // The thread goes (deleted forever, or removed with its account further
    // down): the reminder row goes with it.
    db.write({
        let a = acc.clone();
        move |c| {
            c.execute(
                "DELETE FROM threads WHERE account_id=? AND id='t1'",
                rusqlite::params![a],
            )?;
            Ok(())
        }
    })
    .await
    .unwrap();
    assert_eq!(sift::reminders::next_deadline(&db).await.unwrap(), None);
    assert!(sift::reminders::list(&db, std::slice::from_ref(&acc), true)
        .await
        .unwrap()
        .is_empty());

    // A second account's reminder survives its neighbour's removal, and the
    // removal itself cancels the rest.
    let other = db.new_account("bob@x.com", None, None).await.unwrap();
    db.write({
        let a = other.id.clone();
        move |c| {
            c.execute(
                "INSERT INTO threads (account_id,id,last_message_at,first_message_at) VALUES (?1,'t2',100,100)",
                rusqlite::params![a],
            )?;
            Ok(())
        }
    })
    .await
    .unwrap();
    sift::reminders::set(
        &db,
        &[GestureTarget {
            account_id: other.id.clone(),
            thread_id: "t2".into(),
        }],
        now + 120_000,
    )
    .await
    .unwrap();
    db.accounts_remove(&other.id).await.unwrap();
    assert!(db.accounts_get(&other.id).await.unwrap().is_none());
    assert!(
        sift::reminders::list(&db, std::slice::from_ref(&other.id), true)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(sift::reminders::next_deadline(&db).await.unwrap(), None);
}

// -- P8.4 notifications -----------------------------------------------------

fn notice(account: &str, thread: &str, message: &str, from: &str) -> Notice {
    Notice {
        account_id: account.into(),
        thread_id: thread.into(),
        message_id: message.into(),
        from: from.into(),
        subject: "Hello".into(),
        in_inbox: true,
        is_vip: false,
    }
}

/// P8.4: one message produces one notification claim, so a repeat sync, a
/// restart or a second scheduler cannot raise a second banner for it.
#[tokio::test]
async fn p8_4_one_message_produces_one_notification() {
    let (_dir, db, acc) = fixture().await;
    let recorder = Recorder::new(true, true);
    let n = notice(&acc, "t1", "m1", "Ada");

    let first = deliver_new_mail(&db, &recorder.host, std::slice::from_ref(&n)).await;
    assert_eq!(first, vec![Delivery::Shown]);
    assert_eq!(recorder.shown(), 1);

    // The claim is durable and per message.
    assert!(db.notify_seen(&acc, "m1").await.unwrap());
    assert!(!db.notify_claim(&acc, "m1", "t1").await.unwrap());

    let second = deliver_new_mail(&db, &recorder.host, std::slice::from_ref(&n)).await;
    assert_eq!(second, vec![Delivery::Duplicate]);
    assert_eq!(recorder.shown(), 1, "one message, one notification");
}

/// P8.4: a burst becomes one summary, not N banners — and replaying the burst
/// adds nothing.
#[tokio::test]
async fn p8_4_a_grouped_burst_notifies_once_and_does_not_double_notify() {
    let (_dir, db, acc) = fixture().await;
    let recorder = Recorder::new(true, true);
    let burst: Vec<Notice> = (0..5)
        .map(|i| notice(&acc, &format!("t{i}"), &format!("m{i}"), "Ada"))
        .collect();

    let first = deliver_new_mail(&db, &recorder.host, &burst).await;
    assert_eq!(
        first,
        vec![Delivery::Shown],
        "one summary, not five banners"
    );
    assert_eq!(recorder.shown(), 1);

    let second = deliver_new_mail(&db, &recorder.host, &burst).await;
    assert!(
        second.iter().all(|d| *d == Delivery::Duplicate),
        "{second:?}"
    );
    assert_eq!(recorder.shown(), 1, "a replayed burst is silent");

    // A new message in the same burst is the only thing that notifies again.
    let seventh = notice(&acc, "t9", "m9", "Ada");
    let third = deliver_new_mail(&db, &recorder.host, std::slice::from_ref(&seventh)).await;
    assert_eq!(third, vec![Delivery::Shown]);
    assert_eq!(recorder.shown(), 2);
}

/// P8.4: the filter is decided before anything is claimed, so a suppressed
/// message is not burned by a policy the user later relaxes.
#[tokio::test]
async fn p8_4_a_suppressed_message_is_not_claimed() {
    let (_dir, db, acc) = fixture().await;
    let recorder = Recorder::new(true, true);
    db.settings_set(serde_json::json!({ "notifications": "off" }))
        .await
        .unwrap();
    let n = notice(&acc, "t1", "m1", "Ada");
    let outcome = deliver_new_mail(&db, &recorder.host, std::slice::from_ref(&n)).await;
    assert_eq!(outcome, vec![Delivery::Suppressed]);
    assert_eq!(recorder.shown(), 0);
    assert!(
        !db.notify_seen(&acc, "m1").await.unwrap(),
        "suppression is not delivery"
    );

    // Turning notifications back on notifies about the message the policy had
    // never seen.
    db.settings_set(serde_json::json!({ "notifications": "inbox" }))
        .await
        .unwrap();
    let outcome = deliver_new_mail(&db, &recorder.host, std::slice::from_ref(&n)).await;
    assert_eq!(outcome, vec![Delivery::Shown]);
    assert_eq!(recorder.shown(), 1);
}

/// P8.4: hidden-subject mode keeps mailbox content out of the banner, and the
/// policy is read from stored settings rather than from the caller.
#[tokio::test]
async fn p8_4_hidden_subject_and_the_stored_policy_are_respected() {
    let (_dir, db, _acc) = fixture().await;
    db.settings_set(serde_json::json!({
        "notifications": "inbox",
        "notificationsHideSubject": true,
    }))
    .await
    .unwrap();
    let policy = Policy::from_settings(&db.settings_get().await.unwrap());
    assert!(policy.hide_subject);
    let (title, body) = Notice {
        subject: "Q3 payroll figures".into(),
        ..notice("a", "t1", "m1", "Ada")
    }
    .title_body(&policy);
    assert_eq!(title, "Ada");
    assert_eq!(body, "New message");
    assert!(!body.contains("payroll"));
}
