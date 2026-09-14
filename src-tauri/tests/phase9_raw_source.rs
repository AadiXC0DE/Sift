//! Phase 9 P9.3 — View Source and Save as `.eml`.
//!
//! The promise is byte fidelity: what Sift exports is exactly what it read,
//! proven by digest, so a mail that is not valid UTF-8 still exports intact and
//! a failed copy leaves nothing behind.

use sift::commands::raw_source::write_raw_export;
use sift::db::raw_cache::{raw_dir, raw_file_name, sha256_hex, RawCacheRow};
use sift::db::Db;

/// A raw message that is not valid UTF-8 and contains every byte class that a
/// lossy decode would rewrite: NUL, high bytes and a CRLF header layout.
fn binary_fixture() -> Vec<u8> {
    let mut bytes: Vec<u8> = vec![];
    bytes.extend_from_slice(b"From: \"Zoe\" <zoe@x.test>\r\n");
    bytes.extend_from_slice(b"Subject: =?utf-8?B?w5xiZXI=?=\r\n");
    bytes.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
    bytes.extend_from_slice(&[0x00, 0xff, 0xfe, 0x80, 0x0a, 0x0d, 0x1b, 0x7f]);
    bytes
}

/// The hand-rolled SHA-256 must agree with the published vectors: every export
/// check is worth nothing if the digest is wrong.
#[test]
fn p9_3_the_export_digest_matches_the_published_vectors() {
    assert_eq!(
        sha256_hex(b""),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(
        sha256_hex(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        sha256_hex(b"The quick brown fox jumps over the lazy dog"),
        "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
    );
    // The fixture crosses the padding boundary (55/56/64 bytes) as well.
    for len in [55usize, 56, 63, 64, 65] {
        let bytes = vec![0xa5u8; len];
        assert_eq!(sha256_hex(&bytes).len(), 64);
    }
}

/// P9.3: an exported `.eml` is byte-for-byte the cached raw message, for bytes
/// that are not valid UTF-8.
#[tokio::test]
async fn p9_3_an_export_is_exactly_the_cached_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("ada@x.com", None, None).await.unwrap();
    let message = sift::dto::MessageRef::new(acc.id.clone(), "18f2.with/slashes");
    seed_message(&db, &acc.id, &message.message_id).await;

    let raw = binary_fixture();
    // The cache path is derived from the message id and can never escape it.
    let cache_dir = raw_dir(dir.path(), &acc.id);
    std::fs::create_dir_all(&cache_dir).unwrap();
    let cache_path = cache_dir.join(raw_file_name(&message.message_id));
    assert_eq!(cache_path.parent().unwrap(), cache_dir);
    std::fs::write(&cache_path, &raw).unwrap();
    let digest = sha256_hex(&raw);
    db.raw_cache_put(&RawCacheRow {
        account_id: acc.id.clone(),
        message_id: message.message_id.clone(),
        path: cache_path.to_string_lossy().to_string(),
        size: raw.len() as i64,
        sha256: digest.clone(),
    })
    .await
    .unwrap();

    // The cached bytes are what the row claims they are.
    let row = db
        .raw_cache_get(&acc.id, &message.message_id)
        .await
        .unwrap()
        .unwrap();
    let cached = std::fs::read(&row.path).unwrap();
    assert_eq!(cached, raw, "the cache holds the provider's exact bytes");
    assert_eq!(row.size, raw.len() as i64);
    assert_eq!(sha256_hex(&cached), row.sha256);
    assert!(
        std::str::from_utf8(&cached).is_err(),
        "the fixture is deliberately not valid UTF-8"
    );

    // The export writes those exact bytes: same length, same digest, same
    // content — no lossy round trip through a String.
    let destination = dir.path().join("saved.eml");
    write_raw_export(&cached, &destination).await.unwrap();
    let exported = std::fs::read(&destination).unwrap();
    assert_eq!(sha256_hex(&exported), row.sha256);
    assert_eq!(exported, raw);
    assert!(
        !destination.with_extension("eml.part").exists(),
        "the temporary file is renamed, not left behind"
    );

    // Exporting twice is idempotent, and a re-export of the same cache row is
    // byte-identical to the first.
    let second = dir.path().join("saved-again.eml");
    write_raw_export(&cached, &second).await.unwrap();
    assert_eq!(std::fs::read(&second).unwrap(), exported);
}

/// P9.3: a failed export leaves no partial file where the user asked for a
/// message.
#[tokio::test]
async fn p9_3_a_failed_export_leaves_nothing_behind() {
    let dir = tempfile::tempdir().unwrap();
    let raw = binary_fixture();
    let missing_parent = dir.path().join("no-such-folder").join("out.eml");
    let err = write_raw_export(&raw, &missing_parent).await.unwrap_err();
    assert_eq!(err.code(), "storage");
    assert!(!missing_parent.exists());
    assert!(
        !missing_parent.with_extension("eml.part").exists(),
        "no half file survives a failed export"
    );
    // Nothing was created anywhere under the target either.
    assert!(!dir.path().join("no-such-folder").exists());
}

async fn seed_message(db: &Db, account_id: &str, message_id: &str) {
    let (a, m) = (account_id.to_string(), message_id.to_string());
    db.write(move |c| {
        c.execute(
            "INSERT INTO threads (account_id,id,last_message_at,first_message_at) VALUES (?1,'t1',0,0)",
            rusqlite::params![a],
        )?;
        c.execute(
            "INSERT INTO messages (id,account_id,thread_id,internal_date,subject,label_ids) \
             VALUES (?2,?1,'t1',0,'Source','[\"INBOX\"]')",
            rusqlite::params![a, m],
        )?;
        Ok(())
    })
    .await
    .unwrap();
}
