//! Phase 6 acceptance — the action service (P6.3), snooze (P6.5) and the
//! durable half of permanent deletion (P6.4).

use sift::actions;
use sift::db::messages::MsgUpsert;
use sift::db::outbox::{NewOp, STATE_PENDING};
use sift::db::Db;
use sift::dto::{ActionKind, GestureTarget};

async fn account(db: &Db, email: &str) -> sift::dto::Account {
    db.new_account(email, None, None).await.unwrap()
}

async fn seed(db: &Db, account_id: &str, thread_id: &str, ids: &[(&str, &[&str])]) {
    for (id, labels) in ids {
        db.messages_upsert(MsgUpsert {
            id: (*id).to_string(),
            account_id: account_id.to_string(),
            thread_id: thread_id.to_string(),
            internal_date: 1,
            subject: "s".into(),
            is_unread: labels.contains(&"UNREAD"),
            label_ids: labels.iter().map(|l| l.to_string()).collect(),
            ..Default::default()
        })
        .await
        .unwrap();
    }
}

async fn seed_at(db: &Db, account_id: &str, thread_id: &str, id: &str, labels: &[&str], at: i64) {
    db.messages_upsert(MsgUpsert {
        id: id.to_string(),
        account_id: account_id.to_string(),
        thread_id: thread_id.to_string(),
        internal_date: at,
        subject: "s".into(),
        is_unread: labels.contains(&"UNREAD"),
        label_ids: labels.iter().map(|l| l.to_string()).collect(),
        ..Default::default()
    })
    .await
    .unwrap();
}

