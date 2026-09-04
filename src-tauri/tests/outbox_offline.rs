#![allow(clippy::await_holding_lock)] // tests serialize env-var servers via a shared mutex (intentional)
static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}
// P6-T07: offline accumulates, online drains in id order
#[tokio::test]
async fn p6_t07_offline_order() {
    let _g = lock_env();
    use std::sync::{Arc, Mutex};
    let order: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(vec![]));
    let order2 = order.clone();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .respond_with(move |req: &wiremock::Request| {
            let body = String::from_utf8_lossy(&req.body).to_string();
            order2.lock().unwrap().push(body);
            wiremock::ResponseTemplate::new(200).set_body_string("{}")
        })
        .mount(&server)
        .await;
    let dir = tempfile::tempdir().unwrap();
    let db = sift::db::Db::open(dir.path()).unwrap();
    let acc = db.new_account("a@x", None, None).await.unwrap();
    let provider = sift::provider::gmail::api::GmailApiProvider::new(
        acc.id.clone(),
        sift::provider::gmail::client::GmailClient::new("t".into()),
    );
    let o1 = db
        .outbox_enqueue(
            &acc.id,
            "modify_labels",
            &serde_json::json!({"ids":["m1"],"add":[],"remove":[]}).to_string(),
            None,
            0,
        )
        .await
        .unwrap();
    let o2 = db
        .outbox_enqueue(
            &acc.id,
            "modify_labels",
            &serde_json::json!({"ids":["m2"],"add":[],"remove":[]}).to_string(),
            None,
            0,
        )
        .await
        .unwrap();
    assert!(o1 < o2);
    // offline: no drain
    assert!(!sift::outbox::drain_one(&db, &provider, &acc.id, false)
        .await
        .unwrap());
    // online drains in order
    assert!(sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap());
    assert!(sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap());
    assert!(!sift::outbox::drain_one(&db, &provider, &acc.id, true)
        .await
        .unwrap());
    assert_eq!(order.lock().unwrap().len(), 2);
    std::env::remove_var("SIFT_GMAIL_BASE");
}
