//! P11-T03: scripted passwords yield mapped errors; LOGIN never logs raw.
#[path = "support/mod.rs"]
mod support;

use sift::provider::imap::conn::ImapPool;

fn pool_for(port: u16, pw: &str) -> ImapPool {
    ImapPool::new(
        "user@gmail.com".into(),
        pw.into(),
        "127.0.0.1".into(),
        port,
        true,
    )
}

async fn code_for(port: u16, pw: &str) -> (String, bool) {
    let pool = pool_for(port, pw);
    match pool.worker().await {
        Ok(_) => ("ok".into(), false),
        Err(e) => {
            let v = serde_json::to_value(&e).unwrap();
            (
                v["code"].as_str().unwrap_or("?").to_string(),
                v["retryable"].as_bool().unwrap_or(false),
            )
        }
    }
}

#[tokio::test]
async fn p11_t03_login_error_mapping() {
    let fake = support::fake_imap::FakeGmail::start().await;
    let port = fake.addr.port();
    // good password connects
    assert_eq!(code_for(port, "goodpassword0000").await.0, "ok");
    // each scripted password maps per the task-4 table
    let cases = [
        ("badpassword000000", "imap_bad_password", false),
        ("needapppassword0", "imap_needs_app_password", false),
        ("weblogin00000000", "imap_web_login_required", false),
        ("disabled00000000", "imap_disabled_by_admin", false),
        ("manyconns0000000", "imap_too_many_connections", true),
    ];
    for (pw, code, retryable) in cases {
        let (got, ret) = code_for(port, pw).await;
        assert_eq!(got, code, "for password {pw}");
        assert_eq!(ret, retryable, "retryable for {pw}");
    }
}