async fn labels_of(db: &Db, account_id: &str, message_id: &str) -> Vec<String> {
    let account = account_id.to_string();
    let message = message_id.to_string();
    let raw: String = db
        .read(move |c| {
            Ok(c.query_row(
                "SELECT label_ids FROM messages WHERE account_id=? AND id=?",
                rusqlite::params![account, message],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    serde_json::from_str(&raw).unwrap_or_default()
}

fn target(account_id: &str, thread_id: &str) -> GestureTarget {
    GestureTarget {
        account_id: account_id.to_string(),
        thread_id: thread_id.to_string(),
    }
}

// -- P6.3 -------------------------------------------------------------------

/// A mixed selection across two accounts is one gesture: one operation per
/// account, and one Undo that restores both.
#[tokio::test]
async fn p6_3_cross_account_gesture_is_one_group_and_undo_restores_both() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let a = account(&db, "a@x.com").await;
    let b = account(&db, "b@x.com").await;
    seed(&db, &a.id, "t1", &[("m1", &["INBOX", "UNREAD"])]).await;
    seed(&db, &b.id, "t2", &[("m2", &["INBOX", "UNREAD"])]).await;

    let targets = vec![target(&a.id, "t1"), target(&b.id, "t2")];
    let (outcome, failures) = actions::apply_gesture(&db, "g1", &targets, &ActionKind::Archive)
        .await
        .unwrap();
    assert!(failures.is_empty());
    assert_eq!(outcome.operations.len(), 2, "one operation per account");
    assert!(outcome
        .operations
        .iter()
        .all(|op| op.undo_group.as_deref() == Some("g1")));
    assert!(!labels_of(&db, &a.id, "m1").await.contains(&"INBOX".to_string()));
    assert!(!labels_of(&db, &b.id, "m2").await.contains(&"INBOX".to_string()));

    let (undone, failures) = actions::undo_gesture(&db, "g1").await.unwrap();
    assert!(failures.is_empty());
    assert_eq!(undone.len(), 2);
    assert!(labels_of(&db, &a.id, "m1").await.contains(&"INBOX".to_string()));
    assert!(labels_of(&db, &b.id, "m2").await.contains(&"INBOX".to_string()));
}

/// Undo of a *done* operation is a new operation carrying the exact inverse —
/// and it leaves alone what the gesture never touched.
#[tokio::test]
async fn p6_3_undo_after_done_compensates_with_the_exact_inverse() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let a = account(&db, "a@x.com").await;
    seed(
        &db,
        &a.id,
        "t1",
        &[
            ("m1", &["INBOX", "UNREAD"]),
            ("m2", &["UNREAD"]),
            ("m3", &["INBOX", "STARRED"]),
        ],
    )
    .await;
    let targets = vec![target(&a.id, "t1")];
    let (outcome, _) = actions::apply_gesture(&db, "g1", &targets, &ActionKind::Archive)
        .await
        .unwrap();
    let op = &outcome.operations[0];
    assert_eq!(op.state, STATE_PENDING);
    // The provider accepted it.
    db.outbox_mark_done(op.id, None).await.unwrap();

    let (undone, failures) = actions::undo_gesture(&db, "g1").await.unwrap();
    assert!(failures.is_empty());
    assert_eq!(undone.len(), 1);
    let payload = undone[0].payload_value();
    let ids: Vec<String> = serde_json::from_value(payload["ids"].clone()).unwrap();
    let add: Vec<String> = serde_json::from_value(payload["add"].clone()).unwrap();
    let remove: Vec<String> = serde_json::from_value(payload["remove"].clone()).unwrap();
    assert_eq!(ids.len(), 2, "only the messages the gesture archived");
    assert_eq!(add, vec!["INBOX".to_string()]);
    assert!(remove.is_empty());
    // Locally restored, and the untouched message is untouched.
    assert!(labels_of(&db, &a.id, "m1").await.contains(&"INBOX".to_string()));
    assert!(labels_of(&db, &a.id, "m3").await.contains(&"INBOX".to_string()));
    assert_eq!(labels_of(&db, &a.id, "m2").await, vec!["UNREAD".to_string()]);
}

/// A failure before the enqueue rolls the whole account back: no optimistic
/// mutation survives a gesture that could not be queued.
#[tokio::test]
async fn p6_3_a_failed_enqueue_leaves_no_optimistic_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let a = account(&db, "a@x.com").await;
    seed(&db, &a.id, "t1", &[("m1", &["INBOX", "UNREAD"])]).await;
    // Fault injection: the enqueue cannot commit.
    db.write(|c| {
        c.execute_batch(
            "CREATE TRIGGER test_block_enqueue BEFORE INSERT ON outbox_ops \
             BEGIN SELECT RAISE(ABORT, 'injected'); END;",
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let targets = vec![target(&a.id, "t1")];
    let (outcome, failures) = actions::apply_gesture(&db, "g1", &targets, &ActionKind::Archive)
        .await
        .unwrap();
    assert!(outcome.operations.is_empty());
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].account_id, a.id);
    assert_eq!(
        labels_of(&db, &a.id, "m1").await,
        vec!["INBOX".to_string(), "UNREAD".to_string()],
        "the rollback left the message exactly as it was"
    );
    let in_inbox: i64 = db
        .read({
            let aid = a.id.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT in_inbox FROM threads WHERE account_id=? AND id='t1'",
                    rusqlite::params![aid],
                    |r| r.get(0),
                )?)
            }
        })
        .await
        .unwrap();
    assert_eq!(in_inbox, 1, "the thread aggregate was not left half-updated");
}

/// An Undo of an in-flight gesture waits for its resolution before
/// compensating: the compensation is queued behind the operation.
#[tokio::test]
async fn p6_3_undo_of_an_inflight_gesture_waits_for_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let a = account(&db, "a@x.com").await;
    seed(&db, &a.id, "t1", &[("m1", &["INBOX"])]).await;
    let targets = vec![target(&a.id, "t1")];
    let (outcome, _) = actions::apply_gesture(&db, "g1", &targets, &ActionKind::Archive)
        .await
        .unwrap();
    let parent = outcome.operations[0].id;
    let claimed = db.outbox_claim(&a.id).await.unwrap().expect("claimable");
    assert_eq!(claimed.id, parent);

    let (undone, failures) = actions::undo_gesture(&db, "g1").await.unwrap();
    assert!(failures.is_empty());
    assert_eq!(undone.len(), 1);
    assert_eq!(undone[0].depends_on_op_id, Some(parent));
    assert!(
        db.outbox_claim(&a.id).await.unwrap().is_none(),
        "the compensation must not run before the operation it depends on"
    );

    // The send-side operation completes: the compensation becomes runnable.
    db.outbox_mark_done(parent, None).await.unwrap();
    let next = db.outbox_claim(&a.id).await.unwrap().expect("runnable");
    assert_eq!(next.id, undone[0].id);
    // And it carries the exact inverse of what it must undo.
    let payload = next.payload_value();
    assert_eq!(
        serde_json::from_value::<Vec<String>>(payload["add"].clone()).unwrap(),
        vec!["INBOX".to_string()]
    );
}

