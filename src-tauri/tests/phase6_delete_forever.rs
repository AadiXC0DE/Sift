//! P6.4 — permanent deletion keeps the remote identities and removes exactly
//! the messages the user confirmed, over a real (fake) IMAP conversation.
#[path = "support/mod.rs"]
mod support;

use sift::db::Db;
use sift::provider::imap::conn::ImapPool;
use sift::provider::imap::ops::DeleteTarget;
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

struct Ctx {
    _fake: support::fake_imap::FakeGmail,
    /// The database directory stays alive for as long as the context: the
    /// connection pool reads from it throughout the test.
    _dir: tempfile::TempDir,
    db: Db,
    acc: sift::dto::Account,
    provider: GmailImapProvider,
}

async fn synced() -> Ctx {
    synced_with(support::fake_imap::State::default()).await
}

async fn synced_with(state: support::fake_imap::State) -> Ctx {
    let fake = support::fake_imap::FakeGmail::start_with(state).await;
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
    let provider = GmailImapProvider::new(acc.id.clone(), pool.clone(), db.clone());
    Ctx {
        _fake: fake,
        _dir: dir,
        db,
        acc,
        provider,
    }
}

fn op(payload: serde_json::Value) -> OutboxOp {
    OutboxOp {
        id: 1,
        account_id: "x".into(),
        kind: "delete".into(),
        payload,
    }
}

/// A message that is in \All and whose conversation holds nothing else, so a
/// thread-shaped payload names exactly one message.
async fn any_message(ctx: &Ctx) -> (String, u64, String) {
    let candidates: Vec<(String, u64)> = {
        let st = ctx._fake.state.lock().unwrap();
        st.msgs
            .iter()
            .filter(|(_, m)| m.folders.contains_key("all"))
            .map(|(id, _)| (format!("{id:x}"), *id))
            .collect()
    };
    for (hex, dec) in candidates {
        let thread: Option<String> = ctx
            .db
            .read({
                let hex = hex.clone();
                move |c| {
                    Ok(c.query_row(
                        "SELECT thread_id FROM messages WHERE id=?",
                        rusqlite::params![hex],
                        |r| r.get(0),
                    )
                    .ok())
                }
            })
            .await
            .unwrap();
        let Some(thread) = thread else { continue };
        let count: i64 = ctx
            .db
            .read({
                let thread = thread.clone();
                move |c| {
                    Ok(c.query_row(
                        "SELECT count(*) FROM messages WHERE thread_id=?",
                        rusqlite::params![thread],
                        |r| r.get(0),
                    )?)
                }
            })
            .await
            .unwrap();
        if count == 1 {
            return (hex, dec, thread);
        }
    }
    panic!("no single-message conversation in the fixture");
}

/// Put a message in Trash the way the app does, and return its locator hint.
///
/// The UID is read from the server: a MOVE assigns a fresh UID, and the local
/// map learns it on the next sync — which is exactly why the deletion must be
/// resolvable from the message identity rather than from a remembered UID.
async fn move_to_trash(ctx: &Ctx, hex: &str, dec: u64) -> (String, i64, i64) {
    ctx.provider
        .apply(&OutboxOp {
            id: 2,
            account_id: ctx.acc.id.clone(),
            kind: "modify_labels".into(),
            payload: serde_json::json!({"ids": [hex], "add": ["TRASH"], "remove": ["INBOX"]}),
        })
        .await
        .unwrap();
    let uid = {
        let st = ctx._fake.state.lock().unwrap();
        st.msgs
            .get(&dec)
            .and_then(|m| m.folders.get("trash").copied())
            .expect("the message is in Trash now")
    };
    let epoch = ctx
        .db
        .imap_get_folder(&ctx.acc.id, "trash")
        .await
        .unwrap()
        .expect("folder cursor")
        .uidvalidity as i64;
    ("trash".to_string(), uid as i64, epoch)
}

/// The whole point of the operation shape: what is deleted is exactly what the
/// user confirmed — not the thread, and not another client's \Deleted mail.
#[tokio::test]
async fn p6_4_delete_removes_exactly_the_named_messages() {
    let ctx = synced().await;
    let (hex, dec, _thread_id) = any_message(&ctx).await;
    let (role, uid, epoch) = move_to_trash(&ctx, &hex, dec).await;

    // Another client flagged a *different* message \Deleted but has not
    // expunged it. A broad EXPUNGE would take it too.
    let (other_hex, other_dec) = {
        let st = ctx._fake.state.lock().unwrap();
        st.msgs
            .iter()
            .find(|(id, m)| **id != dec && m.folders.contains_key("all"))
            .map(|(id, _)| (format!("{id:x}"), *id))
            .expect("a second message")
    };
    {
        let mut st = ctx._fake.state.lock().unwrap();
        if let Some(m) = st.msgs.get_mut(&other_dec) {
            m.flags.push("\\Deleted".into());
        }
    }

    // The identity list is deserialized back into `DeleteTarget`s by the
    // provider, so building it as JSON exercises the same path the queued
    // operation takes.
    let payload = serde_json::json!({
        "messages": [{
            "id": hex,
            "role": role,
            "uid": uid,
            "uidvalidity": epoch,
        }],
        "cachePaths": [],
    });
    let _targets_check: Vec<DeleteTarget> =
        serde_json::from_value(payload["messages"].clone()).unwrap();
    assert_eq!(_targets_check.len(), 1);
    let outcome = ctx.provider.apply(&op(payload)).await.unwrap();
    assert_eq!(outcome, ApplyOutcome::Done);

    {
        let st = ctx._fake.state.lock().unwrap();
        let target = st.msgs.get(&dec);
        assert!(
            target.map(|m| m.folders.is_empty()).unwrap_or(true),
            "the confirmed message is gone: {:?}",
            target.map(|m| m.folders.clone())
        );
        let other = st.msgs.get(&other_dec).expect("the other message survives");
        assert!(
            other.folders.contains_key("all"),
            "another client's \\Deleted message is untouched (never a broad EXPUNGE)"
        );
        let _ = other_hex;
    }
}

