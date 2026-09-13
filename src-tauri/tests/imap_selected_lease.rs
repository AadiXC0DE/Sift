//! P4.3: serialized selected-mailbox batches (SYNC-01).
//!
//! The invariant under test: SELECT and the FETCHes that depend on it share
//! one connection lease, so a concurrent Trash sync, a background body read
//! and a drain-triggered sync can never make a UID command run against another
//! task's mailbox. The fake server records the mailbox each FETCH actually ran
//! in, so the assertion is about the wire, not about intentions.
#[path = "support/mod.rs"]
mod support;

use sift::db::Db;
use sift::provider::imap::conn::ImapPool;
use sift::provider::imap::provider::GmailImapProvider;
use sift::provider::imap::{folders, full, message, partial};
use sift::provider::{DbSink, Provider};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

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
    folders: sift::provider::imap::folders::FolderMap,
    _dir: tempfile::TempDir,
}

async fn synced() -> Ctx {
    let fake = support::fake_imap::FakeGmail::start().await;
    let pool = pool_for(fake.addr.port());
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("user@gmail.com", None, None).await.unwrap();
    let folders = {
        let mut g = pool.worker().await.unwrap();
        folders::discover(g.as_mut().unwrap()).await.unwrap()
    };
    let sink = DbSink::new(db.clone());
    full::run_full_sync(
        &pool,
        &sink,
        &acc.id,
        &folders,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap();
    Ctx {
        fake,
        pool,
        db,
        acc,
        folders,
        _dir: dir,
    }
}

async fn prev_cursors(ctx: &Ctx) -> HashMap<String, sift::db::imap::FolderCursor> {
    let mut m = HashMap::new();
    for role in ["all", "trash", "junk"] {
        if let Some(c) = ctx.db.imap_get_folder(&ctx.acc.id, role).await.unwrap() {
            m.insert(role.into(), c);
        }
    }
    m
}

/// Members of a NARROW, concrete UID set (no `*`, at most four UIDs).
///
/// The whole-folder CHANGEDSINCE scan (`1:*`) and the 500-wide metadata chunks
/// legitimately name UIDs across the whole mailbox, so only the targeted
/// fetches (one message's metadata, body or section) can be attributed to a
/// single message.
fn narrow_set(set: &str) -> Option<Vec<u32>> {
    let mut out = vec![];
    for part in set.split(',') {
        let part = part.trim();
        if let Some((lo, hi)) = part.split_once(':') {
            let lo: u32 = lo.trim().parse().ok()?;
            let hi: u32 = hi.trim().parse().ok()?;
            if hi < lo || hi - lo > 3 {
                return None;
            }
            out.extend(lo..=hi);
        } else {
            out.push(part.parse().ok()?);
        }
    }
    (out.len() <= 4).then_some(out)
}

/// Every narrow FETCH that named `uid` must have run with `role` selected (the
/// fake records the selected mailbox by role, e.g. `all`, `trash`).
fn assert_uid_fetched_in(fake: &support::fake_imap::FakeGmail, uid: u32, role: &str) {
    let hits: Vec<(String, String)> = fake
        .fetches()
        .into_iter()
        .filter(|(_, set)| narrow_set(set).is_some_and(|m| m.contains(&uid)))
        .collect();
    assert!(
        !hits.is_empty(),
        "expected at least one narrow FETCH covering uid {uid}"
    );
    for (selected, set) in hits {
        assert_eq!(
            selected, role,
            "FETCH {set} for uid {uid} ran while {selected} was selected, not {role}"
        );
    }
}

/// Ten simultaneous worker checkouts open ONE connection (P4.3): the slot is
/// held across the connect instead of being released first.
#[tokio::test]
async fn p43_ten_simultaneous_worker_checkouts_open_one_connection() {
    let fake = support::fake_imap::FakeGmail::start().await;
    let pool = pool_for(fake.addr.port());
    let mut handles = vec![];
    for _ in 0..10 {
        let pool = pool.clone();
        handles.push(tokio::spawn(async move {
            let guard = pool.worker().await.unwrap();
            // Hold briefly so the checkouts genuinely overlap.
            tokio::time::sleep(Duration::from_millis(20)).await;
            drop(guard);
        }));
    }
    for h in handles {
        h.await.unwrap();
    }
    assert_eq!(
        fake.state.lock().unwrap().connections,
        1,
        "concurrent checkouts must share one worker connection"
    );
    let logins = fake
        .commands()
        .into_iter()
        .filter(|c| c.starts_with("LOGIN"))
        .count();
    assert_eq!(logins, 1, "the worker logged in once");
}

/// Two selected-mailbox batches: the second cannot run until the first has
/// finished its FETCH, and neither can fetch in the other's mailbox.
#[tokio::test]
async fn p43_second_batch_waits_for_the_held_lease() {
    let ctx = synced().await;
    let (all_uid, trash_uid) = {
        let st = ctx.fake.state.lock().unwrap();
        let all_uid = st
            .msgs
            .values()
            .find(|m| m.folders.contains_key("all") && !m.folders.contains_key("trash"))
            .and_then(|m| m.folders.get("all").copied())
            .expect("an All Mail message");
        let trash_uid = st
            .msgs
            .values()
            .find_map(|m| m.folders.get("trash").copied())
            .expect("a Trash message");
        (all_uid, trash_uid)
    };
    let (all_name, trash_name) = (ctx.folders.all.clone(), ctx.folders.trash.clone());
    let gate = Arc::new(tokio::sync::Notify::new());

    // Batch A: All Mail, holding the lease across a slow step (the gate).
    let a = {
        let (pool, gate) = (ctx.pool.clone(), gate.clone());
        let all_name = all_name.clone();
        tokio::spawn(async move {
            let mut w = pool
                .with_selected_worker(&all_name, true, &tokio_util::sync::CancellationToken::new())
                .await
                .unwrap();
            gate.notified().await;
            message::fetch_meta(w.conn(), &[all_uid])
                .await
                .map(|v| v.len())
                .unwrap()
        })
    };
    tokio::time::sleep(Duration::from_millis(150)).await;

    // Batch B: Trash, starting while A still holds the lease.
    let mut b = {
        let (pool, trash_name) = (ctx.pool.clone(), trash_name.clone());
        tokio::spawn(async move {
            let mut w = pool
                .with_selected_worker(
                    &trash_name,
                    true,
                    &tokio_util::sync::CancellationToken::new(),
                )
                .await
                .unwrap();
            message::fetch_meta(w.conn(), &[trash_uid])
                .await
                .map(|v| v.len())
                .unwrap()
        })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(300), &mut b)
            .await
            .is_err(),
        "the Trash batch must wait for the held lease"
    );
    gate.notify_one();
    assert_eq!(a.await.unwrap(), 1, "All Mail returned its message");
    assert_eq!(b.await.unwrap(), 1, "Trash returned its message");

    assert_uid_fetched_in(&ctx.fake, all_uid, "all");
    assert_uid_fetched_in(&ctx.fake, trash_uid, "trash");
    assert_eq!(
        ctx.fake.state.lock().unwrap().connections,
        1,
        "both batches share the single worker"
    );
}

