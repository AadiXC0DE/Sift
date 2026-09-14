//! P2.8 live acceptance against a real Gmail account.
//!
//! Ignored by default: this is the only test in the suite that talks to a real
//! mailbox, and it must never run in CI or on a machine without credentials.
//! It exists so that the one part of Phase 2 that cannot be proven offline —
//! "a real attachment comes down byte-exact from Gmail" — has a reproducible
//! command instead of a claim.
//!
//! Run it with a disposable / test mailbox and an app password:
//!
//! ```text
//! SIFT_LIVE_EMAIL=you@example.com \
//! SIFT_LIVE_APP_PASSWORD=xxxxxxxxxxxxxxxx \
//! SIFT_LIVE_MESSAGE_ID=<gmail hex id> \
//! SIFT_LIVE_PART=<imap section, e.g. 2> \
//! cargo test --manifest-path src-tauri/Cargo.toml \
//!   --test live_gmail_acceptance -- --ignored --nocapture
//! ```
//!
//! Optional, and the reason this harness is worth running:
//!   SIFT_LIVE_EXPECTED_SHA256=<sha256 of the sender's original file>
//!   SIFT_LIVE_EXPECTED_SIZE=<decoded byte count>
//!   SIFT_LIVE_EXPECTED_FILENAME=<name you expect the part to carry>
//!
//! Everything it prints is measured; anything it cannot check is reported as
//! not checked with the reason. It never sends mail, never applies labels and
//! never modifies the mailbox: the fetch path uses `BODY.PEEK`.
//!
//! What this harness does NOT cover, and why: `\Seen` flag verification needs
//! the raw connection (not part of the public provider API) and is asserted
//! offline by the strict fake server, which rejects any non-PEEK fetch of a
//! section; the cache/offline second save is a service-level path covered by
//! `attachment_lifecycle.rs` against a scripted transport, and would need the
//! running app's data directory to reproduce here.

use sift::db::Db;
use sift::provider::imap::conn::ImapPool;
use sift::provider::imap::provider::GmailImapProvider;
use sift::provider::Provider;

struct LiveConfig {
    email: String,
    password: String,
    message_id: String,
    part: String,
    expected_sha256: Option<String>,
    expected_size: Option<u64>,
    expected_filename: Option<String>,
}

fn config() -> Option<LiveConfig> {
    let email = std::env::var("SIFT_LIVE_EMAIL").ok()?;
    let password = std::env::var("SIFT_LIVE_APP_PASSWORD").ok()?;
    let message_id = std::env::var("SIFT_LIVE_MESSAGE_ID").ok()?;
    let part = std::env::var("SIFT_LIVE_PART").ok()?;
    Some(LiveConfig {
        email,
        password,
        message_id,
        part,
        expected_sha256: std::env::var("SIFT_LIVE_EXPECTED_SHA256")
            .ok()
            .map(|v| v.trim().to_lowercase()),
        expected_size: std::env::var("SIFT_LIVE_EXPECTED_SIZE")
            .ok()
            .and_then(|v| v.parse().ok()),
        expected_filename: std::env::var("SIFT_LIVE_EXPECTED_FILENAME").ok(),
    })
}

/// Reuses the crate's own SHA-256 (the same one the raw-source cache uses) so
/// the acceptance harness does not pull a new dependency into the test build.
fn sha256_hex(bytes: &[u8]) -> String {
    sift::db::raw_cache::sha256_hex(bytes)
}

/// The address is never printed in full: this runs against a real account and
/// the output of an acceptance run can end up pasted into a review or a log.
fn redact(email: &str) -> String {
    match email.split_once('@') {
        Some((local, domain)) => {
            let head: String = local.chars().take(2).collect();
            format!("{head}***@{domain}")
        }
        None => "***".into(),
    }
}

fn announce(label: &str, value: impl std::fmt::Display) {
    println!("{label:<28} {value}");
}

