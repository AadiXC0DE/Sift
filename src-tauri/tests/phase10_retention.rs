//! Phase 10 P10.4 — cache cleanup, disk usage and recovery.
//!
//! The rules that matter: eviction takes the least recently used file and
//! nothing else; a pinned file, a file being read, a user-saved copy and
//! anything a draft or a pending send owns are out of reach; and the numbers
//! the Storage panel shows are the files that are actually on disk.

use std::time::{Duration, SystemTime};

use sift::attachments::in_use;
use sift::db::Db;

/// One cached attachment with a real file on disk.
#[allow(clippy::too_many_arguments)]
async fn seed_attachment(
    db: &Db,
    data_dir: &std::path::Path,
    account_id: &str,
    id: &str,
    message_id: &str,
    bytes: usize,
    accessed_at: i64,
    pinned: bool,
) -> std::path::PathBuf {
    let dir = data_dir.join("attachments").join(account_id).join(id);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("file.bin");
    std::fs::write(&path, vec![7u8; bytes]).unwrap();
    let (a, i, m, p) = (
        account_id.to_string(),
        id.to_string(),
        message_id.to_string(),
        path.to_string_lossy().to_string(),
    );
    db.write(move |c| {
        c.execute(
            "INSERT OR REPLACE INTO attachments (account_id,id,message_id,part_id,mime,size,decoded_size,local_path,cache_state,last_accessed_at,pinned_at) \
             VALUES (?7,?1,?2,?1,'application/octet-stream',?3,?3,?4,'ready',?5,?6)",
            rusqlite::params![i, m, bytes as i64, p, accessed_at, if pinned { Some(1i64) } else { None }, a],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    path
}

async fn fixture() -> (tempfile::TempDir, Db, String) {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("ada@x.com", None, None).await.unwrap();
    let id = acc.id.clone();
    db.write(move |c| {
        c.execute(
            "INSERT INTO threads (account_id,id,last_message_at,first_message_at) VALUES (?1,'t1',0,0)",
            rusqlite::params![acc.id],
        )?;
        c.execute(
            "INSERT INTO messages (id,account_id,thread_id,internal_date,label_ids) VALUES ('m1',?1,'t1',0,'[\"INBOX\"]')",
            rusqlite::params![acc.id],
        )?;
        Ok(())
    })
    .await
    .unwrap();
    (dir, db, id)
}

/// P10.4: the least recently used file goes first, a pinned file never goes,
/// and a file being read right now is skipped rather than deleted.
#[tokio::test]
async fn p10_4_eviction_is_lru_and_spares_pinned_and_in_use_files() {
    let (dir, db, acc) = fixture().await;
    let oldest = seed_attachment(&db, dir.path(), &acc, "a-old", "m1", 4_000, 10, false).await;
    let middle = seed_attachment(&db, dir.path(), &acc, "b-mid", "m1", 4_000, 20, false).await;
    let newest = seed_attachment(&db, dir.path(), &acc, "c-new", "m1", 4_000, 30, false).await;
    let pinned = seed_attachment(&db, dir.path(), &acc, "d-pin", "m1", 4_000, 1, true).await;
    let reading = seed_attachment(&db, dir.path(), &acc, "e-read", "m1", 4_000, 5, false).await;

    // A reader holds the second-oldest file open.
    let lease = in_use::acquire(&acc, "e-read");
    assert!(in_use::is_in_use(&acc, "e-read"));

    // 16 KiB cached, cap it at 12 KiB: 4 KiB has to go.
    let report = sift::retention::enforce_attachment_cap(&db, 12 * 1024)
        .await
        .unwrap();
    assert_eq!(report.evicted, 1);
    assert_eq!(report.bytes_freed, 4_000);
    assert!(report.trimmed);

    assert!(!oldest.exists(), "the least recently used file goes first");
    assert!(middle.exists(), "the cap is met without taking more");
    assert!(newest.exists());
    assert!(pinned.exists(), "a pin means keep this offline");
    assert!(reading.exists(), "a file being read is not deleted");

    // The report explains the difference between "over the cap" and "wrong".
    assert_eq!(report.exempt_bytes, 4_000 + 4_000, "pinned plus in use");

    // A row that lost its file no longer claims one, and can be downloaded
    // again: the metadata is untouched.
    let forgotten = db
        .evictable_attachments()
        .await
        .unwrap()
        .into_iter()
        .find(|c| c.attachment_id == "a-old");
    assert!(forgotten.is_none(), "an evicted row is no longer a candidate");
    let (state, filename): (String, String) = db
        .read({
            let a = acc.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT cache_state, mime FROM attachments WHERE account_id=? AND id='a-old'",
                    rusqlite::params![a],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?)
            }
        })
        .await
        .unwrap();
    assert_eq!(state, "missing");
    assert_eq!(filename, "application/octet-stream", "identity survives eviction");

    // Releasing the lease changes nothing about what was already decided.
    drop(lease);
    assert!(!in_use::is_in_use(&acc, "e-read"));
}

/// P10.4: a cap of zero is the user asking for no downloaded attachments, and
/// an already-small cache is left alone.
#[tokio::test]
async fn p10_4_a_zero_cap_clears_the_cache_and_a_sufficient_cap_does_not() {
    let (dir, db, acc) = fixture().await;
    let file = seed_attachment(&db, dir.path(), &acc, "a", "m1", 1_000, 10, false).await;

    let no_need = sift::retention::enforce_attachment_cap(&db, 1_000_000)
        .await
        .unwrap();
    assert_eq!(no_need.evicted, 0);
    assert!(file.exists());

    let cleared = sift::retention::enforce_attachment_cap(&db, 0).await.unwrap();
    assert_eq!(cleared.evicted, 1);
    assert!(!file.exists());
}

/// P10.4: the staging sweep removes only abandoned transfer files that nothing
/// refers to. A draft's attachment and a pending send's payload are both
/// referenced, and a user-saved `.eml` is not the app's to delete at all.
#[tokio::test]
async fn p10_4_the_sweep_spares_drafts_pending_sends_and_saved_files() {
    let (dir, db, acc) = fixture().await;
    let staging = dir.path().join("compose-cache").join("draft-1");
    std::fs::create_dir_all(&staging).unwrap();
    let draft_file = staging.join("kept.bin.part-abc");
    std::fs::write(&draft_file, b"draft attachment").unwrap();

    let outbox_dir = dir.path().join("compose-cache").join("send-1");
    std::fs::create_dir_all(&outbox_dir).unwrap();
    let pending_file = outbox_dir.join("frozen.eml");
    std::fs::write(&pending_file, b"frozen mime").unwrap();

    let orphan_dir = dir.path().join("compose-cache").join("abandoned");
    std::fs::create_dir_all(&orphan_dir).unwrap();
    let orphan = orphan_dir.join("half.bin.part-dead");
    std::fs::write(&orphan, b"left behind").unwrap();

    // A file the user saved themselves, outside the app's cache.
    let saved = dir.path().join("Documents").join("invoice.eml");
    std::fs::create_dir_all(saved.parent().unwrap()).unwrap();
    std::fs::write(&saved, b"user copy").unwrap();

    // A draft still owns its staging file, and a queued send still owns its
    // frozen payload. Both are named, so neither is a candidate.
    db.drafts_upsert(
        &sift::dto::Draft {
            account_id: acc.clone(),
            attachments_json: vec![sift::dto::AttachmentRef {
                name: "kept.bin".into(),
                mime: "application/octet-stream".into(),
                size: 17,
                path: draft_file.to_string_lossy().to_string(),
            }],
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    db.outbox_enqueue(
        &acc,
        "send",
        &serde_json::json!({ "rawPath": pending_file.to_string_lossy() }).to_string(),
        None,
        0,
    )
    .await
    .unwrap();

    // Age everything past the retention window: age is the only reason the
    // orphan is eligible.
    let old = SystemTime::now() - Duration::from_secs(25 * 60 * 60);
    for path in [&draft_file, &pending_file, &orphan] {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(old)
            .unwrap();
    }

    let swept = sift::retention::sweep_ephemeral(&db, dir.path(), 256)
        .await
        .unwrap();
    assert!(swept > 0);
    assert!(!orphan.exists(), "an unreferenced abandoned file is swept");
    assert!(draft_file.exists(), "a draft's attachment is never swept");
    assert!(pending_file.exists(), "a pending send's payload is never swept");
    assert!(saved.exists(), "a user-saved file is not the app's to delete");
}

/// P10.4: the Storage panel's numbers are the files that exist.
#[tokio::test]
async fn p10_4_storage_totals_equal_the_files_sift_manages() {
    let (dir, db, acc) = fixture().await;
    let in_use_file = seed_attachment(&db, dir.path(), &acc, "a", "m1", 1_500, 10, false).await;
    let pinned_file = seed_attachment(&db, dir.path(), &acc, "b", "m1", 500, 20, true).await;
    let draft_dir = dir.path().join("compose-cache").join("draft-1");
    std::fs::create_dir_all(&draft_dir).unwrap();
    std::fs::write(draft_dir.join("staged.bin"), vec![3u8; 700]).unwrap();

    let usage = sift::commands::storage::measure(&db, dir.path()).await.unwrap();
    assert_eq!(usage.attachments.bytes, 2_000);
    assert_eq!(usage.attachments.items, 2);
    assert_eq!(usage.draft_cache.bytes, 700);
    assert_eq!(usage.draft_cache.items, 1);
    assert_eq!(usage.pinned_bytes, 500, "pinned bytes are reported, not evicted");
    assert_eq!(
        usage.total_bytes,
        usage.metadata.bytes + usage.bodies.bytes + usage.attachments.bytes + usage.draft_cache.bytes,
        "the total is the sum of the categories it shows"
    );
    assert!(usage.attachment_cache_limit_bytes > 0);

    // Clearing releases the unpinned files and leaves the pin alone — and the
    // freshly measured totals say exactly that.
    let cleared = sift::commands::storage::clear_attachment_cache(&db, dir.path()).await.unwrap();
    assert!(!in_use_file.exists());
    assert!(pinned_file.exists(), "clearing the cache honours a pin");
    assert_eq!(cleared.attachments.bytes, 500);
    assert_eq!(
        cleared.attachments.items, 2,
        "the released row stays, so the file can be downloaded again"
    );
    assert_eq!(cleared.pinned_bytes, 500);
    assert_eq!(
        sift::commands::storage::dir_usage(&dir.path().join("attachments")),
        (500, 1),
        "the measured bucket and the filesystem agree"
    );
    assert_eq!(cleared.draft_cache.bytes, 700, "draft staging is a different bucket");
}

/// P10.4: removing an account removes what it owned on disk, after its rows are
/// gone — and nothing else.
#[tokio::test]
async fn p10_4_removing_an_account_removes_only_its_cache() {
    let (dir, db, acc) = fixture().await;
    let file = seed_attachment(&db, dir.path(), &acc, "a", "m1", 1_000, 10, false).await;
    let raw_dir = sift::db::raw_cache::raw_dir(dir.path(), &acc);
    std::fs::create_dir_all(&raw_dir).unwrap();
    std::fs::write(raw_dir.join("m1.eml"), b"raw").unwrap();
    let neighbour = dir.path().join("attachments").join("other-account");
    std::fs::create_dir_all(&neighbour).unwrap();
    std::fs::write(neighbour.join("keep.bin"), b"another account").unwrap();

    let draft_ids = db.accounts_remove(&acc).await.unwrap();
    sift::retention::remove_account_cache(dir.path(), &acc, &draft_ids);

    assert!(!file.exists());
    assert!(!raw_dir.exists());
    assert!(neighbour.join("keep.bin").exists(), "another account is untouched");
    assert!(db.accounts_get(&acc).await.unwrap().is_none());
    assert_eq!(sift::commands::storage::dir_usage(&dir.path().join("attachments")), (15, 1));
}
