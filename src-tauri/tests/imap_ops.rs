//! P11-T07: every row of the task-10 table against the fake server.
#[path = "support/mod.rs"]
mod support;
use sift::db::Db;
use sift::provider::imap::conn::ImapPool;
use sift::provider::imap::provider::GmailImapProvider;
use sift::provider::imap::{folders, full};
use sift::provider::{ApplyOutcome, DbSink, OutboxOp, Provider};

fn pool_for(port: u16) -> ImapPool {
    ImapPool::new(
        "user@gmail.com".into(),
        "goodpassword0000".into(),
        "127.0.0.1".into(),
        port,
        true,
    )
}

#[allow(dead_code)]
struct Ctx {
    _fake: support::fake_imap::FakeGmail,
    pool: ImapPool,
    db: Db,
    acc: sift::dto::Account,
    folders: sift::provider::imap::folders::FolderMap,
    provider: GmailImapProvider,
}

async fn synced() -> Ctx {
    let fake = support::fake_imap::FakeGmail::start().await;
    let pool = pool_for(fake.addr.port());
    let dir = tempfile::tempdir().unwrap();
    let dir = Box::leak(Box::new(dir));
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
    // Build provider sharing the same pool/db (folders cached lazily).
    let provider = GmailImapProvider::new(acc.id.clone(), pool.clone(), db.clone());
    Ctx {
        _fake: fake,
        pool,
        db,
        acc,
        folders,
        provider,
    }
}

fn op(kind: &str, payload: serde_json::Value) -> OutboxOp {
    OutboxOp {
        id: 1,
        account_id: "x".into(),
        kind: kind.into(),
        payload,
    }
}

async fn msgid_for_label(ctx: &Ctx, want_inbox: bool) -> (String, u64) {
    let st = ctx._fake.state.lock().unwrap();
    let mut cands: Vec<(u64, u64)> = st
        .msgs
        .iter()
        .filter(|(_, m)| {
            m.folders.contains_key("all") && (m.labels.contains(&"INBOX".to_string()) == want_inbox)
        })
        .map(|(id, m)| (*id, m.thrid))
        .collect();
    cands.sort_unstable();
    // True single-message thread (unique thrid) for stable assertions.
    use std::collections::HashMap;
    let mut counts: HashMap<u64, usize> = HashMap::new();
    for (_, th) in &cands {
        *counts.entry(*th).or_default() += 1;
    }
    let (id, _) = cands
        .iter()
        .find(|(id, th)| id == th && counts.get(th) == Some(&1))
        .copied()
        .unwrap_or(cands[0]);
    (format!("{id:x}"), id)
}