#[tokio::test]
#[ignore = "live network + real credentials: run explicitly with SIFT_LIVE_* set"]
async fn p2_t08_live_gmail_attachment_is_byte_exact() {
    let Some(cfg) = config() else {
        eprintln!(
            "SKIPPED: set SIFT_LIVE_EMAIL, SIFT_LIVE_APP_PASSWORD, SIFT_LIVE_MESSAGE_ID and \
             SIFT_LIVE_PART to run the live acceptance."
        );
        return;
    };

    // A scratch database: the harness must not touch the user's real mirror.
    let dir = tempfile::tempdir().expect("temp dir");
    let db = Db::open(dir.path()).expect("open scratch db");
    let account = db
        .new_account(&cfg.email, None, None)
        .await
        .expect("insert account");

    // Locator persistence references the message row in the real app. Seed
    // that identity in this scratch mirror before asking the provider to fetch.
    db.messages_upsert(sift::db::messages::MsgUpsert {
        id: cfg.message_id.clone(),
        account_id: account.id.clone(),
        thread_id: cfg.message_id.clone(),
        ..Default::default()
    })
    .await
    .expect("seed attachment message identity");

    let pool = ImapPool::gmail(cfg.email.clone(), cfg.password.clone());
    let provider = GmailImapProvider::new(account.id.clone(), pool.clone(), db.clone());
    let folders = provider
        .folders_cached()
        .await
        .expect("discover mailbox folders");
    for role in ["all", "trash", "junk"] {
        let Some(name) = folders.name_for_role(role) else {
            continue;
        };
        let info = {
            let lease = pool
                .with_selected_worker(name, true, &tokio_util::sync::CancellationToken::new())
                .await
                .expect("examine folder");
            lease.info().clone()
        };
        db.imap_set_folder(
            &account.id,
            &sift::db::imap::FolderCursor {
                role: role.into(),
                name: name.into(),
                uidvalidity: info.uidvalidity as i64,
                uidnext: 1,
                highestmodseq: None,
                exists_count: info.exists as i64,
                last_full_scan: None,
            },
        )
        .await
        .expect("seed locator folder parent");
    }

    println!("\n=== P2.8 live Gmail attachment acceptance ===");
    announce("account", redact(&cfg.email));
    announce("message id", &cfg.message_id);
    announce("section", &cfg.part);

    // 1. The credentials and the transport actually work.
    let profile = provider.verify().await.expect("IMAP login must succeed");
    announce("login", format!("ok ({} reported)", redact(&profile.email)));

    // 2. The path the bug report was about: fetch a section over the wire.
    let started = std::time::Instant::now();
    let bytes = provider
        .fetch_attachment(&cfg.message_id, &cfg.part)
        .await
        .expect("fetch_attachment must succeed against real Gmail");
    let elapsed = started.elapsed();
    announce(
        "downloaded",
        format!("{} bytes in {elapsed:?}", bytes.len()),
    );

    let sha = sha256_hex(&bytes);
    announce("sha256", &sha);

    // 3. A second fetch must return the same bytes. This is what proves the
    //    locator resolution is stable rather than accidentally right once.
    let again = provider
        .fetch_attachment(&cfg.message_id, &cfg.part)
        .await
        .expect("second fetch must succeed");
    assert_eq!(
        sha256_hex(&again),
        sha,
        "two fetches of the same part must be byte-identical"
    );
    announce("second fetch", "byte-identical");

    // 4. Compare against the sender's original when the caller supplied it.
    match &cfg.expected_sha256 {
        Some(expected) => {
            assert_eq!(
                &sha, expected,
                "downloaded bytes must match the sender's original file"
            );
            announce("sha256 match", "yes (against SIFT_LIVE_EXPECTED_SHA256)");
        }
        None => announce(
            "sha256 match",
            "NOT CHECKED - set SIFT_LIVE_EXPECTED_SHA256 to the original file's hash",
        ),
    }
    match cfg.expected_size {
        Some(expected) => {
            assert_eq!(
                bytes.len() as u64,
                expected,
                "decoded length must match the sender's original file"
            );
            announce("size match", "yes");
        }
        None => announce(
            "size match",
            "NOT CHECKED - set SIFT_LIVE_EXPECTED_SIZE to the original file's length",
        ),
    }

    // 5. A zero-byte part is a real attachment, not a failure (ATT-07).
    if bytes.is_empty() {
        println!("NOTE: this part decoded to zero bytes; it is reported as a ready empty file.");
    }

    if let Some(name) = &cfg.expected_filename {
        println!(
            "NOTE: expected display name {name:?} - confirm it in the app's attachment strip; \
             the provider returns bytes only, so the name is asserted by the ingest tests."
        );
    }

    println!("=== live acceptance finished; nothing was modified in the mailbox ===");
}