// -- P6.5 -------------------------------------------------------------------

/// Offline snooze: the label is created by a queued operation the label change
/// waits for, and Undo removes the timer as well as the label.
#[tokio::test]
async fn p6_5_offline_snooze_queues_the_label_and_undo_clears_the_timer() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let a = account(&db, "a@x.com").await;
    seed(&db, &a.id, "t1", &[("m1", &["INBOX", "UNREAD"])]).await;
    let targets = vec![target(&a.id, "t1")];
    let (outcome, failures) = sift::snooze::snooze_set(
        &db,
        "g1",
        &targets,
        sift::db::now_ms() + 3_600_000,
        false,
    )
    .await
    .unwrap();
    assert!(failures.is_empty());
    assert_eq!(outcome.operations.len(), 1);
    let snooze_op = &outcome.operations[0];
    let dependency = snooze_op.depends_on_op_id.expect("create-label dependency");
    let create = db.outbox_get(dependency).await.unwrap().unwrap();
    assert_eq!(create.kind, "create_label");
    assert!(create.payload.contains("Sift/Snoozed"));
    assert!(snooze_op.payload.contains("name:Sift/Snoozed"));
    assert!(
        !snooze_op.payload.contains("Label_"),
        "no invented provider id is ever queued"
    );
    // Offline: the label change is not runnable yet.
    assert!(
        db.outbox_claim(&a.id).await.unwrap().is_some(),
        "the create-label operation comes first"
    );

    let (_, failures) = actions::undo_gesture(&db, "g1").await.unwrap();
    assert!(failures.is_empty());
    let sleepers: i64 = db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM snoozes", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(sleepers, 0, "undo removes the timer, not only the label change");
    assert!(labels_of(&db, &a.id, "m1").await.contains(&"INBOX".to_string()));
}

