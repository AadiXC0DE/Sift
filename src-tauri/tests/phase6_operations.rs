//! Phase 6 acceptance — the durable operation state machine (P6.1), Undo Send
//! (P6.2) and bounded storage (P6.6).
//!
//! Every test here asserts an observable contract: what the provider was asked
//! to do, what state the operation is in, and what the UI would be told.
#![allow(clippy::await_holding_lock)] // the shared env server is serialized on purpose

use sift::db::outbox::{NewOp, RECONCILE_DELAYS_MS};
use sift::db::Db;
use sift::dto::Address;
use sift::provider::gmail::api::GmailApiProvider;
use sift::provider::gmail::client::GmailClient;

static ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn addr(email: &str) -> Address {
    Address {
        n: None,
        e: email.into(),
        me: None,
    }
}

async fn account(db: &Db) -> sift::dto::Account {
    db.new_account("ada@x.com", None, None).await.unwrap()
}

fn provider(account_id: &str) -> GmailApiProvider {
    GmailApiProvider::new(account_id.to_string(), GmailClient::new("t".into()))
}

async fn state_of(db: &Db, op_id: i64) -> String {
    db.outbox_get(op_id).await.unwrap().unwrap().state
}

/// The first reconciliation is scheduled five seconds after a failure. Tests
/// do not wait: they age the schedule instead, which is exactly what the clock
/// would do.
async fn make_reconcile_due(db: &Db, op_id: i64) {
    db.write(move |c| {
        c.execute(
            "UPDATE outbox_ops SET reconcile_at=1 WHERE id=?",
            rusqlite::params![op_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}

// -- P6.1 -------------------------------------------------------------------

/// Two drainers racing for one operation: exactly one claims it, and the
/// provider sees exactly one call.
#[tokio::test]
async fn p6_1_two_drainers_claim_one_operation_once() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path_regex(".*/messages/batchModify"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_string("{}")
                .set_delay(std::time::Duration::from_millis(150)),
        )
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = account(&db).await;
    let op = db
        .outbox_enqueue(
            &acc.id,
            "modify_labels",
            "{\"ids\":[\"m1\"],\"add\":[],\"remove\":[\"INBOX\"]}",
            None,
            0,
        )
        .await
        .unwrap();
    let provider = provider(&acc.id);
    let (a, b) = tokio::join!(
        sift::outbox::drain_one(&db, &provider, &acc.id, true),
        sift::outbox::drain_one(&db, &provider, &acc.id, true),
    );
    let worked = [a.unwrap(), b.unwrap()];
    assert_eq!(
        worked.iter().filter(|w| **w).count(),
        1,
        "one drainer claims the operation; the other finds nothing"
    );
    assert_eq!(state_of(&db, op).await, "done");
    std::env::remove_var("SIFT_GMAIL_BASE");
}

/// Cancel versus claim: the claim wins a row that is already in flight, and a
/// cancelled row is never claimable.
#[tokio::test]
async fn p6_1_cancel_versus_claim_has_one_winner() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = account(&db).await;
    let claimed = db
        .outbox_enqueue(&acc.id, "modify_labels", "{\"ids\":[\"m1\"]}", None, 0)
        .await
        .unwrap();
    assert!(db.outbox_claim(&acc.id).await.unwrap().is_some());
    assert!(
        !db.outbox_cancel(claimed, "undo", "too late").await.unwrap(),
        "a claimed operation cannot be cancelled back"
    );
    assert_eq!(state_of(&db, claimed).await, "inflight");

    let cancelled = db
        .outbox_enqueue(&acc.id, "modify_labels", "{\"ids\":[\"m2\"]}", None, 0)
        .await
        .unwrap();
    assert!(db
        .outbox_cancel(cancelled, "undo", "not yet sent")
        .await
        .unwrap());
    assert!(
        db.outbox_claim(&acc.id).await.unwrap().is_none(),
        "a cancelled operation is never claimed"
    );
    assert_eq!(state_of(&db, cancelled).await, "cancelled");
}

/// A send interrupted while it was being handed to Gmail must never be
/// resubmitted on startup (P6.1: no duplicate send), and its draft must not be
/// called sent either.
#[tokio::test]
async fn p6_1_crash_after_data_leaves_the_send_uncertain_and_unsent() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    // Nothing may be delivered: the acceptance is unknown, not retryable.
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/gmail/v1/users/me/messages/send"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"id": "mSent", "threadId": "t1"})),
        )
        .expect(0)
        .mount(&server)
        .await;
    // No Sent copy: reconciliation finds nothing and stays uncertain.
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/gmail/v1/users/me/messages"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({})))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let (op_id, draft_id) = {
        let db = Db::open(dir.path()).unwrap();
        let acc = account(&db).await;
        let draft = db
            .drafts_upsert(
                &sift::dto::Draft {
                    account_id: acc.id.clone(),
                    to_json: vec![addr("bob@y.org")],
                    subject: "half sent".into(),
                    body_html: "<p>hi</p>".into(),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let identity = sift::outgoing::Identity {
            email: "ada@x.com".into(),
            display_name: None,
        };
        let prepared =
            sift::outgoing::prepare(dir.path(), &draft, &identity, 1_700_000_000).unwrap();
        let handle = db
            .drafts_enqueue_send(
                &prepared,
                &sift::db::drafts::SendSchedule::now(sift::db::now_ms()),
                false,
            )
            .await
            .unwrap();
        // The app dies between claiming the operation and writing the outcome.
        db.outbox_claim(&acc.id).await.unwrap();
        (handle.op_id, draft.local_id)
    };

    let db = Db::open(dir.path()).unwrap(); // startup recovery
    let op = db.outbox_get(op_id).await.unwrap().unwrap();
    assert_eq!(
        op.state, "uncertain",
        "an interrupted send is never pending again"
    );
    assert_eq!(op.reconcile_attempts, 0);
    assert!(op.reconcile_at.is_some(), "reconciliation is scheduled");

    let draft = db.drafts_get(&draft_id).await.unwrap().unwrap();
    assert_ne!(draft.state, "sent", "not sent without proof");

    // Drain: reconciliation runs, finds nothing, and the operation stays
    // uncertain with the next check scheduled.
    let acc_id = op.account_id.clone();
    let provider = provider(&acc_id);
    make_reconcile_due(&db, op_id).await;
    assert!(sift::outbox::drain_one(&db, &provider, &acc_id, true)
        .await
        .unwrap());
    let op = db.outbox_get(op_id).await.unwrap().unwrap();
    assert_eq!(op.state, "uncertain");
    assert_eq!(op.reconcile_attempts, 1);
    assert!(op.reconcile_at.is_some(), "the next check is scheduled");
    std::env::remove_var("SIFT_GMAIL_BASE");
}

/// A Sent copy with the same Message-ID proves delivery: the operation becomes
/// done and records the provider identity as evidence.
#[tokio::test]
async fn p6_1_uncertain_send_reconciles_to_done_by_rfc_message_id() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/gmail/v1/users/me/messages"))
        .and(wiremock::matchers::query_param(
            "q",
            "rfc822msgid:sift-deadbeef@x.com",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"messages": [{"id": "m9", "threadId": "t9"}]})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = account(&db).await;
    let op = db
        .outbox_enqueue(
            &acc.id,
            "send",
            "{\"rfcMessageId\":\"<sift-deadbeef@x.com>\"}",
            None,
            0,
        )
        .await
        .unwrap();
    db.outbox_set(op, "inflight", 1, 0, None).await.unwrap();
    db.outbox_mark_uncertain(op, "send_uncertain", "connection lost after DATA")
        .await
        .unwrap();

    let provider = provider(&acc.id);
    make_reconcile_due(&db, op).await;
    assert!(sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap());
    let op = db.outbox_get(op).await.unwrap().unwrap();
    assert_eq!(op.state, "done");
    let result = op.result_json.unwrap_or_default();
    assert!(
        result.contains("m9"),
        "the receipt names the sent copy: {result}"
    );
    std::env::remove_var("SIFT_GMAIL_BASE");
}

