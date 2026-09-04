#![allow(clippy::await_holding_lock)] // tests serialize env-var servers via a shared mutex (intentional)
static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}
// P7-T03: send delayed 5s then messages.send with threadId; T04 cancel within delay
#[tokio::test]
async fn p7_t03_send_delay_and_thread() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path_regex(".*/messages/send"))
        .respond_with(|req: &wiremock::Request| {
            let body: serde_json::Value =
                serde_json::from_str(&String::from_utf8_lossy(&req.body)).unwrap_or_default();
            assert!(body.get("threadId").is_some());
            wiremock::ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"id":"mReal","threadId":"t1"}))
        })
        .expect(1)
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let db = sift::db::Db::open(dir.path()).unwrap();
    let acc = db.new_account("a@x", None, None).await.unwrap();
    let d = sift::dto::Draft {
        local_id: None,
        account_id: acc.id.clone(),
        remote_draft_id: None,
        remote_message_id: None,
        thread_id: Some("t1".into()),
        in_reply_to_message_id: None,
        mode: "reply".into(),
        to_json: vec![],
        cc_json: vec![],
        bcc_json: vec![],
        subject: "Re: hi".into(),
        body_html: "<p>yo</p>".into(),
        attachments_json: vec![],
        updated_at: None,
    };
    let saved = db.drafts_upsert(&d).await.unwrap();
    let lid = saved.local_id.clone().unwrap();
    // not_before in future: drain_one should skip (not_before > now)
    let op = db
        .outbox_enqueue(
            &acc.id,
            "send",
            &serde_json::json!({"raw":"AAAA","threadId":"t1","localId":lid}).to_string(),
            None,
            sift::db::now_ms() + 5000,
        )
        .await
        .unwrap();
    let provider = sift::provider::gmail::api::GmailApiProvider::new(
        acc.id.clone(),
        sift::provider::gmail::client::GmailClient::new("t".into()),
    );
    // immediate drain does nothing (not_before future -> outbox_next filters)
    assert!(!sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap());
    // make due then drain sends
    db.outbox_set(op, "pending", 0, 0, None).await.unwrap();
    assert!(sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap());
    std::env::remove_var("SIFT_GMAIL_BASE");
}

#[tokio::test]
async fn p7_t04_send_cancel_pending() {
    let _g = lock_env();
    let dir = tempfile::tempdir().unwrap();
    let db = sift::db::Db::open(dir.path()).unwrap();
    let acc = db.new_account("a@x", None, None).await.unwrap();
    let d = sift::dto::Draft {
        local_id: None,
        account_id: acc.id.clone(),
        remote_draft_id: None,
        remote_message_id: None,
        thread_id: None,
        in_reply_to_message_id: None,
        mode: "new".into(),
        to_json: vec![],
        cc_json: vec![],
        bcc_json: vec![],
        subject: "s".into(),
        body_html: "b".into(),
        attachments_json: vec![],
        updated_at: None,
    };
    let saved = db.drafts_upsert(&d).await.unwrap();
    let op = db
        .outbox_enqueue(
            &acc.id,
            "send",
            &serde_json::json!({"raw":"AAAA","localId":saved.local_id}).to_string(),
            None,
            sift::db::now_ms() + 5000,
        )
        .await
        .unwrap();
    db.outbox_set(op, "cancelled", 0, 0, None).await.unwrap();
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
}
