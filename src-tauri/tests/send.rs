//! P5.3 — what actually leaves the machine on a send: the frozen MIME, the
//! explicit envelope, and the refusal to guess a recipient.
#![allow(clippy::await_holding_lock)] // tests serialize env-var servers via a shared mutex (intentional)
static ENV_LOCK: std::sync::LazyLock<std::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(()));

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    // A panic in a sibling test poisons the lock; the env vars are still ours
    // to serialize, so recover the guard instead of failing every later test.
    ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

use sift::dto::{Address, Draft};
use sift::provider::gmail::api::GmailApiProvider;
use sift::provider::gmail::client::GmailClient;

fn addr(email: &str) -> Address {
    Address {
        n: None,
        e: email.into(),
        me: None,
    }
}

fn identity() -> sift::outgoing::Identity {
    sift::outgoing::Identity {
        email: "ada@x.com".into(),
        display_name: Some("Ada".into()),
    }
}

/// P5.3: the send is deferred until its deadline, then delivered as the exact
/// prepared bytes, with the prepared envelope and the draft's threadId.
#[tokio::test]
async fn p53_send_is_deferred_then_delivered_with_the_prepared_envelope() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/gmail/v1/users/me/messages/send"))
        .respond_with(|req: &wiremock::Request| {
            use base64::Engine;
            let body: serde_json::Value =
                serde_json::from_str(&String::from_utf8_lossy(&req.body)).unwrap_or_default();
            assert_eq!(body["threadId"], "t1");
            let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(body["raw"].as_str().unwrap())
                .unwrap();
            let text = String::from_utf8_lossy(&raw);
            // The REST message keeps Bcc: Gmail derives the delivery list from
            // the message headers.
            assert!(text.contains("Bcc: <dan@y.org>"), "got {text:.200}");
            // mail-builder quotes the display name (RFC 5322).
            assert!(
                text.contains("From: \"Ada\" <ada@x.com>"),
                "got {text:.200}"
            );
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
                to_json: vec![addr("bob@y.org"), addr("bob@y.org")],
                bcc_json: vec![addr("dan@y.org")],
                subject: "Deferred".into(),
                body_html: "<p>hello</p>".into(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
    let prepared = sift::outgoing::prepare(dir.path(), &draft, &identity(), 1_700_000_000).unwrap();
    // The envelope de-duplicates and keeps the blind recipient.
    assert_eq!(
        prepared.envelope_recipients,
        vec!["bob@y.org".to_string(), "dan@y.org".to_string()]
    );
    assert_eq!(prepared.bcc_recipients, vec!["dan@y.org".to_string()]);

    let provider = GmailApiProvider::new(acc.id.clone(), GmailClient::new("t".into()));
    let handle = db
        .drafts_enqueue_send(
            &prepared,
            &sift::db::drafts::SendSchedule::now(sift::db::now_ms() + 60_000),
            false,
        )
        .await
        .unwrap();
    assert!(
        !sift::outbox::drain_one(&db, &provider, &acc.id, true)
            .await
            .unwrap(),
        "an op before its deadline is not drained"
    );
    db.outbox_set(handle.op_id, "pending", 0, 0, None)
        .await
        .unwrap();
    assert!(sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap());
    let state: String = db
        .read(move |c| {
            Ok(c.query_row(
                "SELECT state FROM outbox_ops WHERE id=?",
                rusqlite::params![handle.op_id],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(state, "done");
    std::env::remove_var("SIFT_GMAIL_BASE");
}

/// P5.3: a queued message with no usable recipient fails before any network
/// work — it is never redirected to the sender.
#[tokio::test]
async fn p53_send_without_recipients_fails_and_sends_nothing() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/gmail/v1/users/me/messages/send"))
        .respond_with(
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"id": "mSent", "threadId": "t1"})),
        )
        .expect(0)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let db = sift::db::Db::open(dir.path()).unwrap();
    let acc = db.new_account("ada@x.com", None, None).await.unwrap();
    // A legacy payload (queued before the envelope was stored) whose message
    // names nobody: the old code sent it to ada@x.com.
    use base64::Engine;
    let raw = b"From: ada@x.com\r\nSubject: nobody\r\n\r\nbody";
    let payload = serde_json::json!({
        "raw": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw),
    })
    .to_string();
    let op = db
        .outbox_enqueue(&acc.id, "send", &payload, None, 0)
        .await
        .unwrap();
    let provider = GmailApiProvider::new(acc.id.clone(), GmailClient::new("t".into()));
    let err = sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap_err();
    assert_eq!(
        serde_json::to_value(&err).unwrap()["code"],
        "bad_recipient",
        "no recipient must be a hard failure"
    );
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
    assert_eq!(state, "failed", "a non-retryable send failure is terminal");
    std::env::remove_var("SIFT_GMAIL_BASE");
}