/// Retrying an uncertain send is a decision the user makes, never one Sift
/// makes on its own.
#[tokio::test]
async fn p6_1_retry_of_an_uncertain_send_requires_acknowledgement() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = account(&db).await;
    let op = db
        .outbox_enqueue(&acc.id, "send", "{\"rfcMessageId\":\"<x@y>\"}", None, 0)
        .await
        .unwrap();
    db.outbox_mark_uncertain(op, "send_uncertain", "unknown")
        .await
        .unwrap();
    let err = db.outbox_retry(op, false).await.unwrap_err();
    let typed = err
        .downcast_ref::<sift::errors::SiftError>()
        .expect("typed");
    assert_eq!(typed.code(), "acknowledge_duplicate_risk");
    assert_eq!(state_of(&db, op).await, "uncertain");

    let retried = db.outbox_retry(op, true).await.unwrap();
    assert_eq!(retried.state, "pending");
    assert!(retried.reconcile_at.is_none());
}

/// The bounded reconciliation schedule ends, and the operation then waits for
/// the user instead of looping forever.
#[tokio::test]
async fn p6_1_reconciliation_is_bounded_and_then_waits_for_the_user() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = account(&db).await;
    let op = db
        .outbox_enqueue(&acc.id, "send", "{\"rfcMessageId\":\"<x@y>\"}", None, 0)
        .await
        .unwrap();
    db.outbox_mark_uncertain(op, "send_uncertain", "unknown")
        .await
        .unwrap();
    for attempt in 0..RECONCILE_DELAYS_MS.len() {
        make_reconcile_due(&db, op).await;
        let claimed = db.outbox_claim_reconcile(&acc.id).await.unwrap();
        assert!(claimed.is_some(), "check {attempt} is due");
        let row = claimed.unwrap();
        assert_eq!(row.reconcile_attempts, attempt as i64 + 1);
        db.outbox_reschedule_reconcile(row.id, row.reconcile_attempts)
            .await
            .unwrap();
    }
    let exhausted = db.outbox_get(op).await.unwrap().unwrap();
    assert!(exhausted.reconcile_at.is_none(), "the schedule is over");
    assert!(exhausted.requires_duplicate_ack());
    assert!(db.outbox_claim_reconcile(&acc.id).await.unwrap().is_none());
}

