//! P11-T06: incremental sync — new mail, flag/label deltas (CONDSTORE +
//! fallback), trash/untrash/delete-forever via UID diff, NeedsFull on
//! UIDVALIDITY change.
#[path = "support/mod.rs"]
mod support;

use sift::db::Db;
use sift::provider::imap::{folders, partial};
use sift::provider::{DbSink, PartialOutcome};
use std::collections::HashMap;

fn pool_for(port: u16) -> sift::provider::imap::conn::ImapPool {
    sift::provider::imap::conn::ImapPool::new(
        "user@gmail.com".into(),
        "goodpassword0000".into(),
        "127.0.0.1".into(),
        port,
        true,
    )
}

struct Ctx {
    _fake: support::fake_imap::FakeGmail,
    pool: sift::provider::imap::conn::ImapPool,
    db: Db,
    acc: sift::dto::Account,
    folders: sift::provider::imap::folders::FolderMap,
}

async fn full_synced() -> Ctx {
    let fake = support::fake_imap::FakeGmail::start().await;
    let pool = pool_for(fake.addr.port());
    let dir = tempfile::tempdir().unwrap();
    // Leak the tempdir: the pool holds no DB refs, only the Db does.
    let dir = Box::leak(Box::new(dir));
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("user@gmail.com", None, None).await.unwrap();
    let folders = {
        let mut guard = pool.worker().await.unwrap();
        folders::discover(guard.as_mut().unwrap()).await.unwrap()
    };
    let sink = DbSink::new(db.clone());
    let cancel = tokio_util::sync::CancellationToken::new();
    sift::provider::imap::full::run_full_sync(&pool, &sink, &acc.id, &folders, cancel)
        .await
        .unwrap();
    Ctx {
        _fake: fake,
        pool,
        db,
        acc,
        folders,
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

async fn tick(ctx: &Ctx, force_uid_diff: bool) -> PartialOutcome {
    let sink = DbSink::new(ctx.db.clone());
    let prev = prev_cursors(ctx).await;
    let cancel = tokio_util::sync::CancellationToken::new();
    partial::run_partial_sync(
        &ctx.pool,
        &sink,
        &ctx.acc.id,
        &ctx.folders,
        &prev,
        force_uid_diff,
        &cancel,
    )
    .await
    .unwrap()
}

type Changed = Vec<(String, String)>;
type NewMail = Vec<(String, String, String, String)>;

fn synced(out: PartialOutcome) -> (Changed, NewMail) {
    match out {
        PartialOutcome::Synced {
            changed_threads,
            new_inbox,
        } => (changed_threads, new_inbox),
        PartialOutcome::NeedsFull => panic!("unexpected NeedsFull"),
    }
}

#[tokio::test]
async fn p11_t06_new_mail_notifies() {
    let ctx = full_synced().await;
    let mid = ctx._fake.deliver(
        "all",
        "Zoe",
        "Hello from the outside",
        vec!["INBOX".into()],
        true,
    );
    let hex = format!("{mid:x}");
    let (changed, new_inbox) = synced(tick(&ctx, false).await);
    assert!(changed.iter().any(|(_, t)| *t == hex));
    let note = new_inbox
        .iter()
        .find(|(_, t, _, _)| *t == hex)
        .expect("notify:new-mail payload");
    assert_eq!(note.2, "Zoe");
    assert_eq!(note.3, "Hello from the outside");
    // quiet tick right after: nothing changes (idempotent, cheap)
    let (changed2, new2) = synced(tick(&ctx, false).await);
    assert!(changed2.is_empty());
    assert!(new2.is_empty());
}

#[tokio::test]
async fn p11_t06_flag_label_diff_applies() {
    let ctx = full_synced().await;
    // Pick a read, unstarred \All message and flip it remotely.
    let target: u64 = {
        let st = ctx._fake.state.lock().unwrap();
        *st.msgs
            .iter()
            .find(|(_, m)| {
                m.folders.contains_key("all")
                    && m.flags.contains(&"\\Seen".to_string())
                    && !m.flags.contains(&"\\Flagged".to_string())
            })
            .map(|(id, _)| id)
            .expect("candidate")
    };
    ctx._fake.set_flag(target, "\\Seen", false); // -> unread
    ctx._fake.set_flag(target, "\\Flagged", true); // -> starred
    {
        let mut st = ctx._fake.state.lock().unwrap();
        st.modseq += 1;
        let ms = st.modseq;
        if let Some(m) = st.msgs.get_mut(&target) {
            if !m.labels.contains(&"Clients/Acme".to_string()) {
                m.labels.push("Clients/Acme".to_string());
            }
            m.modseq = ms;
        }
    }
    let hex = format!("{target:x}");
    let (changed, _) = synced(tick(&ctx, false).await);
    assert!(
        !changed.is_empty(),
        "expected the flipped message to surface"
    );
    let (unread, starred): (bool, bool) = ctx
        .db
        .read({
            let hex = hex.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT is_unread, is_starred FROM messages WHERE id=?",
                    rusqlite::params![hex],
                    |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0)),
                )?)
            }
        })
        .await
        .unwrap();
    assert!(unread && starred);
    let labels: String = ctx
        .db
        .read({
            let hex = hex.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT label_ids FROM messages WHERE id=?",
                    rusqlite::params![hex],
                    |r| r.get::<_, String>(0),
                )?)
            }
        })
        .await
        .unwrap();
    assert!(labels.contains("Clients/Acme"));
}

