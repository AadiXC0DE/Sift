//! P5.1/P5.2 — draft lifecycle, remote draft synchronization and the local
//! truth surviving every failure mode.
//!
//! The remote half runs against a wiremock Gmail endpoint (the same fake the
//! REST tests use), so these assert the actual HTTP calls Sift makes: one
//! create for a burst of edits, an in-place update afterwards, and never a
//! delete that could erase someone else's newer revision.

#![allow(clippy::await_holding_lock)] // tests serialize env-var servers via a shared mutex (intentional)
static ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    // A panic in a sibling test poisons the lock; the env vars are still ours
    // to serialize, so recover the guard instead of failing every later test.
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

use sift::dto::{Address, AttachmentRef, Draft};
use sift::provider::gmail::api::GmailApiProvider;
use sift::provider::gmail::client::GmailClient;

fn addr(email: &str) -> Address {
    Address {
        n: None,
        e: email.into(),
        me: None,
    }
}

/// P5.1: an edit made while offline is local, complete and durable across a
/// restart — recipients, body, attachments and identity included.
#[tokio::test]
async fn p51_offline_draft_survives_restart_with_every_field() {
    let dir = tempfile::tempdir().unwrap();
    let staged = dir.path().join("compose-cache").join("d1");
    std::fs::create_dir_all(&staged).unwrap();
    let file = staged.join("invoice.pdf");
    std::fs::write(&file, b"%PDF-1.4 bytes").unwrap();

    let local_id = {
        let db = sift::db::Db::open(dir.path()).unwrap();
        let acc = db.new_account("ada@x.com", None, None).await.unwrap();
        let saved = db
            .drafts_upsert(
                &Draft {
                    account_id: acc.id.clone(),
                    from_email: Some("ada@x.com".into()),
                    to_json: vec![addr("bob@y.org")],
                    cc_json: vec![addr("carol@y.org")],
                    bcc_json: vec![addr("dan@y.org")],
                    subject: "Offline draft".into(),
                    body_html: "<p>written offline</p>".into(),
                    attachments_json: vec![AttachmentRef {
                        name: "invoice.pdf".into(),
                        mime: "application/pdf".into(),
                        size: 14,
                        path: file.to_string_lossy().into_owned(),
                    }],
                    mode: "new".into(),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        // The remote sync is queued but never runs: this account is offline.
        assert_eq!(saved.revision, 1);
        assert!(saved.has_unsaved_revision());
        saved.local_id
    };

    // Restart: reopen the same data directory.
    let db = sift::db::Db::open(dir.path()).unwrap();
    let restored = db.drafts_get(&local_id).await.unwrap().unwrap();
    assert_eq!(restored.subject, "Offline draft");
    assert_eq!(restored.body_html, "<p>written offline</p>");
    assert_eq!(restored.to_json[0].e, "bob@y.org");
    assert_eq!(restored.cc_json[0].e, "carol@y.org");
    assert_eq!(restored.bcc_json[0].e, "dan@y.org");
    assert_eq!(restored.from_email.as_deref(), Some("ada@x.com"));
    assert_eq!(restored.state, "editing");
    assert_eq!(restored.attachments_json.len(), 1);
    assert!(std::path::Path::new(&restored.attachments_json[0].path).exists());
    let page = db.drafts_list(&[], None, 10).await.unwrap();
    assert_eq!(page.drafts.len(), 1);
}

/// P5.2: a burst of edits produces exactly one Gmail draft — one create, and
/// updates in place after that, never a second draft.
#[tokio::test]
async fn p52_debounced_sync_creates_once_then_updates_in_place() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/gmail/v1/users/me/drafts"))
        .respond_with(|req: &wiremock::Request| {
            let body: serde_json::Value =
                serde_json::from_str(&String::from_utf8_lossy(&req.body)).unwrap_or_default();
            assert!(
                body["message"]["raw"]
                    .as_str()
                    .is_some_and(|r| !r.is_empty()),
                "the draft content must be sent as raw MIME"
            );
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "draft-1",
                "message": {"id": "msg-1", "threadId": "t1"}
            }))
        })
        .expect(1)
        .mount(&server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("PUT"))
        .and(wiremock::matchers::path(
            "/gmail/v1/users/me/drafts/draft-1",
        ))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "draft-1",
                "message": {"id": "msg-2", "threadId": "t1"}
            })),
        )
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let db = sift::db::Db::open(dir.path()).unwrap();
    let acc = db.new_account("ada@x.com", None, None).await.unwrap();
    let provider = GmailApiProvider::new(acc.id.clone(), GmailClient::new("t".into()));

    let mut saved = db
        .drafts_upsert(
            &Draft {
                account_id: acc.id.clone(),
                to_json: vec![addr("bob@y.org")],
                subject: "S".into(),
                body_html: "<p>1</p>".into(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    // Three keystrokes in a row: three saves, one queued sync.
    for body in ["<p>12</p>", "<p>123</p>"] {
        saved = db
            .drafts_upsert(
                &Draft {
                    body_html: body.into(),
                    ..saved.clone()
                },
                Some(saved.revision),
            )
            .await
            .unwrap();
        db.outbox_enqueue_draft_sync(&acc.id, &saved.local_id, saved.revision)
            .await
            .unwrap();
    }
    assert_eq!(
        db.outbox_draft_sync_count(&saved.local_id).await.unwrap(),
        1,
        "a burst of edits coalesces into one queued sync"
    );
    let page = db
        .drafts_list(std::slice::from_ref(&acc.id), None, 10)
        .await
        .unwrap();
    assert_eq!(
        page.drafts.len(),
        1,
        "one local draft, even after three saves"
    );

    // The debounced op is not due immediately.
    assert!(!sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap());
    // Make it due; the sync pushes the newest revision.
    let op: i64 = db
        .read(move |c| {
            Ok(c.query_row(
                "SELECT id FROM outbox_ops WHERE kind='draft_sync'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    db.outbox_set(op, "pending", 0, 0, None).await.unwrap();
    assert!(sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap());
    let synced = db.drafts_get(&saved.local_id).await.unwrap().unwrap();
    assert_eq!(synced.remote_draft_id.as_deref(), Some("draft-1"));
    assert_eq!(synced.saved_revision, synced.revision);
    assert!(!synced.has_unsaved_revision());
    assert_eq!(synced.remote_message_id.as_deref(), Some("msg-1"));

    // A further edit updates the same Gmail draft rather than creating one.
    let edited = db
        .drafts_upsert(
            &Draft {
                body_html: "<p>1234</p>".into(),
                ..synced.clone()
            },
            Some(synced.revision),
        )
        .await
        .unwrap();
    db.outbox_enqueue_draft_sync(&acc.id, &edited.local_id, edited.revision)
        .await
        .unwrap();
    let op: i64 = db
        .read(move |c| {
            Ok(c.query_row(
                "SELECT id FROM outbox_ops WHERE kind='draft_sync' AND state='pending'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    db.outbox_set(op, "pending", 0, 0, None).await.unwrap();
    assert!(sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap());
    let after = db.drafts_get(&edited.local_id).await.unwrap().unwrap();
    assert_eq!(after.remote_draft_id.as_deref(), Some("draft-1"));
    assert_eq!(after.remote_message_id.as_deref(), Some("msg-2"));
    std::env::remove_var("SIFT_GMAIL_BASE");
}

/// P5.2: a failed or cancelled remote sync never touches local content.
#[tokio::test]
async fn p52_failed_and_cancelled_syncs_cannot_erase_local_content() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    // Gmail refuses every draft write (permission revoked, quota, …).
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/gmail/v1/users/me/drafts"))
        .respond_with(wiremock::ResponseTemplate::new(403).set_body_string("forbidden"))
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let db = sift::db::Db::open(dir.path()).unwrap();
    let acc = db.new_account("ada@x.com", None, None).await.unwrap();
    let provider = GmailApiProvider::new(acc.id.clone(), GmailClient::new("t".into()));
    let saved = db
        .drafts_upsert(
            &Draft {
                account_id: acc.id.clone(),
                to_json: vec![addr("bob@y.org")],
                subject: "Keep me".into(),
                body_html: "<p>precious</p>".into(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    let op = db
        .outbox_enqueue_draft_sync(&acc.id, &saved.local_id, saved.revision)
        .await
        .unwrap();
    db.outbox_set(op, "pending", 0, 0, None).await.unwrap();
    let err = sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap_err();
    assert!(!err.to_string().is_empty());
    let after_failure = db.drafts_get(&saved.local_id).await.unwrap().unwrap();
    assert_eq!(after_failure.body_html, "<p>precious</p>");
    assert_eq!(after_failure.subject, "Keep me");
    assert!(after_failure.remote_draft_id.is_none());

    // A cancelled sync is simply dropped: the draft keeps its content.
    let op = db
        .outbox_enqueue_draft_sync(&acc.id, &saved.local_id, saved.revision)
        .await
        .unwrap();
    let cancelled = db
        .outbox_cancel_draft_sync(&acc.id, &saved.local_id)
        .await
        .unwrap();
    assert_eq!(cancelled, 1);
    let state: String = db
        .read(move |c| {
            Ok(c.query_row(
                "SELECT state FROM outbox_ops WHERE id=?",
                rusqlite::params![op],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(state, "cancelled");
    assert!(!sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap());
    let after_cancel = db.drafts_get(&saved.local_id).await.unwrap().unwrap();
    assert_eq!(after_cancel.body_html, "<p>precious</p>");
    assert_eq!(after_cancel.revision, saved.revision);
    std::env::remove_var("SIFT_GMAIL_BASE");
}

/// P5.2: a Gmail draft that only exists on the server is imported as an
/// editable local draft, and the message already synced supplies its content.
#[tokio::test]
async fn p52_remote_only_gmail_draft_is_imported_editable() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/gmail/v1/users/me/drafts"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "drafts": [{"id": "r1", "message": {"id": "m1", "threadId": "t1"}}]
            })),
        )
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let db = sift::db::Db::open(dir.path()).unwrap();
    let acc = db.new_account("ada@x.com", None, None).await.unwrap();
    // The message sync already stored the draft's copy.
    db.messages_upsert(sift::db::messages::MsgUpsert {
        id: "m1".into(),
        account_id: acc.id.clone(),
        thread_id: "t1".into(),
        subject: "Written in Gmail".into(),
        rfc_message_id: Some("<m1@example.com>".into()),
        ..Default::default()
    })
    .await
    .unwrap();
    db.bodies_put(sift::db::bodies::BodyPut {
        account_id: acc.id.clone(),
        message_id: "m1".into(),
        html: Some("<p>written elsewhere</p>".into()),
        text: None,
        remote_images: 0,
        trackers: 0,
        dark_safe: true,
        quoted_from: None,
        unsubscribe: Default::default(),
    })
    .await
    .unwrap();

    let provider = GmailApiProvider::new(acc.id.clone(), GmailClient::new("t".into()));
    let report = sift::outbox::sync_remote_drafts(&db, &provider, &acc.id)
        .await
        .unwrap();
    assert_eq!(report.imported.len(), 1);

    let page = db
        .drafts_list(std::slice::from_ref(&acc.id), None, 10)
        .await
        .unwrap();
    assert_eq!(page.drafts.len(), 1);
    let imported = &page.drafts[0];
    assert_eq!(imported.subject, "Written in Gmail");
    assert_eq!(imported.body_html, "<p>written elsewhere</p>");
    assert_eq!(imported.remote_draft_id.as_deref(), Some("r1"));
    assert_eq!(imported.state, "editing");

    // Re-running the reconcile adopts rather than duplicating.
    let again = sift::outbox::sync_remote_drafts(&db, &provider, &acc.id)
        .await
        .unwrap();
    assert!(again.imported.is_empty() && again.adopted.is_empty());
    assert_eq!(
        db.drafts_list(std::slice::from_ref(&acc.id), None, 10)
            .await
            .unwrap()
            .drafts
            .len(),
        1
    );
    std::env::remove_var("SIFT_GMAIL_BASE");
}

/// P5.1: the send path no longer deletes the draft — it stays until an
/// acknowledged send cleans it up (P6.2).
#[tokio::test]
async fn p51_send_keeps_the_draft_and_freezes_its_revision() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/gmail/v1/users/me/messages/send"))
        .respond_with(|req: &wiremock::Request| {
            let body: serde_json::Value =
                serde_json::from_str(&String::from_utf8_lossy(&req.body)).unwrap_or_default();
            assert_eq!(body["threadId"], "t1", "a reply sends with its threadId");
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"id": "mSent", "threadId": "t1"}))
        })
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let db = sift::db::Db::open(dir.path()).unwrap();
    let acc = db.new_account("ada@x.com", None, None).await.unwrap();
    let draft = db
        .drafts_upsert(
            &Draft {
                account_id: acc.id.clone(),
                thread_id: Some("t1".into()),
                to_json: vec![addr("bob@y.org")],
                subject: "Re: hi".into(),
                body_html: "<p>reply</p>".into(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    let identity = sift::outgoing::Identity {
        email: "ada@x.com".into(),
        display_name: Some("Ada".into()),
    };
    let prepared = sift::outgoing::prepare(dir.path(), &draft, &identity, 1_700_000_000).unwrap();
    assert!(std::path::Path::new(&prepared.raw_path).exists());
    assert_eq!(prepared.revision, draft.revision);
    assert_eq!(prepared.draft_id, draft.local_id);
    assert!(prepared.raw_size > 0);
    assert_eq!(prepared.envelope_recipients, vec!["bob@y.org".to_string()]);

    let handle = db
        .drafts_enqueue_send(
            &prepared,
            &sift::db::drafts::SendSchedule::now(sift::db::now_ms()),
            false,
        )
        .await
        .unwrap();
    let queued = db.drafts_get(&draft.local_id).await.unwrap().unwrap();
    assert_eq!(queued.state, "queued");
    assert_eq!(
        queued.rfc_message_id.as_deref(),
        Some(prepared.rfc_message_id.as_str())
    );

    // Undo before the deadline: the real draft comes back.
    let reopened = db.drafts_cancel_send(handle.op_id).await.unwrap().unwrap();
    assert_eq!(reopened.state, "editing");
    assert!(reopened.not_before.is_none());
    assert_eq!(reopened.body_html, "<p>reply</p>");
    // Cancelling twice is "too late" rather than a second success.
    assert!(db.drafts_cancel_send(handle.op_id).await.unwrap().is_none());

    // Re-queue and send for real: the draft survives the send.
    let handle = db
        .drafts_enqueue_send(
            &prepared,
            &sift::db::drafts::SendSchedule::now(sift::db::now_ms()),
            false,
        )
        .await
        .unwrap();
    let provider = GmailApiProvider::new(acc.id.clone(), GmailClient::new("t".into()));
    assert!(sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap());
    let after_send = db.drafts_get(&draft.local_id).await.unwrap().unwrap();
    assert_eq!(after_send.body_html, "<p>reply</p>");
    // P6.2: an acknowledged send marks the draft `sent` and keeps its content
    // as the seven-day recovery copy. It is no longer a draft the user can
    // edit or send, so it does not appear in the Drafts list either.
    assert_eq!(after_send.state, "sent");
    assert!(
        db.drafts_list(std::slice::from_ref(&acc.id), None, 10)
            .await
            .unwrap()
            .drafts
            .iter()
            .all(|d| d.local_id != draft.local_id),
        "a sent draft is a recovery copy, not a draft"
    );
    assert!(handle.op_id > 0);
    std::env::remove_var("SIFT_GMAIL_BASE");
}