// -- P6.2 -------------------------------------------------------------------

/// Double Send produces one operation for the revision; Undo before the
/// deadline returns the real draft with everything it had.
#[tokio::test]
async fn p6_2_double_send_is_one_operation_and_undo_returns_the_draft() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = account(&db).await;
    let draft = db
        .drafts_upsert(
            &sift::dto::Draft {
                account_id: acc.id.clone(),
                to_json: vec![addr("bob@y.org")],
                cc_json: vec![addr("cc@y.org")],
                subject: "undo me".into(),
                body_html: "<p>body</p>".into(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    let identity = sift::outgoing::Identity {
        email: "ada@x.com".into(),
        display_name: None,
    };
    let prepared = sift::outgoing::prepare(dir.path(), &draft, &identity, 1_700_000_000).unwrap();
    let first = db
        .drafts_enqueue_send(
            &prepared,
            &sift::db::drafts::SendSchedule::now(sift::db::now_ms() + 60_000),
            false,
        )
        .await
        .unwrap();
    let second = db
        .drafts_enqueue_send(
            &prepared,
            &sift::db::drafts::SendSchedule::now(sift::db::now_ms() + 60_000),
            false,
        )
        .await
        .unwrap();
    assert_eq!(first.op_id, second.op_id, "one operation for one revision");
    let count: i64 = db
        .read({
            let a = acc.id.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM outbox_ops WHERE account_id=? AND kind='send'",
                    rusqlite::params![a],
                    |r| r.get(0),
                )?)
            }
        })
        .await
        .unwrap();
    assert_eq!(count, 1);

    let queued = db.drafts_get(&draft.local_id).await.unwrap().unwrap();
    assert_eq!(queued.state, "queued");

    let reopened = db.drafts_cancel_send_detailed(first.op_id).await.unwrap();
    match reopened {
        sift::db::drafts::SendCancel::Cancelled(draft) => {
            assert_eq!(draft.state, "editing");
            assert_eq!(draft.body_html, "<p>body</p>");
            assert_eq!(draft.to_json, vec![addr("bob@y.org")]);
            assert_eq!(draft.cc_json, vec![addr("cc@y.org")]);
            assert_eq!(draft.rfc_message_id, queued.rfc_message_id);
            assert_eq!(draft.revision, queued.revision, "the same frozen revision");
        }
        other => panic!("undo must reopen the draft, got {other:?}"),
    }
    // A second undo has nothing left to cancel.
    match db.drafts_cancel_send_detailed(first.op_id).await.unwrap() {
        sift::db::drafts::SendCancel::TooLate { state } => assert_eq!(state, "cancelled"),
        other => panic!("expected TooLate, got {other:?}"),
    }
    // And the same revision can be sent again as a fresh operation.
    let again = db
        .drafts_enqueue_send(
            &prepared,
            &sift::db::drafts::SendSchedule::now(sift::db::now_ms() + 60_000),
            false,
        )
        .await
        .unwrap();
    assert_ne!(again.op_id, first.op_id);
}