/// Production paths: a partial sync (All Mail → Trash → Junk) running beside a
/// background body read on the same worker lane. Every FETCH for a message
/// must run in the mailbox that actually holds it, and no batch may be
/// rejected for a missing selection (the fake server is strict about that).
#[tokio::test]
async fn p43_sync_and_body_read_fetch_in_their_own_mailbox() {
    let ctx = synced().await;
    let (all_uid, mid_hex) = {
        let st = ctx.fake.state.lock().unwrap();
        let m = st
            .msgs
            .values()
            .find(|m| m.folders.contains_key("all") && !m.folders.contains_key("trash"))
            .expect("an All Mail message");
        (m.folders["all"], format!("{:x}", m.msgid))
    };
    let provider = Arc::new(GmailImapProvider::new(
        ctx.acc.id.clone(),
        ctx.pool.clone(),
        ctx.db.clone(),
    ));
    let prev = prev_cursors(&ctx).await;

    let sync = {
        let (pool, db, aid, folders) = (
            ctx.pool.clone(),
            ctx.db.clone(),
            ctx.acc.id.clone(),
            ctx.folders.clone(),
        );
        tokio::spawn(async move {
            let sink = DbSink::new(db);
            partial::run_partial_sync(
                &pool,
                &sink,
                &aid,
                &folders,
                &prev,
                false,
                &tokio_util::sync::CancellationToken::new(),
            )
            .await
            .map(|_| ())
        })
    };
    let read = {
        let provider = provider.clone();
        let mid = mid_hex.clone();
        tokio::spawn(async move { provider.fetch_body_backfill(&mid).await.map(|_| ()) })
    };
    sync.await.unwrap().expect("partial sync");
    read.await.unwrap().expect("body read");

    assert_uid_fetched_in(&ctx.fake, all_uid, "all");
}

/// Cancelling a command mid-read leaves the connection poisoned: the next task
/// reconnects instead of reading the previous response as its own answer.
#[tokio::test]
async fn p43_cancelled_command_poisons_the_connection() {
    let ctx = synced().await;
    let all_name = ctx.folders.all.clone();
    ctx.fake.state.lock().unwrap().behavior.completion_delay_ms = 400;
    let before = ctx.fake.state.lock().unwrap().connections;

    {
        let mut w = ctx.pool.worker().await.unwrap();
        let conn = w.as_mut().unwrap();
        let timed_out =
            tokio::time::timeout(Duration::from_millis(60), conn.select(&all_name, true)).await;
        assert!(timed_out.is_err(), "the delayed SELECT must not complete");
        // The lease goes back to the pool mid-response.
    }
    ctx.fake.state.lock().unwrap().behavior.completion_delay_ms = 0;

    let mut w = ctx.pool.worker().await.unwrap();
    w.as_mut()
        .unwrap()
        .noop()
        .await
        .expect("a poisoned connection is rebuilt, never reused");
    assert!(
        ctx.fake.state.lock().unwrap().connections > before,
        "the next task must open a fresh connection"
    );
}
