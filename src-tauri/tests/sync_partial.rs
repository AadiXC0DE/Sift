#![allow(clippy::await_holding_lock)] // tests serialize env-var servers via a shared mutex (intentional)
static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}
// P3-T11 + T12: history script add/label/delete + 404 tolerance
use sift::db::Db;
use sift::provider::Provider;

#[tokio::test]
async fn p3_t11_history_applies() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("a@x", None, None).await.unwrap();
    db.accounts_set_history(&acc.id, "100", 0).await.unwrap();
    // seed B and C
    db.messages_upsert(sift::db::messages::MsgUpsert {
        id: "mB".into(),
        account_id: acc.id.clone(),
        thread_id: "tB".into(),
        internal_date: 1,
        subject: "B".into(),
        is_unread: true,
        label_ids: vec!["INBOX".into(), "UNREAD".into()],
        ..Default::default()
    })
    .await
    .unwrap();
    db.messages_upsert(sift::db::messages::MsgUpsert {
        id: "mC".into(),
        account_id: acc.id.clone(),
        thread_id: "tC".into(),
        internal_date: 2,
        subject: "C".into(),
        label_ids: vec![],
        ..Default::default()
    })
    .await
    .unwrap();

    // history: added A, labelRemoved UNREAD on A? use added then deleted + label changes
    wiremock::Mock::given(wiremock::matchers::method("GET"))
    .and(wiremock::matchers::path_regex(".*/history"))
    .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
      "history": [
        {"id": "101", "messagesAdded": [{"message": {"id": "mA", "threadId": "tA"}}]},
        {"id": "102", "messagesDeleted": [{"message": {"id": "mB", "threadId": "tB"}}]},
        {"id": "103", "labelsAdded": [{"message": {"id": "mC", "threadId": "tC"}, "labelIds": ["INBOX"]}]}
      ],
      "historyId": "103"
    })))
    .mount(&server).await;
    // meta for mA
    wiremock::Mock::given(wiremock::matchers::method("GET"))
    .and(wiremock::matchers::path_regex(".*/messages/mA"))
    .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
      "id": "mA", "threadId": "tA", "snippet": "a", "historyId": "101", "internalDate": "1700000000001",
      "labelIds": ["INBOX", "UNREAD"],
      "payload": {"headers": [{"name": "From", "value": "a@x"}, {"name": "Subject", "value": "A"}]}
    })))
    .mount(&server).await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(".*/messages/mC"))
        .respond_with(
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
              "id": "mC", "threadId": "tC", "snippet": "c", "historyId": "103", "internalDate": "2",
              "labelIds": ["INBOX"],
              "payload": {"headers": []}
            })),
        )
        .mount(&server)
        .await;

    let provider = sift::provider::gmail::api::GmailApiProvider::new(
        acc.id.clone(),
        sift::provider::gmail::client::GmailClient::new("t".into()),
    );
    let sink = sift::provider::DbSink::new(db.clone());
    let out = provider
        .partial_sync(
            &sift::provider::Cursor::Gmail {
                history_id: "100".into(),
            },
            &sink,
        )
        .await
        .unwrap();
    let sift::provider::PartialOutcome::Synced {
        changed_threads, ..
    } = out
    else {
        panic!("expected Synced, got NeedsFull");
    };
    assert!(changed_threads.iter().any(|(_, t)| t == "tA"));
    // B deleted
    let exists: bool = db
        .read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id='mB')",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert!(!exists);
    // history advanced
    let hid: String = db
        .read({
            let aid = acc.id.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT history_id FROM accounts WHERE id=?",
                    rusqlite::params![aid],
                    |r| r.get(0),
                )?)
            }
        })
        .await
        .unwrap();
    assert_eq!(hid, "103");
    std::env::remove_var("SIFT_GMAIL_BASE");
}

#[tokio::test]
async fn p3_t12_added_then_deleted_404_skipped() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("a@x", None, None).await.unwrap();
    db.accounts_set_history(&acc.id, "100", 0).await.unwrap();
    wiremock::Mock::given(wiremock::matchers::method("GET"))
    .and(wiremock::matchers::path_regex(".*/history"))
    .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
      "history": [{"id": "101", "messagesAdded": [{"message": {"id": "mX", "threadId": "tX"}}]}],
      "historyId": "101"
    })))
    .mount(&server).await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(".*/messages/mX"))
        .respond_with(wiremock::ResponseTemplate::new(404).set_body_string("{}"))
        .mount(&server)
        .await;
    let provider = sift::provider::gmail::api::GmailApiProvider::new(
        acc.id.clone(),
        sift::provider::gmail::client::GmailClient::new("t".into()),
    );
    let sink = sift::provider::DbSink::new(db.clone());
    // should not fail
    provider
        .partial_sync(
            &sift::provider::Cursor::Gmail {
                history_id: "100".into(),
            },
            &sink,
        )
        .await
        .unwrap();
    std::env::remove_var("SIFT_GMAIL_BASE");
}