/// Undo works after a restart while the operation is still pending, and an
/// offline drain changes nothing.
#[tokio::test]
async fn p6_2_undo_works_after_restart_while_pending_and_offline() {
    let dir = tempfile::tempdir().unwrap();
    let (op_id, draft_id) = {
        let db = Db::open(dir.path()).unwrap();
        let acc = account(&db).await;
        let draft = db
            .drafts_upsert(
                &sift::dto::Draft {
                    account_id: acc.id.clone(),
                    to_json: vec![addr("bob@y.org")],
                    subject: "restart".into(),
                    body_html: "<p>x</p>".into(),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let identity = sift::outgoing::Identity {
            email: "ada@x.com".into(),
            display_name: None,
        };
        let prepared =
            sift::outgoing::prepare(dir.path(), &draft, &identity, 1_700_000_000).unwrap();
        let handle = db
            .drafts_enqueue_send(
                &prepared,
                &sift::db::drafts::SendSchedule::now(sift::db::now_ms() + 30_000),
                false,
            )
            .await
            .unwrap();
        (handle.op_id, draft.local_id)
    };
    let db = Db::open(dir.path()).unwrap();
    let acc_id = db
        .outbox_get(op_id)
        .await
        .unwrap()
        .unwrap()
        .account_id
        .clone();
    let provider = provider(&acc_id);
    // Offline: nothing drains, the operation keeps its pending state.
    assert!(!sift::outbox::drain_one(&db, &provider, &acc_id, false)
        .await
        .unwrap());
    assert_eq!(state_of(&db, op_id).await, "pending");

    match db.drafts_cancel_send_detailed(op_id).await.unwrap() {
        sift::db::drafts::SendCancel::Cancelled(draft) => {
            assert_eq!(draft.local_id, draft_id);
            assert_eq!(draft.state, "editing");
        }
        other => panic!("an undo after restart must still work: {other:?}"),
    }
}

/// A cancelled send must never leave its archive-after-send behind, and a
/// send that is already claimed cannot be recalled.
#[tokio::test]
async fn p6_2_claimed_send_cannot_be_undone_and_its_archive_never_runs() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/gmail/v1/users/me/messages/send"))
        // A definite rejection: a 5xx would be a transient failure that stays
        // queued, which is a different assertion.
        .respond_with(wiremock::ResponseTemplate::new(400).set_body_string("nope"))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = account(&db).await;
    let draft = db
        .drafts_upsert(
            &sift::dto::Draft {
                account_id: acc.id.clone(),
                thread_id: Some("t1".into()),
                to_json: vec![addr("bob@y.org")],
                subject: "archive me".into(),
                body_html: "<p>x</p>".into(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    // The thread being replied to.
    db.messages_upsert(sift::db::messages::MsgUpsert {
        id: "m1".into(),
        account_id: acc.id.clone(),
        thread_id: "t1".into(),
        internal_date: 1,
        subject: "orig".into(),
        label_ids: vec!["INBOX".into()],
        ..Default::default()
    })
    .await
    .unwrap();
    let identity = sift::outgoing::Identity {
        email: "ada@x.com".into(),
        display_name: None,
    };
    let prepared = sift::outgoing::prepare(dir.path(), &draft, &identity, 1_700_000_000).unwrap();
    let handle = db
        .drafts_enqueue_send(&prepared, &sift::db::drafts::SendSchedule::now(0), true)
        .await
        .unwrap();
    let archive = db
        .read({
            let a = acc.id.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT id FROM outbox_ops WHERE account_id=? AND operation_key LIKE 'archive-after-send:%'",
                    rusqlite::params![a],
                    |r| r.get::<_, i64>(0),
                )?)
            }
        })
        .await
        .unwrap();
    assert_eq!(state_of(&db, archive).await, "pending");

    // The send fails. Draft is not sent, and the archive must not have run —
    // it is a dependent of the send, so a failure cancels it.
    let provider = provider(&acc.id);
    let _ = sift::outbox::drain_one(&db, &provider, &acc.id, true).await;
    assert_eq!(state_of(&db, handle.op_id).await, "failed");
    let draft = db.drafts_get(&draft.local_id).await.unwrap().unwrap();
    assert_ne!(draft.state, "sent");
    assert_eq!(
        state_of(&db, archive).await,
        "cancelled",
        "send-and-archive waits for a successful send"
    );
    let labels: Vec<String> = db
        .read({
            let a = acc.id.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT label_ids FROM messages WHERE account_id=? AND id='m1'",
                    rusqlite::params![a],
                    |r| r.get::<_, String>(0),
                )?)
            }
        })
        .await
        .unwrap()
        .pipe_parse();
    assert!(
        labels.contains(&"INBOX".to_string()),
        "the source thread was not archived"
    );
    std::env::remove_var("SIFT_GMAIL_BASE");
}