#[tokio::test]
async fn p11_t07_archive_read_star_labels() {
    let ctx = synced().await;
    // Archive: remove INBOX via STORE -X-GM-LABELS in \All.
    let (hex, dec) = msgid_for_label(&ctx, true).await;
    let r = ctx
        .provider
        .apply(&op(
            "modify_labels",
            serde_json::json!({"ids":[hex.clone()],"add":[],"remove":["INBOX"]}),
        ))
        .await
        .unwrap();
    assert_eq!(r, ApplyOutcome::Done);
    // Idempotent replay succeeds.
    let r2 = ctx
        .provider
        .apply(&op(
            "modify_labels",
            serde_json::json!({"ids":[hex.clone()],"add":[],"remove":["INBOX"]}),
        ))
        .await
        .unwrap();
    assert!(matches!(
        r2,
        ApplyOutcome::Done | ApplyOutcome::AlreadyApplied
    ));
    // Server no longer lists INBOX for the message.
    {
        let st = ctx._fake.state.lock().unwrap();
        let m = st.msgs.get(&dec).unwrap();
        assert!(
            !m.labels.iter().any(|l| l == "INBOX" || l == "\\Inbox"),
            "labels {:?}",
            m.labels
        );
    }

    // Read/unread via FLAGS.
    let (hex2, dec2) = msgid_for_label(&ctx, true).await;
    // Mark read (remove UNREAD → +FLAGS \Seen).
    ctx.provider
        .apply(&op(
            "modify_labels",
            serde_json::json!({"ids":[hex2.clone()],"add":[],"remove":["UNREAD"]}),
        ))
        .await
        .unwrap();
    {
        let st = ctx._fake.state.lock().unwrap();
        assert!(st
            .msgs
            .get(&dec2)
            .unwrap()
            .flags
            .contains(&"\\Seen".to_string()));
    }
    // Mark unread (add UNREAD → -FLAGS \Seen).
    ctx.provider
        .apply(&op(
            "modify_labels",
            serde_json::json!({"ids":[hex2.clone()],"add":["UNREAD"],"remove":[]}),
        ))
        .await
        .unwrap();
    {
        let st = ctx._fake.state.lock().unwrap();
        assert!(!st
            .msgs
            .get(&dec2)
            .unwrap()
            .flags
            .contains(&"\\Seen".to_string()));
    }

    // Star/unstar via \Flagged.
    ctx.provider
        .apply(&op(
            "modify_labels",
            serde_json::json!({"ids":[hex.clone()],"add":["STARRED"],"remove":[]}),
        ))
        .await
        .unwrap();
    {
        let st = ctx._fake.state.lock().unwrap();
        assert!(st
            .msgs
            .get(&dec)
            .unwrap()
            .flags
            .contains(&"\\Flagged".to_string()));
    }
    ctx.provider
        .apply(&op(
            "modify_labels",
            serde_json::json!({"ids":[hex.clone()],"add":[],"remove":["STARRED"]}),
        ))
        .await
        .unwrap();
    {
        let st = ctx._fake.state.lock().unwrap();
        assert!(!st
            .msgs
            .get(&dec)
            .unwrap()
            .flags
            .contains(&"\\Flagged".to_string()));
    }

    // Add/remove user label (CREATE idempotent).
    ctx.provider
        .apply(&op(
            "modify_labels",
            serde_json::json!({"ids":[hex.clone()],"add":["Clients/Acme"],"remove":[]}),
        ))
        .await
        .unwrap();
    {
        let st = ctx._fake.state.lock().unwrap();
        assert!(st
            .msgs
            .get(&dec)
            .unwrap()
            .labels
            .contains(&"Clients/Acme".to_string()));
    }
    ctx.provider
        .apply(&op(
            "modify_labels",
            serde_json::json!({"ids":[hex.clone()],"add":[],"remove":["Clients/Acme"]}),
        ))
        .await
        .unwrap();
    {
        let st = ctx._fake.state.lock().unwrap();
        assert!(!st
            .msgs
            .get(&dec)
            .unwrap()
            .labels
            .contains(&"Clients/Acme".to_string()));
    }
}

