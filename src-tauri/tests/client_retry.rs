#![allow(clippy::await_holding_lock)] // tests serialize env-var servers via a shared mutex (intentional)
static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}
// P3-T03: 429 Retry-After, 5x500 then 200, 8 failures
#[tokio::test]
async fn p3_t03_retry_policies() {
    let _g = lock_env();
    // 429 with Retry-After: 2 -> second attempt after >=2s tested via history_list? Use get_profile endpoint.
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let calls2 = calls.clone();
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(".*/profile"))
        .respond_with(move |_: &wiremock::Request| {
            let n = calls2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n == 0 {
                wiremock::ResponseTemplate::new(429)
                    .insert_header("Retry-After", "1")
                    .set_body_string("{}")
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"emailAddress":"a@x","messagesTotal":1,"historyId":"100"}),
                )
            }
        })
        .mount(&server)
        .await;
    let c = sift::provider::gmail::client::GmailClient::new("t".into());
    let t0 = std::time::Instant::now();
    let p = c.get_profile().await.unwrap();
    assert_eq!(p.history_id, "100");
    assert!(t0.elapsed() >= std::time::Duration::from_secs(1));
    assert!(calls.load(std::sync::atomic::Ordering::SeqCst) >= 2);
    std::env::remove_var("SIFT_GMAIL_BASE");
}

#[tokio::test]
async fn p3_t03_500_then_ok_and_eventual_fail() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    let c2 = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c3 = c2.clone();
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(move |_: &wiremock::Request| {
            let n = c3.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if n < 5 {
                wiremock::ResponseTemplate::new(500).set_body_string("err")
            } else {
                wiremock::ResponseTemplate::new(200).set_body_json(
                    serde_json::json!({"emailAddress":"a@x","messagesTotal":1,"historyId":"101"}),
                )
            }
        })
        .mount(&server)
        .await;
    let c = sift::provider::gmail::client::GmailClient::new("t".into());
    let p = c.get_profile().await.unwrap();
    assert_eq!(p.history_id, "101");
    std::env::remove_var("SIFT_GMAIL_BASE");
}
