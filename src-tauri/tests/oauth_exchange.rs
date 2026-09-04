#![allow(clippy::await_holding_lock)] // tests serialize env-var servers via a shared mutex (intentional)
static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}
// P2-T03: token exchange posts verifier/id/secret/grant
#[tokio::test]
async fn p2_t03_exchange_shape() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var("SIFT_TOKEN_URL", format!("{}/token", server.uri()));
    wiremock::Mock::given(wiremock::matchers::method("POST"))
        .and(wiremock::matchers::path("/token"))
        .respond_with(|req: &wiremock::Request| {
            let body = String::from_utf8_lossy(&req.body).to_string();
            assert!(body.contains("code_verifier="), "{body}");
            assert!(body.contains("client_id="), "{body}");
            assert!(body.contains("client_secret="), "{body}");
            assert!(body.contains("grant_type=authorization_code"), "{body}");
            wiremock::ResponseTemplate::new(200).set_body_json(
                serde_json::json!({"access_token":"at","refresh_token":"rt","expires_in":3600}),
            )
        })
        .expect(1)
        .mount(&server)
        .await;
    let http = reqwest::Client::new();
    let t = sift::provider::gmail::oauth::exchange("code123", "verifier123", 9999, &http)
        .await
        .unwrap();
    assert_eq!(t.access_token, "at");
    assert_eq!(t.refresh_token.as_deref(), Some("rt"));
    std::env::remove_var("SIFT_TOKEN_URL");
}
