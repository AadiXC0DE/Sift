#![allow(clippy::await_holding_lock)] // tests serialize env-var servers via a shared mutex (intentional)
static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}
// P8-T04: server search hydrates unknown then serves local
#[tokio::test]
async fn p8_t04_server_hydrates_then_local() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    // messages.list?q= returns 3 ids
    wiremock::Mock::given(wiremock::matchers::method("GET"))
    .and(wiremock::matchers::path_regex(".*/messages$"))
    .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
      "messages": [{"id":"m1","threadId":"t1"},{"id":"m2","threadId":"t2"},{"id":"m3","threadId":"t3"}]
    })))
    .mount(&server).await;
    // metadata batch for unknown: use single get fallback path (search command uses get_message_meta per unknown)
    wiremock::Mock::given(wiremock::matchers::method("GET"))
    .and(wiremock::matchers::path_regex(".*/messages/m3"))
    .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
      "id":"m3","threadId":"t3","snippet":"s","historyId":"1","internalDate":"1700000000000",
      "labelIds":["INBOX"],"payload":{"headers":[{"name":"Subject","value":"S3"}]}
    })))
    .mount(&server).await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
    .and(wiremock::matchers::path_regex(".*/messages/m[12]"))
    .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({
      "id":"m1","threadId":"t1","snippet":"s","historyId":"1","internalDate":"1700000000000",
      "labelIds":["INBOX"],"payload":{"headers":[]}
    })))
    .mount(&server).await;
    let dir = tempfile::tempdir().unwrap();
    let db = sift::db::Db::open(dir.path()).unwrap();
    let acc = db.new_account("a@x", None, None).await.unwrap();
    // seed m1,m2 locally
    for (id, tid) in [("m1", "t1"), ("m2", "t2")] {
        db.messages_upsert(sift::db::messages::MsgUpsert {
            id: id.into(),
            account_id: acc.id.clone(),
            thread_id: tid.into(),
            internal_date: 1,
            label_ids: vec!["INBOX".into()],
            ..Default::default()
        })
        .await
        .unwrap();
    }
    let client = sift::provider::gmail::client::GmailClient::new("t".into());
    let resp = client
        .list_messages(None, Some("hello"), false)
        .await
        .unwrap();
    assert_eq!(resp.messages.unwrap().len(), 3);
    // hydrate m3
    let m3 = client.get_message_meta("m3").await.unwrap();
    assert_eq!(m3.thread_id, "t3");
    std::env::remove_var("SIFT_GMAIL_BASE");
}