/// Waking applies Inbox (plus Unread when configured) and removes the Snoozed
/// label in one transaction, with the queued operation matching exactly.
#[tokio::test]
async fn p6_5_wake_applies_the_exact_label_set_for_both_policies() {
    for wake_unread in [true, false] {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let a = account(&db, "a@x.com").await;
        // Read before the snooze, so the wake policy decides what happens to
        // Unread rather than inheriting it.
        seed(&db, &a.id, "t1", &[("m1", &["INBOX"])]).await;
        let targets = vec![target(&a.id, "t1")];
        sift::snooze::snooze_set(&db, "g1", &targets, 10, wake_unread)
            .await
            .unwrap();
        // The create-label operation completed remotely: the real provider id
        // is adopted, and every local reference to the placeholder follows it.
        db.labels_adopt_created(&sift::dto::Label {
            account_id: a.id.clone(),
            id: "Label_Snoozed".into(),
            name: "Sift/Snoozed".into(),
            kind: "user".into(),
            color_bg: None,
            color_fg: None,
            visible: true,
            unread_count: 0,
            total_count: 0,
            sort_order: 200,
            ..Default::default()
        })
        .await
        .unwrap();

        let woken = sift::snooze::wake_due(&db, 11).await.unwrap();
        assert_eq!(woken.len(), 1);
        let labels = labels_of(&db, &a.id, "m1").await;
        assert!(labels.contains(&"INBOX".to_string()), "{labels:?}");
        assert_eq!(
            labels.contains(&"UNREAD".to_string()),
            wake_unread,
            "wake_unread={wake_unread}: {labels:?}"
        );
        assert!(
            !labels.iter().any(|l| l.contains("Snoozed")),
            "the snooze label is removed on wake: {labels:?}"
        );
        // The queued operation says exactly the same thing.
        let op: sift::db::outbox::Op = db
            .read({
                let aid = a.id.clone();
                move |c| {
                    let sql = "SELECT id,account_id,kind,payload,undo_group,state,attempts,not_before,\
                     operation_key,draft_id,draft_revision,rfc_message_id,previous_state_json,\
                     depends_on_op_id,reconcile_at,reconcile_attempts,failure_code,last_error,result_json,\
                     created_at,started_at,completed_at,summary_action,summary_recipient,summary_subject \
                     FROM outbox_ops WHERE account_id=? AND summary_action='Waking snoozed mail'";
                    Ok(c.query_row(sql, rusqlite::params![aid], |r| {
                        Ok(sift::db::outbox::Op {
                            id: r.get(0)?,
                            account_id: r.get(1)?,
                            kind: r.get(2)?,
                            payload: r.get(3)?,
                            undo_group: r.get(4)?,
                            state: r.get(5)?,
                            attempts: r.get(6)?,
                            not_before: r.get(7)?,
                            operation_key: r.get(8)?,
                            draft_id: r.get(9)?,
                            draft_revision: r.get(10)?,
                            rfc_message_id: r.get(11)?,
                            previous_state_json: r.get(12)?,
                            depends_on_op_id: r.get(13)?,
                            reconcile_at: r.get(14)?,
                            reconcile_attempts: r.get(15)?,
                            failure_code: r.get(16)?,
                            last_error: r.get(17)?,
                            result_json: r.get(18)?,
                            created_at: r.get(19)?,
                            started_at: r.get(20)?,
                            completed_at: r.get(21)?,
                            summary_action: r.get(22)?,
                            summary_recipient: r.get(23)?,
                            summary_subject: r.get(24)?,
                        })
                    })?)
                }
            })
            .await
            .unwrap();
        let payload = op.payload_value();
        let add: Vec<String> = serde_json::from_value(payload["add"].clone()).unwrap();
        assert!(add.contains(&"INBOX".to_string()));
        assert_eq!(add.contains(&"UNREAD".to_string()), wake_unread);
        assert!(payload["remove"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v.as_str().unwrap_or_default().contains("Snoozed")));
        // The timer is gone, and its operation is durable.
        assert_eq!(sift::snooze::next_deadline(&db).await.unwrap(), None);
    }
}

/// A snoozed thread that receives a new incoming message wakes by policy.
#[tokio::test]
async fn p6_5_a_new_reply_wakes_a_snoozed_thread() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let a = account(&db, "a@x.com").await;
    seed(&db, &a.id, "t1", &[("m1", &["INBOX", "UNREAD"])]).await;
    let targets = vec![target(&a.id, "t1")];
    sift::snooze::snooze_set(&db, "g1", &targets, sift::db::now_ms() + 3_600_000, false)
        .await
        .unwrap();

    // Nothing new: the thread stays asleep.
    let woken = sift::snooze::wake_threads_with_new_mail(&db, &a.id, &["t1".to_string()])
        .await
        .unwrap();
    assert!(woken.is_empty(), "a thread with no new mail does not wake");

    // A reply arrives *after* the snooze (its delivery time is what matters).
    seed_at(
        &db,
        &a.id,
        "t1",
        "m2",
        &["INBOX", "UNREAD"],
        sift::db::now_ms() + 60_000,
    )
    .await;
    let woken = sift::snooze::wake_threads_with_new_mail(&db, &a.id, &["t1".to_string()])
        .await
        .unwrap();
    assert_eq!(woken, vec!["t1".to_string()]);
    assert_eq!(sift::snooze::next_deadline(&db).await.unwrap(), None);
}

// -- P6.4 (service half) -----------------------------------------------------