/// A message another client moved back out of Trash is not deleted.
#[tokio::test]
async fn p6_4_a_message_that_left_trash_is_left_alone() {
    let ctx = synced().await;
    let (hex, dec, _thread_id) = any_message(&ctx).await;
    let (role, uid, epoch) = move_to_trash(&ctx, &hex, dec).await;
    // Back to the inbox.
    ctx.provider
        .apply(&OutboxOp {
            id: 3,
            account_id: ctx.acc.id.clone(),
            kind: "modify_labels".into(),
            payload: serde_json::json!({"ids": [hex], "add": ["INBOX"], "remove": ["TRASH"]}),
        })
        .await
        .unwrap();

    let payload = serde_json::json!({
        "messages": [{"id": hex, "role": role, "uid": uid, "uidvalidity": epoch}],
        "cachePaths": [],
    });
    let err = ctx.provider.apply(&op(payload)).await.unwrap_err();
    assert_eq!(err.code(), "delete_target_moved");
    let st = ctx._fake.state.lock().unwrap();
    assert!(st.msgs.contains_key(&dec), "the message is still there");
}

/// A stale UID inside a matching epoch triggers rediscovery, not a false
/// "already applied" and not a wrong deletion.
#[tokio::test]
async fn p6_4_a_stale_uid_is_rediscovered() {
    let ctx = synced().await;
    let (hex, dec, _thread_id) = any_message(&ctx).await;
    let (role, _, epoch) = move_to_trash(&ctx, &hex, dec).await;
    let payload = serde_json::json!({
        "messages": [{"id": hex, "role": role, "uid": 999_999u32, "uidvalidity": epoch}],
        "cachePaths": [],
    });
    let outcome = ctx.provider.apply(&op(payload)).await.unwrap();
    assert_eq!(outcome, ApplyOutcome::Done);
    let st = ctx._fake.state.lock().unwrap();
    assert!(
        st.msgs
            .get(&dec)
            .map(|m| m.folders.is_empty())
            .unwrap_or(true),
        "the identity was rediscovered and deleted"
    );
}

/// A message that is genuinely no longer on the server is reported as already
/// applied — after an identity search, not because a UID lookup failed.
#[tokio::test]
async fn p6_4_an_absent_identity_is_already_applied() {
    let ctx = synced().await;
    // "deadbeef" is not a message the fake server knows.
    let payload = serde_json::json!({
        "messages": [{"id": "deadbeef", "role": "trash", "uid": 4, "uidvalidity": 1}],
        "cachePaths": [],
    });
    let outcome = ctx.provider.apply(&op(payload)).await.unwrap();
    assert_eq!(outcome, ApplyOutcome::AlreadyApplied);
}

/// Without UIDPLUS there is no safe targeted EXPUNGE, and Sift says so instead
/// of falling back to one that would delete unrelated mail.
#[tokio::test]
async fn p6_4_missing_uidplus_is_an_unsupported_operation() {
    let mut state = support::fake_imap::State::default();
    state.behavior.no_uidplus = true;
    let ctx = synced_with(state).await;
    let (hex, dec, _thread_id) = any_message(&ctx).await;
    let (role, uid, epoch) = move_to_trash(&ctx, &hex, dec).await;
    let payload = serde_json::json!({
        "messages": [{"id": hex, "role": role, "uid": uid, "uidvalidity": epoch}],
        "cachePaths": [],
    });
    let err = ctx.provider.apply(&op(payload)).await.unwrap_err();
    assert_eq!(err.code(), "unsupported_operation");
    let st = ctx._fake.state.lock().unwrap();
    assert!(st.msgs.contains_key(&dec), "nothing was deleted");
}

/// A payload written by the previous build (thread ids) still deletes the
/// messages it names, so upgrading cannot strand a queued deletion.
#[tokio::test]
async fn p6_4_a_legacy_thread_payload_still_deletes() {
    let ctx = synced().await;
    let (hex, dec, thread_id) = any_message(&ctx).await;
    move_to_trash(&ctx, &hex, dec).await;
    let payload = serde_json::json!({"threads": [thread_id], "cachePaths": []});
    let outcome = ctx.provider.apply(&op(payload)).await.unwrap();
    assert!(matches!(
        outcome,
        ApplyOutcome::Done | ApplyOutcome::AlreadyApplied
    ));
    let st = ctx._fake.state.lock().unwrap();
    assert!(
        st.msgs
            .get(&dec)
            .map(|m| m.folders.is_empty())
            .unwrap_or(true),
        "the message named by the legacy payload is gone"
    );
}
