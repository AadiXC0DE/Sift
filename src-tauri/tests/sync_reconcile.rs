#![allow(clippy::await_holding_lock)] // tests serialize env-var servers via a shared mutex (intentional)
static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}
// P3-T13: history 404 triggers reconcile
#[tokio::test]
async fn p3_t13_reconcile_on_404() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    std::env::set_var("SIFT_BATCH_URL", format!("{}/batch/gmail/v1", server.uri()));
    let dir = tempfile::tempdir().unwrap();
    let db = sift::db::Db::open(dir.path()).unwrap();
    let acc = db.new_account("a@x", None, None).await.unwrap();
    db.accounts_set_history(&acc.id, "old", 0).await.unwrap();
    // local extra
    db.messages_upsert(sift::db::messages::MsgUpsert {
        id: "mLocal".into(),
        account_id: acc.id.clone(),
        thread_id: "tLocal".into(),
        internal_date: 1,
        label_ids: vec!["INBOX".into()],
        ..Default::default()
    })
    .await
    .unwrap();
    // history 404
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(".*/history"))
        .respond_with(wiremock::ResponseTemplate::new(404).set_body_string("{}"))
        .mount(&server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(".*/profile"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"emailAddress":"a@x","messagesTotal":1,"historyId":"999"}),
        ))
        .mount(&server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(".*/messages$"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"messages": [{"id": "mServer", "threadId": "tServer"}]}),
        ))
        .mount(&server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
    .and(wiremock::matchers::path("/batch/gmail/v1"))
    .respond_with(|req: &wiremock::Request| {
      let ct = req.headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
      let b = ct.split("boundary=").nth(1).unwrap_or("batch").to_string();
      let body = String::from_utf8_lossy(&req.body).to_string();
      let mut out = String::new();
      for line in body.lines() {
        if line.starts_with("GET ") {
          let id = line.split("/messages/").nth(1).unwrap_or("m?").split('?').next().unwrap_or("m?");
          // minimal: metadata returns INBOX for mServer
          out.push_str(&format!("--{b}\r\nContent-Type: application/http\r\n\r\nHTTP/1.1 200 OK\r\n\r\n{{\"id\":\"{id}\",\"threadId\":\"tServer\",\"labelIds\":[\"INBOX\"],\"snippet\":\"s\",\"internalDate\":\"1700000000000\",\"payload\":{{\"headers\":[{{\"name\":\"Subject\",\"value\":\"S\"}}]}}}}\r\n"));
        }
      }
      out.push_str(&format!("--{b}--\r\n"));
      wiremock::ResponseTemplate::new(200).set_body_string(out).insert_header("content-type", format!("multipart/mixed; boundary={b}"))
    })
    .mount(&server).await;
    let client = sift::gmail::client::GmailClient::new("t".into());
    sift::sync::partial::run_partial_sync(&db, &acc.id, &client)
        .await
        .unwrap();
    // local extra deleted, server extra inserted
    let has_local: bool = db
        .read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id='mLocal')",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert!(!has_local);
    let has_server: bool = db
        .read(|c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id='mServer')",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert!(has_server);
    std::env::remove_var("SIFT_GMAIL_BASE");
    std::env::remove_var("SIFT_BATCH_URL");
}