#[tokio::test]
async fn p11_t06_trash_untrash_delete() {
    let ctx = full_synced().await;
    // Trash a message in Gmail: move all -> trash inside the fake.
    let target: u64 = {
        let st = ctx._fake.state.lock().unwrap();
        *st.msgs
            .iter()
            .find(|(_, m)| m.folders.contains_key("all") && !m.labels.contains(&"DRAFT".into()))
            .map(|(id, _)| id)
            .expect("candidate")
    };
    let hex = format!("{target:x}");
    {
        let mut st = ctx._fake.state.lock().unwrap();
        let uid = st.msgs.get_mut(&target).unwrap().folders.remove("all");
        assert!(uid.is_some());
        let trash_next = *st.next_uid.get("trash").unwrap();
        st.next_uid.insert("trash".into(), trash_next + 1);
        st.modseq += 1;
        let ms = st.modseq;
        let m = st.msgs.get_mut(&target).unwrap();
        m.folders.insert("trash".into(), trash_next);
        m.flags.push("\\Deleted".into());
        m.modseq = ms;
    }
    let (changed, _) = synced(tick(&ctx, false).await);
    assert!(changed.iter().any(|(_, t)| *t == hex));
    // The trashed message row carries TRASH now (thread id may differ
    // for multi-message threads, so assert on the message row itself).
    let labels: String = ctx
        .db
        .read({
            let hex = hex.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT label_ids FROM messages WHERE id=?",
                    rusqlite::params![hex],
                    |r| r.get::<_, String>(0),
                )?)
            }
        })
        .await
        .unwrap();
    assert!(labels.contains("TRASH"), "labels: {labels}");

    // Untrash: back to \All.
    {
        let mut st = ctx._fake.state.lock().unwrap();
        let uid = st.msgs.get_mut(&target).unwrap().folders.remove("trash");
        assert!(uid.is_some());
        let all_next = *st.next_uid.get("all").unwrap();
        st.next_uid.insert("all".into(), all_next + 1);
        st.modseq += 1;
        let ms = st.modseq;
        let m = st.msgs.get_mut(&target).unwrap();
        m.folders.insert("all".into(), all_next);
        m.flags.retain(|f| f != "\\Deleted");
        m.labels.push("INBOX".into());
        m.modseq = ms;
    }
    let (changed, _) = synced(tick(&ctx, false).await);
    assert!(changed.iter().any(|(_, _)| true));

    // Delete forever: vanish from every folder.
    {
        let mut st = ctx._fake.state.lock().unwrap();
        st.modseq += 1;
        let ms = st.modseq;
        if let Some(m) = st.msgs.get_mut(&target) {
            m.folders.clear();
            m.modseq = ms;
        }
    }
    let (changed, _) = synced(tick(&ctx, true).await);
    assert!(changed.iter().any(|(_, _)| true));
    let gone: bool = ctx
        .db
        .read({
            let hex = hex.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?)",
                    rusqlite::params![hex],
                    |r| r.get(0),
                )?)
            }
        })
        .await
        .unwrap();
    assert!(!gone, "deleted-forever message must be gone locally");
}

#[tokio::test]
async fn p11_t06_uidvalidity_needs_full() {
    let ctx = full_synced().await;
    ctx._fake.state.lock().unwrap().behavior.uidvalidity_bump = true;
    let out = tick(&ctx, false).await;
    assert!(matches!(out, PartialOutcome::NeedsFull));
}

#[tokio::test]
async fn p11_t06_no_condstore_rolling_scan() {
    let ctx = full_synced().await;
    ctx._fake.state.lock().unwrap().behavior.no_condstore = true;
    let target: u64 = {
        let st = ctx._fake.state.lock().unwrap();
        *st.msgs
            .iter()
            .find(|(_, m)| m.folders.contains_key("all") && m.flags.contains(&"\\Seen".to_string()))
            .map(|(id, _)| id)
            .expect("candidate")
    };
    ctx._fake.set_flag(target, "\\Seen", false);
    let hex = format!("{target:x}");
    let (changed, _) = synced(tick(&ctx, false).await);
    assert!(!changed.is_empty());
    // The flipped row itself must be unread now (thread ids differ for
    // multi-message threads, so assert on the message row).
    let unread: bool = ctx
        .db
        .read({
            let hex = hex.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT is_unread FROM messages WHERE id=?",
                    rusqlite::params![hex],
                    |r| r.get::<_, i64>(0),
                )?)
            }
        })
        .await
        .unwrap()
        != 0;
    assert!(unread);
}