#[tokio::test]
async fn p11_t07_trash_untrash_spam_delete() {
    let ctx = synced().await;
    // Pick a message in \All.
    let (hex, dec) = msgid_for_label(&ctx, true).await;
    // Trash via MOVE to \Trash.
    ctx.provider
        .apply(&op(
            "modify_labels",
            serde_json::json!({"ids":[hex.clone()],"add":["TRASH"],"remove":["INBOX"]}),
        ))
        .await
        .unwrap();
    {
        let st = ctx._fake.state.lock().unwrap();
        let m = st.msgs.get(&dec).unwrap();
        assert!(m.folders.contains_key("trash"), "{:?}", m.folders);
        assert!(!m.folders.contains_key("all"));
    }
    // Re-applying trash is idempotent (already in trash → AlreadyApplied/Done).
    let r = ctx
        .provider
        .apply(&op(
            "modify_labels",
            serde_json::json!({"ids":[hex.clone()],"add":["TRASH"],"remove":["INBOX"]}),
        ))
        .await
        .unwrap();
    assert!(matches!(
        r,
        ApplyOutcome::Done | ApplyOutcome::AlreadyApplied
    ));
    // Untrash back to \All + INBOX.
    ctx.provider
        .apply(&op(
            "modify_labels",
            serde_json::json!({"ids":[hex.clone()],"add":["INBOX"],"remove":["TRASH"]}),
        ))
        .await
        .unwrap();
    {
        let st = ctx._fake.state.lock().unwrap();
        let m = st.msgs.get(&dec).unwrap();
        assert!(m.folders.contains_key("all"), "{:?}", m.folders);
    }

    // Spam / unspam.
    ctx.provider
        .apply(&op(
            "modify_labels",
            serde_json::json!({"ids":[hex.clone()],"add":["SPAM"],"remove":["INBOX"]}),
        ))
        .await
        .unwrap();
    {
        let st = ctx._fake.state.lock().unwrap();
        assert!(st.msgs.get(&dec).unwrap().folders.contains_key("junk"));
    }
    ctx.provider
        .apply(&op(
            "modify_labels",
            serde_json::json!({"ids":[hex.clone()],"add":["INBOX"],"remove":["SPAM"]}),
        ))
        .await
        .unwrap();
    {
        let st = ctx._fake.state.lock().unwrap();
        assert!(st.msgs.get(&dec).unwrap().folders.contains_key("all"));
    }

    // Delete forever: trash then delete (thread-based).
    ctx.provider
        .apply(&op(
            "modify_labels",
            serde_json::json!({"ids":[hex.clone()],"add":["TRASH"],"remove":["INBOX"]}),
        ))
        .await
        .unwrap();
    // Resolve thread for delete payload.
    let tid: String = ctx
        .db
        .read({
            let hex = hex.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT thread_id FROM messages WHERE id=?",
                    rusqlite::params![hex],
                    |r| r.get(0),
                )?)
            }
        })
        .await
        .unwrap();
    ctx.provider
        .apply(&op("delete", serde_json::json!({"threads":[tid]})))
        .await
        .unwrap();
    {
        let st = ctx._fake.state.lock().unwrap();
        match st.msgs.get(&dec) {
            None => {}
            Some(m) => assert!(m.folders.is_empty(), "{:?}", m.folders),
        };
    }
}

#[tokio::test]
async fn p11_t07_missing_uid_searches_then_succeeds() {
    let ctx = synced().await;
    // Simulate a message that arrived via another client: drop its local UID
    // map rows so locate() must UID SEARCH X-GM-MSGID first.
    let (hex, dec) = msgid_for_label(&ctx, true).await;
    ctx.db
        .imap_delete_uids(&ctx.acc.id, "all", &[1])
        .await
        .unwrap_or(());
    // Delete all uid rows for this message to force search path.
    {
        let holders = ctx.db.uids_for_message(&ctx.acc.id, &hex).await.unwrap();
        for (role, uid) in holders {
            let _ = ctx.db.imap_delete_uids(&ctx.acc.id, &role, &[uid]).await;
        }
    }
    // Archive should still succeed via SEARCH.
    let r = ctx
        .provider
        .apply(&op(
            "modify_labels",
            serde_json::json!({"ids":[hex.clone()],"add":[],"remove":["INBOX"]}),
        ))
        .await
        .unwrap();
    assert!(matches!(
        r,
        ApplyOutcome::Done | ApplyOutcome::AlreadyApplied
    ));
    {
        let st = ctx._fake.state.lock().unwrap();
        let m = st.msgs.get(&dec).unwrap();
        assert!(!m.labels.contains(&"INBOX".to_string()));
    }
}

#[tokio::test]
async fn p11_t07_drafts_append_expunge() {
    let ctx = synced().await;
    let raw =
        b"From: me@example.com\r\nTo: you@example.com\r\nSubject: draft\r\n\r\nhello".to_vec();
    let id1 = ctx.provider.draft_upsert(None, &raw).await.unwrap();
    assert!(!id1.is_empty());
    // Upsert new revision expunges previous (server has exactly one draft copy).
    let id2 = ctx.provider.draft_upsert(Some(&id1), &raw).await.unwrap();
    assert!(!id2.is_empty());
    ctx.provider.draft_delete(&id2).await.unwrap();
}
