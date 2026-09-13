//! P1.1/P1.2/P1.3/P1.4/P1.6: attachment transport protocol.
//!
//! Runs the fake Gmail server in its STRICT mode (the default): FETCH items
//! must be one legal item or a balanced parenthesized list, selected-state
//! operations before SELECT/EXAMINE are rejected, and every command is
//! recorded sanitized.
#[path = "support/mod.rs"]
mod support;

use base64::Engine as _;
use sift::db::Db;
use sift::provider::imap::conn::ImapPool;
use sift::provider::imap::provider::GmailImapProvider;
use sift::provider::imap::ids::ImapSection;
use sift::provider::{DbSink, Provider};

fn pool_for(port: u16) -> ImapPool {
    ImapPool::new(
        "user@gmail.com".into(),
        "goodpassword0000".into(),
        "127.0.0.1".into(),
        port,
        true,
    )
}

struct Ctx {
    fake: support::fake_imap::FakeGmail,
    pool: ImapPool,
    db: Db,
    acc: sift::dto::Account,
    provider: std::sync::Arc<GmailImapProvider>,
    /// Last field so the data directory outlives every handle into it.
    _dir: tempfile::TempDir,
}

async fn synced() -> Ctx {
    let fake = support::fake_imap::FakeGmail::start().await;
    let pool = pool_for(fake.addr.port());
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("user@gmail.com", None, None).await.unwrap();
    let provider = std::sync::Arc::new(GmailImapProvider::new(
        acc.id.clone(),
        pool.clone(),
        db.clone(),
    ));
    let sink = DbSink::new(db.clone());
    let cancel = tokio_util::sync::CancellationToken::new();
    let cursor = provider.full_sync(&sink, cancel).await.unwrap();
    assert!(matches!(cursor, sift::provider::Cursor::Imap { .. }));
    Ctx {
        fake,
        pool,
        db,
        acc,
        provider,
        _dir: dir,
    }
}

/// A message (hex id + decimal Gmail id) whose BODYSTRUCTURE has a base64
/// part at section `2`.
async fn pdf_message(db: &Db, acc: &str) -> (String, u64) {
    let mid: String = db
        .read({
            let acc = acc.to_string();
            move |c| {
                Ok(c.query_row(
                    "SELECT m.id FROM messages m JOIN attachments a ON a.message_id=m.id \
                     WHERE m.account_id=? AND a.mime='application/pdf' LIMIT 1",
                    rusqlite::params![acc],
                    |r| r.get::<_, String>(0),
                )?)
            }
        })
        .await
        .unwrap();
    let dec = u64::from_str_radix(&mid, 16).unwrap();
    (mid, dec)
}

async fn uid_in(db: &Db, acc: &str, role: &str, mid: &str) -> i64 {
    db.imap_uid_map(acc, role)
        .await
        .unwrap()
        .into_iter()
        .find(|(_, m)| m == mid)
        .unwrap_or_else(|| panic!("{mid} not mapped in {role}"))
        .0
}

fn err_code(e: &sift::errors::SiftError) -> String {
    serde_json::to_value(e).unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string()
}

fn err_retryable(e: &sift::errors::SiftError) -> bool {
    serde_json::to_value(e).unwrap()["retryable"]
        .as_bool()
        .unwrap_or(false)
}

fn err_message(e: &sift::errors::SiftError) -> String {
    serde_json::to_value(e).unwrap()["message"]
        .as_str()
        .unwrap()
        .to_string()
}

// -- P1.1: exact wire bytes -------------------------------------------------