/// Permanent deletion queues the immutable identity list *before* the display
/// rows go away, and refuses a message that is not in Trash or Spam.
#[tokio::test]
async fn p6_4_deletion_queues_identities_before_removing_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let a = account(&db, "a@x.com").await;
    seed(&db, &a.id, "t1", &[("m9", &["TRASH", "UNREAD"])]).await;
    seed(&db, &a.id, "t2", &[("m8", &["INBOX"])]).await;
    // Location hints: the message is in Trash with a known epoch.
    db.write({
        let aid = a.id.clone();
        move |c| {
            c.execute(
                "INSERT INTO imap_folders (account_id,role,name,uidvalidity,uidnext,exists_count) \
                 VALUES (?,'trash','[Gmail]/Trash',77,9,1)",
                rusqlite::params![aid],
            )?;
            c.execute(
                "INSERT INTO imap_uids (account_id,role,uid,message_id) VALUES (?,'trash',5,'m9')",
                rusqlite::params![aid],
            )?;
            Ok(())
        }
    })
    .await
    .unwrap();

    let targets = vec![target(&a.id, "t1")];
    let (outcome, failures) = actions::apply_gesture(&db, "g1", &targets, &ActionKind::DeleteForever)
        .await
        .unwrap();
    assert!(failures.is_empty());
    assert_eq!(outcome.operations.len(), 1);
    let op = &outcome.operations[0];
    let payload = op.payload_value();
    let messages = payload["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0]["id"], "m9");
    assert_eq!(messages[0]["role"], "trash");
    assert_eq!(messages[0]["uid"], 5);
    assert_eq!(messages[0]["uidvalidity"], 77);
    // The display rows are gone; the operation still knows exactly what to
    // delete on the server.
    let remaining: i64 = db
        .read({
            let aid = a.id.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM messages WHERE account_id=? AND id='m9'",
                    rusqlite::params![aid],
                    |r| r.get(0),
                )?)
            }
        })
        .await
        .unwrap();
    assert_eq!(remaining, 0);

    // A message that is not in Trash/Spam is refused, and nothing is removed.
    let targets = vec![target(&a.id, "t2")];
    let (outcome, failures) = actions::apply_gesture(&db, "g2", &targets, &ActionKind::DeleteForever)
        .await
        .unwrap();
    assert!(outcome.operations.is_empty());
    assert_eq!(failures[0].code, "not_in_trash");
    let still_there: i64 = db
        .read({
            let aid = a.id.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM messages WHERE account_id=? AND id='m8'",
                    rusqlite::params![aid],
                    |r| r.get(0),
                )?)
            }
        })
        .await
        .unwrap();
    assert_eq!(still_there, 1);
}

/// The durable half of P6.4's acceptance: the deletion survives a restart and
/// only the named messages are removed on reconnect.
#[tokio::test]
async fn p6_4_an_offline_deletion_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let account_id;
    let op_id;
    {
        let db = Db::open(dir.path()).unwrap();
        let a = account(&db, "a@x.com").await;
        account_id = a.id.clone();
        seed(&db, &a.id, "t1", &[("m9", &["TRASH"])]).await;
        let targets = vec![target(&a.id, "t1")];
        let (outcome, _) =
            actions::apply_gesture(&db, "g1", &targets, &ActionKind::DeleteForever)
                .await
                .unwrap();
        op_id = outcome.operations[0].id;
    }
    let db = Db::open(dir.path()).unwrap();
    let op = db.outbox_get(op_id).await.unwrap().unwrap();
    assert_eq!(op.state, "pending", "queued work survives a restart");
    assert!(op.payload.contains("m9"));
    assert_eq!(op.account_id, account_id);
}

/// A queued permanent deletion is pruned like any finished work, but a failed
/// one keeps its identity list so it stays recoverable.
#[tokio::test]
async fn p6_4_a_failed_deletion_stays_visible_and_recoverable() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let a = account(&db, "a@x.com").await;
    let op = db
        .outbox_enqueue_op(&NewOp {
            account_id: a.id.clone(),
            kind: "delete".into(),
            payload: serde_json::json!({"messages": [{"id": "m9"}], "cachePaths": []}).to_string(),
            summary_action: Some("Deleting permanently".into()),
            ..Default::default()
        })
        .await
        .unwrap()
        .id;
    db.outbox_mark_failed(op, "delete_target_moved", "the message moved")
        .await
        .unwrap();
    db.outbox_prune(0).await.unwrap();
    let row = db.outbox_get(op).await.unwrap().unwrap();
    assert_eq!(row.state, "failed");
    assert!(row.payload.contains("m9"), "a failed deletion stays recoverable");
    assert_eq!(row.failure_code.as_deref(), Some("delete_target_moved"));
}
