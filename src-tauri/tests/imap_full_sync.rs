//! P11-T05: full sync against the 2,000-message fake (LOGIN→LIST→SELECT→
//! SEARCH→FETCH→snippets→Trash/Junk→cursors), chunked progress, resume.
#[path = "support/mod.rs"]
mod support;

use sift::db::Db;
use sift::provider::imap::{folders, full};
use sift::provider::{DbSink, SyncEvent};
use std::sync::{Arc, Mutex};

fn pool_for(port: u16) -> sift::provider::imap::conn::ImapPool {
    sift::provider::imap::conn::ImapPool::new(
        "user@gmail.com".into(),
        "goodpassword0000".into(),
        "127.0.0.1".into(),
        port,
        true,
    )
}

#[tokio::test]
async fn p11_t05_full_sync_2k() {
    let fake = support::fake_imap::FakeGmail::start().await;
    let pool = pool_for(fake.addr.port());
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("user@gmail.com", None, None).await.unwrap();

    let folders = {
        let mut guard = pool.worker().await.unwrap();
        folders::discover(guard.as_mut().unwrap()).await.unwrap()
    };
    assert!(folders.all.contains("Alle Nachrichten"));

    let events: Arc<Mutex<Vec<SyncEvent>>> = Arc::new(Mutex::new(vec![]));
    let ev2 = events.clone();
    let sink = DbSink::with_events(db.clone(), move |e| {
        ev2.lock().unwrap().push(e);
    });
    let cancel = tokio_util::sync::CancellationToken::new();
    let cursor = full::run_full_sync(&pool, &sink, &acc.id, &folders, cancel)
        .await
        .unwrap();
    let sift::provider::Cursor::Imap { folders: curs } = cursor else {
        panic!("want IMAP cursor");
    };
    assert_eq!(curs.len(), 3);

    // all messages, threads, labels, attachments present
    let nm: i64 = db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(nm, 2000);
    let nt: i64 = db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM threads", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert!(nt > 1000, "threads: {nt}");
    let nl: i64 = db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM labels", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert!(nl >= 8, "labels: {nl}");
    let na: i64 = db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM attachments", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert!(na > 100, "attachments: {na}");

    // snippets present for every \All message (fixture < 5,000)
    let nosnip: i64 = db
        .read(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM messages m WHERE m.snippet='' AND EXISTS (SELECT 1 FROM imap_uids u WHERE u.account_id=m.account_id AND u.message_id=m.id AND u.role='all')",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(nosnip, 0, "all-folder messages must have snippets");

    // store:threads fired per 500-UID chunk, newest first
    let chunks: Vec<_> = {
        let ev = events.lock().unwrap();
        ev.iter()
            .filter_map(|e| match e {
                SyncEvent::ThreadsChanged { thread_ids, .. } => Some(thread_ids.len()),
                _ => None,
            })
            .collect()
    };
    assert!(chunks.len() >= 4, "chunks: {chunks:?}");
    assert!(chunks.iter().all(|&n| n > 0 && n <= 500));

    // cursors match the server
    for role in ["all", "trash", "junk"] {
        let cur = db
            .imap_get_folder(&acc.id, role)
            .await
            .unwrap()
            .expect(role);
        assert_eq!(cur.uidvalidity, 987654);
        assert!(cur.uidnext > 1);
        assert!(cur.last_full_scan.is_some());
    }
}

#[tokio::test]
async fn p11_t05_resume_after_kill() {
    let fake = support::fake_imap::FakeGmail::start().await;
    let pool = pool_for(fake.addr.port());
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("user@gmail.com", None, None).await.unwrap();
    let folders = {
        let mut guard = pool.worker().await.unwrap();
        folders::discover(guard.as_mut().unwrap()).await.unwrap()
    };
    // Cancel after 2 metadata chunks, then resume to completion.
    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel2 = cancel.clone();
    let seen = Arc::new(Mutex::new(0usize));
    let seen2 = seen.clone();
    let sink = DbSink::with_events(db.clone(), move |e| {
        if matches!(e, SyncEvent::Progress(s) if s.phase == "metadata") {
            let mut n = seen2.lock().unwrap();
            *n += 1;
            if *n >= 2 {
                cancel2.cancel();
            }
        }
    });
    let r = full::run_full_sync(&pool, &sink, &acc.id, &folders, cancel).await;
    assert!(r.is_err(), "cancelled run must error, got {r:?}");
    let partial: i64 = db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert!(
        partial > 0 && partial < 2000,
        "partial progress expected, got {partial}"
    );

    // Count metadata FETCHes (BODYSTRUCTURE) separately from snippet
    // partials: resume must not re-fetch stored metadata.
    let meta_fetches = || {
        fake.state
            .lock()
            .unwrap()
            .commands_seen
            .iter()
            .filter(|c| c.starts_with("UID FETCH") && c.contains("BODYSTRUCTURE"))
            .count()
    };
    let fetches_before = meta_fetches();
    let sink2 = DbSink::new(db.clone());
    full::run_full_sync(
        &pool,
        &sink2,
        &acc.id,
        &folders,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    let nm: i64 = db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(nm, 2000, "resume completes without duplicates");
    let fetches_after = meta_fetches();
    // Remainder after 2 stored chunks: 2 \All chunks + trash + junk = 4.
    assert!(
        fetches_after - fetches_before <= 4,
        "resume must not re-fetch stored chunks ({} new metadata fetches)",
        fetches_after - fetches_before
    );
}