#[tokio::test]
async fn p1_t01_exact_wire_bytes_for_sections() {
    let ctx = synced().await;
    let (mid, _dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    let uid = uid_in(&ctx.db, &ctx.acc.id, "all", &mid).await;
    for section in ["1", "2", "1.2", "2.1.3"] {
        let _ = ctx.provider.fetch_attachment(&mid, section).await;
        let want = format!("UID FETCH {uid} (UID BODY.PEEK[{section}])");
        assert!(
            ctx.fake.commands().iter().any(|c| c == &want),
            "expected exact command {want:?} in {:?}",
            ctx.fake.commands_with_prefix("UID FETCH")
        );
    }
}

#[tokio::test]
async fn p1_t01_invalid_locators_send_zero_network_commands() {
    // No sync: the provider has never opened a socket.
    let fake = support::fake_imap::FakeGmail::start().await;
    let _dir = tempfile::tempdir().unwrap();
    let db = Db::open(_dir.path()).unwrap();
    let acc = db.new_account("user@gmail.com", None, None).await.unwrap();
    let provider = GmailImapProvider::new(acc.id.clone(), pool_for(fake.addr.port()), db);

    let mid = "abcdef0123456789";
    for bad in ["cid@x", "a1", "", "0", "1..2", "2]", "1\r\n", "x1.2", "01"] {
        let e = provider.fetch_attachment(mid, bad).await.unwrap_err();
        assert_eq!(
            err_code(&e),
            "attachment_locator_invalid",
            "locator {bad:?} must be rejected before connecting"
        );
        assert!(!err_retryable(&e), "invalid locator is terminal: {bad:?}");
    }
    let st = fake.state.lock().unwrap();
    assert_eq!(st.cmd_count, 0, "no IMAP command may be sent");
    assert_eq!(st.connections, 0, "no socket may be opened");
}

#[tokio::test]
async fn p1_t01_builder_renders_partial_section_items() {
    // The snippet path's partial read must be one parenthesized item.
    let ctx = synced().await;
    let (mid, _dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    let uid = uid_in(&ctx.db, &ctx.acc.id, "all", &mid).await;
    let _ = ctx.provider.fetch_body(&mid).await;
    let items = sift::provider::imap::conn::FetchItems::new()
        .uid()
        .peek_partial(Some(ImapSection::parse("1").unwrap()), 0, Some(2048));
    assert_eq!(items.wire(), "(UID BODY.PEEK[1]<0.2048>)");
    // The partial form is accepted by the strict parser and stays one item.
    assert!(support::fake_imap::parse_fetch_args_for_test(&format!(
        "{uid} {items}"
    ))
    .is_ok());
}

// -- P1.2: strict fake server ----------------------------------------------

#[tokio::test]
async fn p1_t02_strict_parser_valid_and_malformed_cases() {
    // Valid: one bare item, one parenthesized item, a parenthesized list.
    for ok in [
        "1 BODYSTRUCTURE",
        "1 (UID)",
        "1 UID",
        "1 (UID BODY.PEEK[1])",
        "1:5,9 (UID FLAGS)",
        "1 (UID BODY.PEEK[HEADER.FIELDS (FROM TO SUBJECT)])",
        "1 (UID BODY.PEEK[2]<0.2048>)",
        "1 BODY.PEEK[1.2]",
        "1:* (UID X-GM-MSGID BODYSTRUCTURE) (CHANGEDSINCE 5)",
    ] {
        if let Err(e) = support::fake_imap::parse_fetch_args_for_test(ok) {
            panic!("must accept {ok:?}: {e}");
        }
    }
    // Malformed: two bare items (the old production form), unbalanced
    // brackets, unknown items, missing set.
    for bad in [
        "1 UID BODY.PEEK[1]",
        "1 UID FLAGS",
        "1 FLAGS UID",
        "1 (UID",
        "1 ()",
        "1 (BOGUS)",
        "1 (UID BOGUS)",
        "",
        "(UID)",
        "x (UID)",
        "1 (UID) extra",
        "1 BODY.PEEK[1",
    ] {
        assert!(
            support::fake_imap::parse_fetch_args_for_test(bad).is_err(),
            "must reject {bad:?}"
        );
    }
}

#[tokio::test]
async fn p1_t02_strict_server_rejects_old_form_accepts_corrected() {
    let fake = support::fake_imap::FakeGmail::start().await;
    let out = raw_exchange(
        fake.addr.port(),
        &[
            "LOGIN \"user@gmail.com\" \"goodpassword0000\"",
            "SELECT \"INBOX\"",
            // The old, malformed production form: two bare items.
            "UID FETCH 1 UID BODY.PEEK[1]",
            // The corrected form.
            "UID FETCH 1 (UID BODY.PEEK[1])",
        ],
    )
    .await;
    let t3 = line_for(&out, "t3");
    assert!(t3.contains(" BAD "), "old form must be BAD, got {t3:?}");
    let t4 = line_for(&out, "t4");
    assert!(t4.contains(" OK "), "corrected form must be OK, got {t4:?}");
}

#[tokio::test]
async fn p1_t02_decoded_bytes_and_length_are_exact() {
    let ctx = synced().await;
    let (mid, dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    let b64 = base64::engine::general_purpose::STANDARD;
    let cases: Vec<Vec<u8>> = vec![
        vec![],                                                    // zero-byte
        vec![0x00, 0x00, 0x00, 0x00],                              // NULs
        vec![0x80, 0xff, 0xfe, 0xc3, 0xa9, 0x0a, 0x0d, 0x1b, 0x7f], // > 0x7f + control
        b"PDFDATA-exact".to_vec(),
        (0u8..=255).collect(),
    ];
    for payload in cases {
        // Section 2 is base64 in BODYSTRUCTURE; serve the real encoding.
        ctx.fake
            .set_section_bytes(dec, "2", b64.encode(&payload).into_bytes());
        let got = ctx
            .provider
            .fetch_attachment(&mid, "2")
            .await
            .unwrap_or_else(|e| panic!("fetch {}-byte payload: {e}", payload.len()));
        assert_eq!(got.len(), payload.len(), "length for {payload:?}");
        assert_eq!(got, payload, "exact bytes for {payload:?}");
    }
}

#[tokio::test]
async fn p1_t02_zero_byte_attachment_is_not_missing() {
    let ctx = synced().await;
    let (mid, dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    ctx.fake.set_section_bytes(dec, "2", Vec::new());
    let got = ctx.provider.fetch_attachment(&mid, "2").await.unwrap();
    assert!(got.is_empty(), "zero-byte part returns Ok(empty)");
}

#[tokio::test]
async fn p1_t02_unsolicited_wrong_uid_returns_no_bytes() {
    let ctx = synced().await;
    let (mid, _dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    ctx.fake.state.lock().unwrap().behavior.wrong_uid_response = true;
    let e = ctx.provider.fetch_attachment(&mid, "2").await.unwrap_err();
    assert_eq!(err_code(&e), "attachment_message_missing");
    ctx.fake.state.lock().unwrap().behavior.wrong_uid_response = false;
}

#[tokio::test]
async fn p1_t02_missing_section_data_is_part_missing() {
    let ctx = synced().await;
    let (mid, _dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    ctx.fake.state.lock().unwrap().behavior.omit_section_data = true;
    let e = ctx.provider.fetch_attachment(&mid, "2").await.unwrap_err();
    assert_eq!(err_code(&e), "attachment_part_missing");
    ctx.fake.state.lock().unwrap().behavior.omit_section_data = false;
}

#[tokio::test]
async fn p1_t02_fragmented_literals_and_delayed_completion_still_decode() {
    let ctx = synced().await;
    let (mid, dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    {
        let mut st = ctx.fake.state.lock().unwrap();
        st.behavior.fragmented_literals = 7;
        st.behavior.completion_delay_ms = 30;
    }
    let payload = b"FRAGMENTED-PAYLOAD-0123456789".to_vec();
    ctx.fake.set_section_bytes(
        dec,
        "2",
        base64::engine::general_purpose::STANDARD
            .encode(&payload)
            .into_bytes(),
    );
    let got = ctx.provider.fetch_attachment(&mid, "2").await.unwrap();
    assert_eq!(got, payload);
}

// -- P1.3: selected state and replay restrictions ---------------------------

#[tokio::test]
async fn p1_t03_disconnect_after_select_restores_order() {
    let ctx = synced().await;
    let (mid, _dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    let uid = uid_in(&ctx.db, &ctx.acc.id, "all", &mid).await;
    let before = ctx.fake.commands().len();
    ctx.fake
        .state
        .lock()
        .unwrap()
        .behavior
        .disconnect_after_select = true;

    let got = ctx.provider.fetch_attachment(&mid, "2").await.unwrap();

    let seq: Vec<String> = ctx
        .fake
        .commands()
        .into_iter()
        .skip(before)
        .filter(|c| {
            c.starts_with("LOGIN")
                || c.starts_with("EXAMINE")
                || c.starts_with("SELECT")
                || c.starts_with("UID FETCH")
        })
        .collect();
    let all = ctx
        .db
        .imap_get_folder(&ctx.acc.id, "all")
        .await
        .unwrap()
        .unwrap()
        .name;
    // The foreground lease opens a connection (LOGIN), selects, and the
    // server drops it right after EXAMINE; recovery must log in again and
    // re-EXAMINE before any FETCH is reissued.
    assert_eq!(
        seq,
        vec![
            "LOGIN <redacted>".to_string(),
            format!("EXAMINE {all:?}"),
            "LOGIN <redacted>".to_string(),
            format!("EXAMINE {all:?}"),
            format!("UID FETCH {uid} (UID X-GM-MSGID BODYSTRUCTURE)"),
            format!("UID FETCH {uid} (UID BODY.PEEK[2])"),
        ],
        "LOGIN -> EXAMINE -> FETCH must be restored after the drop"
    );
    // No FETCH may appear on a connection that has not selected first.
    let mut selected_since_login = false;
    for c in &seq {
        if c.starts_with("LOGIN") {
            selected_since_login = false;
        } else if c.starts_with("EXAMINE") || c.starts_with("SELECT") {
            selected_since_login = true;
        } else if c.starts_with("UID FETCH") {
            assert!(selected_since_login, "FETCH before EXAMINE: {seq:?}");
        }
    }
    assert!(!got.is_empty());
}

#[tokio::test]
async fn p1_t03_epoch_change_on_reconnect_fetches_nothing() {
    let ctx = synced().await;
    let (mid, _dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    let uid = uid_in(&ctx.db, &ctx.acc.id, "all", &mid).await;
    let all = ctx
        .db
        .imap_get_folder(&ctx.acc.id, "all")
        .await
        .unwrap()
        .unwrap();

    let mut conn = ctx.pool.fresh_conn().await.unwrap();
    let info = conn.select(&all.name, true).await.unwrap();
    // The epoch moves while the connection is down.
    ctx.fake
        .state
        .lock()
        .unwrap()
        .uidvalidity
        .insert("all".into(), info.uidvalidity + 1);
    let before = ctx.fake.commands().len();
    {
        let mut st = ctx.fake.state.lock().unwrap();
        st.behavior.drop_after_n = Some(st.cmd_count + 1);
    }
    let e = conn.uid_fetch(&uid.to_string(), "(UID)").await.unwrap_err();
    assert_eq!(
        err_code(&e),
        "imap_uidvalidity_changed",
        "reconnect onto a new epoch must not reuse the old UID"
    );
    // Exactly one FETCH was attempted (the dropped one); no reissue.
    let fetches: Vec<String> = ctx
        .fake
        .commands()
        .into_iter()
        .skip(before)
        .filter(|c| c.starts_with("UID FETCH"))
        .collect();
    assert_eq!(fetches.len(), 1, "no FETCH of the old UID: {fetches:?}");
}

#[tokio::test]
async fn p1_t03_mutation_disconnect_is_not_replayed() {
    let ctx = synced().await;
    let (mid, _dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    let uid = uid_in(&ctx.db, &ctx.acc.id, "all", &mid).await;
    let all = ctx
        .db
        .imap_get_folder(&ctx.acc.id, "all")
        .await
        .unwrap()
        .unwrap();

    let mut conn = ctx.pool.fresh_conn().await.unwrap();
    conn.select(&all.name, false).await.unwrap();
    ctx.fake
        .state
        .lock()
        .unwrap()
        .behavior
        .disconnect_after_mutation = true;
    let e = conn
        .uid_store(&uid.to_string(), "+FLAGS", &["\\Flagged".to_string()])
        .await
        .unwrap_err();
    assert!(
        err_code(&e) == "imap_transient" || err_code(&e) == "offline",
        "connection loss surfaces as a transport error, got {e:?}"
    );
    let stores = ctx.fake.commands_with_prefix("UID STORE");
    assert_eq!(
        stores.len(),
        1,
        "a mutation that may have been accepted must not be replayed: {stores:?}"
    );
}

// -- P1.4: identity-verified locator resolution -----------------------------

#[tokio::test]
async fn p1_t04_stale_cached_uid_is_rediscovered_by_gmmsgid() {
    let ctx = synced().await;
    let (mid, dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    let old_uid = uid_in(&ctx.db, &ctx.acc.id, "all", &mid).await;
    let msgs_before: i64 = ctx
        .db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?))
        .await
        .unwrap();
    let atts_before: i64 = ctx
        .db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM attachments", [], |r| r.get(0))?))
        .await
        .unwrap();

    // Gmail-side move: the message leaves All Mail and gets a fresh UID in
    // Trash. The local cache still points at the old (all, uid) mapping.
    {
        let mut st = ctx.fake.state.lock().unwrap();
        let m = st.msgs.get_mut(&dec).unwrap();
        m.folders.remove("all");
        m.folders.insert("trash".into(), 9001);
    }

    let got = ctx.provider.fetch_attachment(&mid, "2").await.unwrap();
    assert!(!got.is_empty());

    let pairs = ctx.db.uids_for_message(&ctx.acc.id, &mid).await.unwrap();
    assert!(
        pairs.contains(&("trash".to_string(), 9001)),
        "verified locator must be persisted: {pairs:?}"
    );
    assert!(
        !pairs.iter().any(|(r, u)| r == "all" && *u == old_uid),
        "stale mapping must be dropped: {pairs:?}"
    );
    // Never delete bodies or cached mail.
    let msgs_after: i64 = ctx
        .db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?))
        .await
        .unwrap();
    let atts_after: i64 = ctx
        .db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM attachments", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(msgs_after, msgs_before, "messages must be preserved");
    assert_eq!(atts_after, atts_before, "attachment rows must be preserved");
}

#[tokio::test]
async fn p1_t04_uidvalidity_reset_invalidates_only_uids() {
    let ctx = synced().await;
    let (mid, _dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    let msgs_before: i64 = ctx
        .db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?))
        .await
        .unwrap();
    {
        let mut st = ctx.fake.state.lock().unwrap();
        let v = st.uidvalidity.get("all").copied().unwrap_or(987654);
        st.uidvalidity.insert("all".into(), v + 1);
    }
    let got = ctx.provider.fetch_attachment(&mid, "2").await.unwrap();
    assert!(!got.is_empty(), "rediscovered after the epoch reset");
    let msgs_after: i64 = ctx
        .db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(msgs_after, msgs_before);
    let pairs = ctx.db.uids_for_message(&ctx.acc.id, &mid).await.unwrap();
    assert!(
        pairs.iter().any(|(r, _)| r == "all"),
        "verified mapping re-persisted: {pairs:?}"
    );
}

#[tokio::test]
async fn p1_t04_deleted_message_reports_message_missing() {
    let ctx = synced().await;
    let e = ctx
        .provider
        .fetch_attachment("fedcba9876543210", "2")
        .await
        .unwrap_err();
    assert_eq!(err_code(&e), "attachment_message_missing");
}

// -- P1.5: bounded foreground connection + dedup ----------------------------

#[tokio::test]
async fn p1_t05_repeated_click_creates_one_transfer() {
    let ctx = synced().await;
    let (mid, dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    ctx.fake.set_section_bytes(
        dec,
        "2",
        base64::engine::general_purpose::STANDARD
            .encode(b"SHARED")
            .into_bytes(),
    );
    let (a, b) = tokio::join!(
        ctx.provider.fetch_attachment(&mid, "2"),
        ctx.provider.fetch_attachment(&mid, "2"),
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert_eq!(a, b"SHARED".to_vec());
    assert_eq!(b, b"SHARED".to_vec());
    let section_fetches: Vec<String> = ctx
        .fake
        .commands()
        .into_iter()
        .filter(|c| c.starts_with("UID FETCH") && c.contains("BODY.PEEK[2]"))
        .collect();
    assert_eq!(
        section_fetches.len(),
        1,
        "simultaneous requests must share one transfer: {section_fetches:?}"
    );
    // Sift owns at most three connections per account (worker + idle + lease).
    assert!(
        ctx.fake.state.lock().unwrap().connections <= 3,
        "connection cap exceeded"
    );
}

#[tokio::test]
async fn p1_t05_consumer_cancel_does_not_cancel_shared_download() {
    let ctx = synced().await;
    let (mid, dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    ctx.fake.set_section_bytes(
        dec,
        "2",
        base64::engine::general_purpose::STANDARD
            .encode(b"CANCELLABLE")
            .into_bytes(),
    );
    ctx.fake
        .state
        .lock()
        .unwrap()
        .behavior
        .completion_delay_ms = 400;

    let p1 = ctx.provider.clone();
    let p2 = ctx.provider.clone();
    let m1 = mid.clone();
    let m2 = mid.clone();
    let h1 = tokio::spawn(async move { p1.fetch_attachment(&m1, "2").await });
    let h2 = tokio::spawn(async move { p2.fetch_attachment(&m2, "2").await });
    // Let both consumers join the same transfer, then cancel one.
    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
    h1.abort();
    let got = h2.await.unwrap().unwrap();
    assert_eq!(got, b"CANCELLABLE".to_vec());

    let section_fetches: Vec<String> = ctx
        .fake
        .commands()
        .into_iter()
        .filter(|c| c.starts_with("UID FETCH") && c.contains("BODY.PEEK[2]"))
        .collect();
    assert_eq!(
        section_fetches.len(),
        1,
        "a cancelled consumer must not cancel the shared transfer: {section_fetches:?}"
    );
}

// -- P1.6: error codes ------------------------------------------------------

#[tokio::test]
async fn p1_t06_tagged_bad_is_terminal_and_not_retried() {
    let ctx = synced().await;
    let (mid, dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    let subject = ctx
        .fake
        .state
        .lock()
        .unwrap()
        .msgs
        .get(&dec)
        .unwrap()
        .subject
        .clone();
    let before = ctx.fake.commands().len();
    ctx.fake.state.lock().unwrap().behavior.fetch_bad = true;
    let e = ctx.provider.fetch_attachment(&mid, "2").await.unwrap_err();
    assert_eq!(err_code(&e), "attachment_locator_invalid");
    assert!(!err_retryable(&e), "BAD is terminal");
    let fetches: Vec<String> = ctx
        .fake
        .commands()
        .into_iter()
        .skip(before)
        .filter(|c| c.starts_with("UID FETCH"))
        .collect();
    assert_eq!(fetches.len(), 1, "a BAD syntax error is not retried");
    let msg = err_message(&e);
    assert!(msg.contains("BAD at"), "redacted developer detail: {msg}");
    assert!(
        !msg.contains(&subject),
        "detail must not leak the subject: {msg}"
    );
    assert!(
        !msg.contains("Alle") && !msg.contains("Gmail]/"),
        "detail must not leak folder names: {msg}"
    );
}

#[tokio::test]
async fn p1_t06_transient_no_is_bounded_retry() {
    let ctx = synced().await;
    let (mid, _dec) = pdf_message(&ctx.db, &ctx.acc.id).await;
    ctx.fake.state.lock().unwrap().behavior.fetch_no = true;
    let e = ctx.provider.fetch_attachment(&mid, "2").await.unwrap_err();
    assert_eq!(err_code(&e), "attachment_timeout");
    assert!(err_retryable(&e), "a transient NO is retryable");
    assert!(err_message(&e).contains("NO at"));
}

// -- raw IMAP client for the strict-server test -----------------------------

async fn read_until_tag(
    r: &mut tokio::io::BufReader<tokio::net::tcp::OwnedReadHalf>,
    tag: Option<&str>,
    out: &mut Vec<String>,
) {
    use tokio::io::AsyncBufReadExt;
    loop {
        let mut buf = Vec::new();
        let n = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            r.read_until(b'\n', &mut buf),
        )
        .await
        .expect("server response in time")
        .unwrap();
        if n == 0 {
            return;
        }
        let line = String::from_utf8_lossy(&buf).trim_end().to_string();
        out.push(line.clone());
        if let Some(tag) = tag {
            if line.starts_with(&format!("{tag} ")) {
                return;
            }
        } else {
            return;
        }
    }
}

async fn raw_exchange(port: u16, cmds: &[&str]) -> Vec<String> {
    use tokio::io::AsyncWriteExt;
    let sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let (rh, mut wh) = sock.into_split();
    let mut r = tokio::io::BufReader::new(rh);
    let mut out = vec![];
    read_until_tag(&mut r, None, &mut out).await; // greeting
    for (i, c) in cmds.iter().enumerate() {
        let tag = format!("t{}", i + 1);
        wh.write_all(format!("{tag} {c}\r\n").as_bytes())
            .await
            .unwrap();
        wh.flush().await.unwrap();
        read_until_tag(&mut r, Some(&tag), &mut out).await;
    }
    out
}

fn line_for(lines: &[String], tag: &str) -> String {
    lines
        .iter()
        .find(|l| l.starts_with(&format!("{tag} ")))
        .cloned()
        .unwrap_or_default()
}
