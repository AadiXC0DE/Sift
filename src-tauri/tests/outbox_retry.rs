#![allow(clippy::await_holding_lock)] // tests serialize env-var servers via a shared mutex (intentional)
static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}
// P6-T06: 500x3 then 200 -> done attempts=4; 8 failures -> failed + revert signal
#[tokio::test]
async fn p6_t06_retry_then_done_and_fail() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    // fail 3 then ok
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path_regex(".*/messages/batchModify"))
        .respond_with(wiremock::ResponseTemplate::new(500).set_body_string("err"))
        .up_to_n_times(3)
        .mount(&server)
        .await;
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path_regex(".*/messages/batchModify"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("{}"))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let db = sift::db::Db::open(dir.path()).unwrap();
    let acc = db.new_account("a@x", None, None).await.unwrap();
    let op = db
        .outbox_enqueue(
            &acc.id,
            "modify_labels",
            &serde_json::json!({"ids":["m1"],"add":[],"remove":["INBOX"]}).to_string(),
            None,
            0,
        )
        .await
        .unwrap();
    let client = sift::gmail::client::GmailClient::new("t".into());
    // drain until done (retries inside client send_with_retry handle 500s transparently, so one drain_one succeeds)
    sift::outbox::drain_one(&db, &client, &acc.id, true)
        .await
        .unwrap();
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
    assert_eq!(state, "done");
    std::env::remove_var("SIFT_GMAIL_BASE");
}

#[tokio::test]
async fn p6_t06_eight_failures_failed() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(wiremock::ResponseTemplate::new(500).set_body_string("err"))
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let db = sift::db::Db::open(dir.path()).unwrap();
    let acc = db.new_account("a@x", None, None).await.unwrap();
    // seed op with attempts=7 so next failure is 8th -> failed
    let op = db
        .outbox_enqueue(
            &acc.id,
            "modify_labels",
            &serde_json::json!({"ids":["m1"],"add":[],"remove":[]}).to_string(),
            None,
            0,
        )
        .await
        .unwrap();
    db.outbox_set(op, "pending", 7, 0, None).await.unwrap();
    let client = sift::gmail::client::GmailClient::new("t".into());
    let _ = sift::outbox::drain_one(&db, &client, &acc.id, true).await;
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
    assert_eq!(state, "failed");
    std::env::remove_var("SIFT_GMAIL_BASE");
}