/// Undo of an operation that is already inflight reports the honest state.
#[tokio::test]
async fn p6_2_undo_reports_the_state_it_lost_to() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = account(&db).await;
    let draft = db
        .drafts_upsert(
            &sift::dto::Draft {
                account_id: acc.id.clone(),
                to_json: vec![addr("bob@y.org")],
                subject: "gone".into(),
                body_html: "<p>x</p>".into(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    let identity = sift::outgoing::Identity {
        email: "ada@x.com".into(),
        display_name: None,
    };
    let prepared = sift::outgoing::prepare(dir.path(), &draft, &identity, 1_700_000_000).unwrap();
    let handle = db
        .drafts_enqueue_send(&prepared, &sift::db::drafts::SendSchedule::now(0), false)
        .await
        .unwrap();
    assert!(db.outbox_claim(&acc.id).await.unwrap().is_some());
    match db.drafts_cancel_send_detailed(handle.op_id).await.unwrap() {
        sift::db::drafts::SendCancel::TooLate { state } => assert_eq!(state, "inflight"),
        other => panic!("expected TooLate, got {other:?}"),
    }
}

// -- P6.6 -------------------------------------------------------------------

/// The Outbox panel never receives a payload: a queued megabyte stays in
/// SQLite, and the page stays small.
#[tokio::test]
async fn p6_6_the_list_page_never_carries_the_payload() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = account(&db).await;
    let big = "A".repeat(1_000_000);
    let payload = serde_json::json!({"raw": big, "recipientSummary": "bob@y.org"}).to_string();
    db.outbox_enqueue_op(&NewOp {
        account_id: acc.id.clone(),
        kind: "send".into(),
        payload,
        summary_action: Some("Sending".into()),
        summary_recipient: Some("bob@y.org".into()),
        summary_subject: Some("a big one".into()),
        ..Default::default()
    })
    .await
    .unwrap();

    let page = db
        .outbox_list(std::slice::from_ref(&acc.id), &[], None, 50)
        .await
        .unwrap();
    assert_eq!(page.operations.len(), 1);
    let json = serde_json::to_string(
        &page
            .operations
            .iter()
            .map(sift::dto::OutboxOpSummary::from)
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(!json.contains("AAAA"), "the payload never reaches the UI");
    assert!(
        json.len() < 2_000,
        "the summary stays small: {} bytes",
        json.len()
    );
    assert!(json.contains("bob@y.org"));
}

/// Counts by kind and state stay cheap at 10 000 queued label operations.
#[tokio::test]
async fn p6_6_ten_thousand_label_operations_stay_countable() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = account(&db).await;
    let account_id = acc.id.clone();
    let started = std::time::Instant::now();
    db.write(move |c| {
        let tx = c.unchecked_transaction()?;
        {
            let mut st = tx.prepare(
                "INSERT INTO outbox_ops (account_id,kind,payload,state,attempts,not_before,created_at,summary_action) \
                 VALUES (?, 'modify_labels', ?, 'pending', 0, 0, ?, 'Archiving')",
            )?;
            for i in 0..10_000 {
                st.execute(rusqlite::params![
                    account_id,
                    format!("{{\"ids\":[\"m{i}\"],\"add\":[],\"remove\":[\"INBOX\"]}}"),
                    i as i64
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    })
    .await
    .unwrap();
    let queued = started.elapsed();

    let count_started = std::time::Instant::now();
    let page = db
        .outbox_list(std::slice::from_ref(&acc.id), &[], None, 50)
        .await
        .unwrap();
    let summary = db.outbox_summary(&acc.id).await.unwrap();
    let counted = count_started.elapsed();
    assert_eq!(page.operations.len(), 50);
    assert_eq!(page.counts.pending, 10_000);
    assert_eq!(page.total, 10_000);
    assert_eq!(summary, vec![("Archiving".to_string(), 10_000)]);
    assert!(
        counted.as_millis() < 1_000,
        "counting must not scan payloads: {} ms (insert {queued:?})",
        counted.as_millis()
    );
}

/// Failed and uncertain counts are durable: they survive a restart.
#[tokio::test]
async fn p6_6_failed_and_uncertain_counts_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let account_id;
    {
        let db = Db::open(dir.path()).unwrap();
        let acc = account(&db).await;
        account_id = acc.id.clone();
        let failed = db
            .outbox_enqueue(&acc.id, "modify_labels", "{}", None, 0)
            .await
            .unwrap();
        db.outbox_mark_failed(failed, "imap_error", "server said no")
            .await
            .unwrap();
        let uncertain = db
            .outbox_enqueue(&acc.id, "send", "{\"rfcMessageId\":\"<x@y>\"}", None, 0)
            .await
            .unwrap();
        db.outbox_mark_uncertain(uncertain, "send_uncertain", "unknown")
            .await
            .unwrap();
    }
    let db = Db::open(dir.path()).unwrap();
    let counts = db.outbox_state_counts(&account_id).await.unwrap();
    assert_eq!(counts.failed, 1);
    assert_eq!(counts.uncertain, 1);
    assert_eq!(db.outbox_failed_count(&account_id).await.unwrap(), 2);
}

/// Pruning drops the payload of finished work after the window and never
/// touches anything that is still pending, failed, uncertain or scheduled.
#[tokio::test]
async fn p6_6_prune_releases_only_finished_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = account(&db).await;
    let old = sift::db::now_ms() - sift::db::outbox::OP_PAYLOAD_RETENTION_MS - 1_000;
    let done = db
        .outbox_enqueue(&acc.id, "modify_labels", "{\"ids\":[\"m1\"]}", None, 0)
        .await
        .unwrap();
    let cancelled = db
        .outbox_enqueue(&acc.id, "modify_labels", "{\"ids\":[\"m2\"]}", None, 0)
        .await
        .unwrap();
    db.outbox_mark_done(done, None).await.unwrap();
    db.outbox_cancel(cancelled, "undone", "no").await.unwrap();
    let failed = db
        .outbox_enqueue(&acc.id, "modify_labels", "{\"ids\":[\"m3\"]}", None, 0)
        .await
        .unwrap();
    db.outbox_mark_failed(failed, "imap_error", "no")
        .await
        .unwrap();
    // Age the finished ones past the retention window.
    db.write(move |c| {
        c.execute(
            "UPDATE outbox_ops SET completed_at=? WHERE id IN (?,?)",
            rusqlite::params![old, done, cancelled],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    db.outbox_prune(sift::db::outbox::OP_PAYLOAD_RETENTION_MS)
        .await
        .unwrap();
    assert_eq!(db.outbox_get(done).await.unwrap().unwrap().payload, "");
    assert_eq!(db.outbox_get(cancelled).await.unwrap().unwrap().payload, "");
    assert_eq!(
        db.outbox_get(failed).await.unwrap().unwrap().payload,
        "{\"ids\":[\"m3\"]}",
        "a failure keeps its payload: the user may need to retry it"
    );
    // The summaries survive, so the counters are still right.
    let counts = db.outbox_state_counts(&acc.id).await.unwrap();
    assert_eq!(counts.done, 1);
    assert_eq!(counts.cancelled, 1);
    assert_eq!(counts.failed, 1);
}

/// A newer conflicting label change cannot overtake an older pending one for
/// the same message.
#[tokio::test]
async fn p6_6_a_newer_action_cannot_overtake_an_older_one() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = account(&db).await;
    let older = db
        .outbox_enqueue(
            &acc.id,
            "modify_labels",
            "{\"ids\":[\"m1\"],\"add\":[\"STARRED\"],\"remove\":[]}",
            None,
            sift::db::now_ms() + 60_000,
        )
        .await
        .unwrap();
    let newer = db
        .outbox_enqueue(
            &acc.id,
            "modify_labels",
            "{\"ids\":[\"m1\"],\"add\":[],\"remove\":[\"STARRED\"]}",
            None,
            0,
        )
        .await
        .unwrap();
    assert!(
        db.outbox_claim(&acc.id).await.unwrap().is_none(),
        "the newer op must wait for the older one on the same message"
    );

    // Once the older one is gone (failed), the newer becomes claimable.
    db.outbox_mark_failed(older, "imap_error", "no")
        .await
        .unwrap();
    let claimed = db.outbox_claim(&acc.id).await.unwrap().expect("claimable");
    assert_eq!(claimed.id, newer);
}

/// A trait used by one assertion above; kept next to it for clarity.
trait ParseLabels {
    fn pipe_parse(self) -> Vec<String>;
}

impl ParseLabels for String {
    fn pipe_parse(self) -> Vec<String> {
        serde_json::from_str(&self).unwrap_or_default()
    }
}
