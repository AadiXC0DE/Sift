#![allow(clippy::await_holding_lock)] // tests serialize env-var servers via a shared mutex (intentional)
static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}
// P3-T04: 5-unit calls never exceed 250 units in any 1s window (we cap at 200/s = 40 calls/s).
#[tokio::test]
async fn p3_t04_ratelimit_window() {
    let _g = lock_env();
    use std::sync::{Arc, Mutex};
    let times: Arc<Mutex<Vec<std::time::Instant>>> = Arc::new(Mutex::new(vec![]));
    let times2 = times.clone();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    // 5-unit endpoint: messages.list
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(".*/messages$"))
        .respond_with(move |_: &wiremock::Request| {
            times2.lock().unwrap().push(std::time::Instant::now());
            wiremock::ResponseTemplate::new(200).set_body_json(serde_json::json!({"messages": []}))
        })
        .mount(&server)
        .await;
    let c = sift::provider::gmail::client::GmailClient::new("t".into());
    // 60 sequential 5-unit calls = 300 units; at 200/s must take >= ~1.2s
    let t0 = std::time::Instant::now();
    for _ in 0..60 {
        c.list_messages(None, None, true).await.unwrap();
    }
    let el = t0.elapsed();
    let ts = times.lock().unwrap().clone();
    let mut max_in_window = 0;
    for i in 0..ts.len() {
        let mut n = 0;
        for j in i..ts.len() {
            if ts[j].duration_since(ts[i]) < std::time::Duration::from_secs(1) {
                n += 1;
            } else {
                break;
            }
        }
        max_in_window = max_in_window.max(n);
    }
    // 5 units each -> 250 units = 50 requests
    assert!(max_in_window <= 52, "max in 1s window = {max_in_window}");
    assert!(
        el >= std::time::Duration::from_millis(900),
        "expected throttling, elapsed = {el:?}"
    );
    std::env::remove_var("SIFT_GMAIL_BASE");
}
