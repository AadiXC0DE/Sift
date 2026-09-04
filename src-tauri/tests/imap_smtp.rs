//! P11-T10: SMTP send via fake sink — MIME preserved, errors mapped,
//! retryable 421 stays pending (outbox policy), success triggers partial.
#[path = "support/mod.rs"]
mod support;

fn raw_msg() -> Vec<u8> {
    b"From: user@gmail.com\r\nTo: bob@example.com\r\nSubject: hello\r\nIn-Reply-To: <abc@example.com>\r\nReferences: <abc@example.com>\r\nMessage-ID: <smtp-1@example.com>\r\n\r\nhello body".to_vec()
}

#[tokio::test]
async fn p11_t10_send_delivers_mime() {
    let fake = support::fake_smtp::FakeSmtp::start().await;
    let raw = raw_msg();
    sift::provider::imap::smtp::send_test(
        "127.0.0.1",
        fake.addr.port(),
        "user@gmail.com",
        "goodpassword0000",
        &raw,
    )
    .await
    .unwrap();
    let accepted = fake.accepted.lock().unwrap();
    assert_eq!(accepted.len(), 1);
    assert_eq!(accepted[0].from, "user@gmail.com");
    assert!(accepted[0].to.iter().any(|t| t.contains("bob@example.com")));
    let got = String::from_utf8_lossy(&accepted[0].raw);
    assert!(got.contains("In-Reply-To: <abc@example.com>"));
    assert!(got.contains("References: <abc@example.com>"));
}

#[tokio::test]
async fn p11_t10_smtp_errors_mapped() {
    // 535 → bad password (re-auth fix flow).
    let fake = support::fake_smtp::FakeSmtp::start_with(support::fake_smtp::SmtpBehavior {
        fail_auth: true,
        ..Default::default()
    })
    .await;
    let e = sift::provider::imap::smtp::send_test(
        "127.0.0.1",
        fake.addr.port(),
        "user@gmail.com",
        "badpassword00000000",
        &raw_msg(),
    )
    .await
    .unwrap_err();
    assert_eq!(
        serde_json::to_value(&e).unwrap()["code"],
        "imap_bad_password"
    );

    // 552 → size error surfaced.
    let fake = support::fake_smtp::FakeSmtp::start_with(support::fake_smtp::SmtpBehavior {
        fail_data_552: true,
        ..Default::default()
    })
    .await;
    let e = sift::provider::imap::smtp::send_test(
        "127.0.0.1",
        fake.addr.port(),
        "user@gmail.com",
        "goodpassword0000",
        &raw_msg(),
    )
    .await
    .unwrap_err();
    assert_eq!(serde_json::to_value(&e).unwrap()["code"], "too_large");

    // 421 once → retryable transient (outbox keeps pending with backoff).
    let fake = support::fake_smtp::FakeSmtp::start_with(support::fake_smtp::SmtpBehavior {
        fail_data_421_once: true,
        ..Default::default()
    })
    .await;
    // First attempt fails transient...
    let e = sift::provider::imap::smtp::send_test(
        "127.0.0.1",
        fake.addr.port(),
        "user@gmail.com",
        "goodpassword0000",
        &raw_msg(),
    )
    .await
    .unwrap_err();
    assert_eq!(serde_json::to_value(&e).unwrap()["code"], "imap_transient");
    // ...second succeeds (fake flips after one 421).
    sift::provider::imap::smtp::send_test(
        "127.0.0.1",
        fake.addr.port(),
        "user@gmail.com",
        "goodpassword0000",
        &raw_msg(),
    )
    .await
    .unwrap();
}
