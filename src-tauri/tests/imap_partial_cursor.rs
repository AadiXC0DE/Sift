//! P4.5: cursor advancement, sparse UIDs and remote membership (SYNC-02).
#[path = "support/mod.rs"]
mod support;

use sift::db::Db;
use sift::provider::imap::conn::ImapPool;
use sift::provider::imap::{folders, full, partial};
use sift::provider::{DbSink, PartialOutcome, SyncSink};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

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

async fn full_synced() -> Ctx {
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

async fn tick(ctx: &Ctx, sink: &dyn SyncSink) -> PartialOutcome {
    let prev = prev_cursors(ctx).await;
    partial::run_partial_sync(
        &ctx.pool,
        sink,
        &ctx.acc.id,
        &ctx.folders,
        &prev,
        false,
        &tokio_util::sync::CancellationToken::new(),
    )
    .await
    .unwrap()
}

async fn message_count(db: &Db, account_id: &str) -> i64 {
    let a = account_id.to_string();
    db.read(move |c| {
        Ok(c.query_row(
            "SELECT count(*) FROM messages WHERE account_id=?",
            rusqlite::params![a],
            |r| r.get(0),
        )?)
    })
    .await
    .unwrap()
}

async fn labels_of(db: &Db, account_id: &str, mid: &str) -> Vec<String> {
    let (a, m) = (account_id.to_string(), mid.to_string());
    db.read(move |c| {
        Ok(c.query_row(
            "SELECT label_ids FROM messages WHERE account_id=? AND id=?",
            rusqlite::params![a, m],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_else(|_| "[]".into()))
    })
    .await
    .unwrap()
    .pipe()
}

trait Pipe {
    fn pipe(self) -> Vec<String>;
}
impl Pipe for String {
    fn pipe(self) -> Vec<String> {
        serde_json::from_str(&self).unwrap_or_default()
    }
}

/// A UIDNEXT jump of a billion with two new messages stays bounded: one
/// SEARCH and two rows, never `prev..uidnext` materialized (P4.5).
#[tokio::test]
async fn p45_uidnext_jump_of_a_billion_stays_bounded() {
    let ctx = full_synced().await;
    let before = message_count(&ctx.db, &ctx.acc.id).await;
    let jump = 1_000_000_000u32;
    let (m1, m2) = {
        let mut st = ctx.fake.state.lock().unwrap();
        st.next_uid.insert("all".into(), jump);
        drop(st);
        (
            ctx.fake
                .deliver("all", "Ana", "One", vec!["INBOX".into()], true),
            ctx.fake
                .deliver("all", "Ana", "Two", vec!["INBOX".into()], true),
        )
    };
    let fetch_before = ctx.fake.fetches().len();
    let started = std::time::Instant::now();
    let sink = DbSink::new(ctx.db.clone());
    let out = tick(&ctx, &sink).await;
    let elapsed = started.elapsed();
    match out {
        PartialOutcome::Synced { new_inbox, .. } => {
            assert_eq!(new_inbox.len(), 2, "exactly the two new messages");
        }
        PartialOutcome::NeedsFull => panic!("unexpected NeedsFull"),
    }
    assert_eq!(
        message_count(&ctx.db, &ctx.acc.id).await,
        before + 2,
        "only the two real UIDs were ingested"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "a sparse interval must not be materialized (took {elapsed:?})"
    );
    // One metadata FETCH per batch plus the snippet pass: nowhere near the gap.
    let fetches = ctx.fake.fetches().len() - fetch_before;
    assert!(
        fetches <= 8,
        "expected a bounded number of FETCHes (per folder, not per UID), saw {fetches}"
    );
    for mid in [m1, m2] {
        let hex = format!("{mid:x}");
        assert!(
            !labels_of(&ctx.db, &ctx.acc.id, &hex).await.is_empty(),
            "message {hex} must be stored"
        );
    }
    let cursor = ctx
        .db
        .imap_get_folder(&ctx.acc.id, "all")
        .await
        .unwrap()
        .unwrap();
    assert!(
        cursor.uidnext >= jump as i64 + 2,
        "the checkpoint advanced past the new messages: {}",
        cursor.uidnext
    );
}

/// A sink whose metadata batch fails on demand: the checkpoint must stay
/// BEFORE the failed batch, the row must be retried, and nothing skipped.
#[derive(Clone)]
struct FlakySink {
    inner: DbSink,
    calls: Arc<AtomicUsize>,
    fail_on_call: Arc<AtomicUsize>,
}

impl FlakySink {
    fn new(db: Db, fail_on_call: usize) -> Self {
        Self {
            inner: DbSink::new(db),
            calls: Arc::new(AtomicUsize::new(0)),
            fail_on_call: Arc::new(AtomicUsize::new(fail_on_call)),
        }
    }
    fn batches(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
    fn stop_failing(&self) {
        self.fail_on_call.store(usize::MAX, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl SyncSink for FlakySink {
    async fn upsert_labels(&self, labels: &[sift::dto::Label]) -> anyhow::Result<()> {
        self.inner.upsert_labels(labels).await
    }
    async fn insert_stub(&self, id: &str, account: &str, thread: &str) -> anyhow::Result<()> {
        self.inner.insert_stub(id, account, thread).await
    }
    async fn upsert_message(&self, m: sift::db::messages::MsgUpsert) -> anyhow::Result<()> {
        self.inner.upsert_message(m).await
    }
    async fn commit_metadata_batch(
        &self,
        batch: &sift::provider::MetadataBatch,
    ) -> anyhow::Result<()> {
        let n = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        if n == self.fail_on_call.load(Ordering::SeqCst) {
            anyhow::bail!("injected metadata batch failure");
        }
        self.inner.commit_metadata_batch(batch).await
    }
    async fn label_intents(
        &self,
        account: &str,
        message_id: &str,
    ) -> anyhow::Result<Vec<(Vec<String>, Vec<String>)>> {
        self.inner.label_intents(account, message_id).await
    }
    async fn delete_message(&self, r: &sift::dto::MessageRef, thread: &str) -> anyhow::Result<()> {
        self.inner.delete_message(r, thread).await
    }
    async fn message_labels(&self, r: &sift::dto::MessageRef) -> anyhow::Result<Vec<String>> {
        self.inner.message_labels(r).await
    }
    async fn apply_label_change(
        &self,
        r: &sift::dto::MessageRef,
        add: &[String],
        remove: &[String],
    ) -> anyhow::Result<(String, String)> {
        self.inner.apply_label_change(r, add, remove).await
    }
    async fn message_thread(&self, r: &sift::dto::MessageRef) -> anyhow::Result<Option<String>> {
        self.inner.message_thread(r).await
    }
    async fn message_snippet(&self, r: &sift::dto::MessageRef) -> anyhow::Result<String> {
        self.inner.message_snippet(r).await
    }
    async fn message_flags(&self, r: &sift::dto::MessageRef) -> anyhow::Result<(bool, bool)> {
        self.inner.message_flags(r).await
    }
    async fn set_snippet(&self, r: &sift::dto::MessageRef, snippet: &str) -> anyhow::Result<()> {
        self.inner.set_snippet(r, snippet).await
    }
    async fn message_exists(&self, r: &sift::dto::MessageRef) -> anyhow::Result<bool> {
        self.inner.message_exists(r).await
    }
    async fn list_local_messages(&self, account: &str) -> anyhow::Result<Vec<(String, String)>> {
        self.inner.list_local_messages(account).await
    }
    async fn next_bodies_to_fetch(
        &self,
        account: &str,
        limit: i64,
        min_date: i64,
    ) -> anyhow::Result<Vec<sift::dto::MessageRef>> {
        self.inner
            .next_bodies_to_fetch(account, limit, min_date)
            .await
    }
    async fn store_body(&self, b: sift::db::bodies::BodyPut) -> anyhow::Result<()> {
        self.inner.store_body(b).await
    }
    async fn store_attachment(&self, a: sift::db::attachments::AttPut) -> anyhow::Result<()> {
        self.inner.store_attachment(a).await
    }
    async fn set_history_id(&self, account: &str, hid: &str) -> anyhow::Result<()> {
        self.inner.set_history_id(account, hid).await
    }
    async fn set_sync_state(&self, account: &str, state: &str) -> anyhow::Result<()> {
        self.inner.set_sync_state(account, state).await
    }
    async fn log_sync(&self, account: &str, kind: &str, detail: &str) -> anyhow::Result<()> {
        self.inner.log_sync(account, kind, detail).await
    }
    async fn imap_set_folder(
        &self,
        account: &str,
        cur: &sift::db::imap::FolderCursor,
    ) -> anyhow::Result<()> {
        self.inner.imap_set_folder(account, cur).await
    }
    async fn imap_get_folder(
        &self,
        account: &str,
        role: &str,
    ) -> anyhow::Result<Option<sift::db::imap::FolderCursor>> {
        self.inner.imap_get_folder(account, role).await
    }
    async fn imap_clear_folder(&self, account: &str, role: &str) -> anyhow::Result<()> {
        self.inner.imap_clear_folder(account, role).await
    }
    async fn imap_put_uids(
        &self,
        account: &str,
        role: &str,
        pairs: &[(i64, String)],
    ) -> anyhow::Result<()> {
        self.inner.imap_put_uids(account, role, pairs).await
    }
    async fn imap_delete_uids(
        &self,
        account: &str,
        role: &str,
        uids: &[i64],
    ) -> anyhow::Result<()> {
        self.inner.imap_delete_uids(account, role, uids).await
    }
    async fn imap_uid_map(&self, account: &str, role: &str) -> anyhow::Result<Vec<(i64, String)>> {
        self.inner.imap_uid_map(account, role).await
    }
    async fn uids_for_message(
        &self,
        account: &str,
        message_id: &str,
    ) -> anyhow::Result<Vec<(String, i64)>> {
        self.inner.uids_for_message(account, message_id).await
    }
    fn progress(&self, status: sift::dto::SyncStatus) {
        self.inner.progress(status);
    }
    fn threads_changed(&self, account: &str, thread_ids: &[String]) {
        self.inner.threads_changed(account, thread_ids);
    }
}

/// SYNC-02: a failed upsert batch leaves the checkpoint before it, records a
/// recoverable sync error, and the next tick delivers every message.
#[tokio::test]
async fn p45_failed_batch_is_retried_and_never_skipped() {
    let ctx = full_synced().await;
    let before = message_count(&ctx.db, &ctx.acc.id).await;
    // Twenty new messages in one batch: the batch carrying the 17th fails.
    for i in 0..20 {
        ctx.fake.deliver(
            "all",
            "Sender",
            &format!("Batch message {i}"),
            vec!["INBOX".into()],
            true,
        );
    }
    let cursor_before = ctx
        .db
        .imap_get_folder(&ctx.acc.id, "all")
        .await
        .unwrap()
        .unwrap();
    let stored_before = ctx.db.imap_uid_map(&ctx.acc.id, "all").await.unwrap().len();
    let flaky = FlakySink::new(ctx.db.clone(), 1);
    let out = tick(&ctx, &flaky).await;
    assert!(matches!(out, PartialOutcome::Synced { .. }));
    assert_eq!(flaky.batches(), 1, "the batch was attempted once");
    assert_eq!(
        message_count(&ctx.db, &ctx.acc.id).await,
        before,
        "a failed batch commits nothing"
    );
    let cursor = ctx
        .db
        .imap_get_folder(&ctx.acc.id, "all")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        stored_before,
        ctx.db.imap_uid_map(&ctx.acc.id, "all").await.unwrap().len(),
        "no UID may be recorded for a batch that failed"
    );
    assert_eq!(
        cursor.uidnext, cursor_before.uidnext,
        "the checkpoint must not advance past the failed batch"
    );
    // The recoverable error is recorded, not swallowed.
    let logged: i64 = ctx
        .db
        .read({
            let a = ctx.acc.id.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM sync_log WHERE account_id=? AND kind='partial-error'",
                    rusqlite::params![a],
                    |r| r.get(0),
                )?)
            }
        })
        .await
        .unwrap();
    assert!(logged >= 1, "the failed batch must be recorded");

    // Restart-equivalent: a fresh tick with a healthy sink delivers everything.
    flaky.stop_failing();
    let sink = DbSink::new(ctx.db.clone());
    let out = tick(&ctx, &sink).await;
    assert!(matches!(out, PartialOutcome::Synced { .. }));
    assert_eq!(
        message_count(&ctx.db, &ctx.acc.id).await,
        before + 20,
        "message 17 (and its batch-mates) must arrive after the retry"
    );
}

/// The batch commit is all-or-nothing at the database level: a row that cannot
/// be written rolls the whole batch back, including the checkpoint.
#[tokio::test]
async fn p45_batch_commit_is_atomic_with_its_checkpoint() {
    let ctx = full_synced().await;
    let account = ctx.acc.id.clone();
    let before = message_count(&ctx.db, &account).await;
    let sink = DbSink::new(ctx.db.clone());
    let cursor_before = ctx
        .db
        .imap_get_folder(&account, "all")
        .await
        .unwrap()
        .unwrap();
    // An attachment row for a message that does not exist violates the
    // account-qualified foreign key: the message row and the checkpoint in the
    // same batch must not survive.
    let batch = sift::provider::MetadataBatch {
        account_id: account.clone(),
        cursor: sift::db::imap::FolderCursor {
            uidnext: cursor_before.uidnext + 7,
            ..cursor_before.clone()
        },
        messages: vec![sift::db::messages::MsgUpsert {
            id: "deadbeef".into(),
            account_id: account.clone(),
            thread_id: "deadbeef".into(),
            subject: "atomic".into(),
            ..Default::default()
        }],
        attachments: vec![sift::db::attachments::AttPut {
            id: "att-x".into(),
            account_id: account.clone(),
            message_id: "no-such-message".into(),
            gmail_att_id: None,
            part_id: "2".into(),
            filename: Some("x.pdf".into()),
            mime: "application/pdf".into(),
            size: 3,
            content_id: None,
            is_inline: false,
            data: None,
        }],
        uid_pairs: vec![(999_999, "deadbeef".into())],
    };
    assert!(
        sink.commit_metadata_batch(&batch).await.is_err(),
        "the batch must fail as a unit"
    );
    let after = ctx
        .db
        .imap_get_folder(&account, "all")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.uidnext, cursor_before.uidnext, "checkpoint unchanged");
    assert_eq!(
        message_count(&ctx.db, &account).await,
        before,
        "the message row is not committed on its own"
    );
}

/// Restoring a trashed message into Archive through another client must land in
/// Archive: INBOX is never inferred from All Mail membership (P4.5).
#[tokio::test]
async fn p45_untrash_into_archive_does_not_infer_inbox() {
    let ctx = full_synced().await;
    let (target, hex) = {
        let st = ctx.fake.state.lock().unwrap();
        // A single-message thread: the thread aggregate then describes exactly
        // this message, so `in_inbox` is unambiguous.
        let m = st
            .msgs
            .values()
            .find(|m| {
                m.thrid == m.msgid
                    && m.folders.contains_key("all")
                    && m.labels.contains(&"INBOX".to_string())
            })
            .expect("a single-message inbox thread in All Mail");
        (m.msgid, format!("{:x}", m.msgid))
    };
    assert!(labels_of(&ctx.db, &ctx.acc.id, &hex)
        .await
        .contains(&"INBOX".to_string()));

    // Another client trashes it.
    {
        let mut st = ctx.fake.state.lock().unwrap();
        let uid = st
            .msgs
            .get_mut(&target)
            .unwrap()
            .folders
            .remove("all")
            .unwrap();
        let next = *st.next_uid.get("trash").unwrap();
        st.next_uid.insert("trash".into(), next + 1);
        st.modseq += 1;
        let ms = st.modseq;
        let m = st.msgs.get_mut(&target).unwrap();
        m.folders.insert("trash".into(), next);
        m.labels.retain(|l| l != "INBOX");
        m.labels.push("TRASH".into());
        m.modseq = ms;
        let _ = uid;
    }
    let sink = DbSink::new(ctx.db.clone());
    assert!(matches!(
        tick(&ctx, &sink).await,
        PartialOutcome::Synced { .. }
    ));
    let trashed = labels_of(&ctx.db, &ctx.acc.id, &hex).await;
    assert!(trashed.contains(&"TRASH".to_string()), "{trashed:?}");

    // ... then restores it into Archive (no INBOX label on the server).
    {
        let mut st = ctx.fake.state.lock().unwrap();
        st.msgs.get_mut(&target).unwrap().folders.remove("trash");
        let next = *st.next_uid.get("all").unwrap();
        st.next_uid.insert("all".into(), next + 1);
        st.modseq += 1;
        let ms = st.modseq;
        let m = st.msgs.get_mut(&target).unwrap();
        m.folders.insert("all".into(), next);
        m.labels.retain(|l| l != "TRASH" && l != "INBOX");
        m.modseq = ms;
    }
    assert!(matches!(
        tick(&ctx, &sink).await,
        PartialOutcome::Synced { .. }
    ));
    let restored = labels_of(&ctx.db, &ctx.acc.id, &hex).await;
    assert!(!restored.contains(&"TRASH".to_string()), "{restored:?}");
    assert!(
        !restored.contains(&"INBOX".to_string()),
        "All Mail membership must not imply INBOX: {restored:?}"
    );
    let in_inbox: bool = ctx
        .db
        .read({
            let a = ctx.acc.id.clone();
            let h = hex.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT in_inbox FROM threads WHERE account_id=? AND id=(SELECT thread_id FROM messages WHERE account_id=? AND id=?)",
                    rusqlite::params![a, a, h],
                    |r| r.get::<_, i64>(0),
                )?)
            }
        })
        .await
        .unwrap()
        != 0;
    let (members, inbox_rows): (i64, i64) = ctx
        .db
        .read({
            let a = ctx.acc.id.clone();
            let h = hex.clone();
            move |c| {
                let members: i64 = c.query_row(
                    "SELECT count(*) FROM messages WHERE account_id=?1 AND thread_id=(SELECT thread_id FROM messages WHERE account_id=?1 AND id=?2)",
                    rusqlite::params![a, h],
                    |r| r.get(0),
                )?;
                let inbox: i64 = c.query_row(
                    "SELECT count(*) FROM message_labels WHERE account_id=? AND message_id=? AND label_id='INBOX'",
                    rusqlite::params![a, h],
                    |r| r.get(0),
                )?;
                Ok((members, inbox))
            }
        })
        .await
        .unwrap();
    assert_eq!(inbox_rows, 0, "the message itself is not in Inbox");
    if members == 1 {
        // The thread aggregate then describes exactly this message.
        assert!(
            !in_inbox,
            "a restored-to-Archive message is not in Inbox (labels={restored:?})"
        );
    }
}

/// An old server snapshot must not undo a local change that is still queued
/// (P4.5): the pending label intent is re-applied over the server state.
#[tokio::test]
async fn p45_pending_label_intent_survives_an_old_snapshot() {
    let ctx = full_synced().await;
    let (target, hex) = {
        let st = ctx.fake.state.lock().unwrap();
        let m = st
            .msgs
            .values()
            .find(|m| m.folders.contains_key("all") && m.labels.contains(&"INBOX".to_string()))
            .expect("an inbox message");
        (m.msgid, format!("{:x}", m.msgid))
    };
    let account = ctx.acc.id.clone();
    // The user archives offline: the local row changes and the op is queued.
    ctx.db
        .apply_label_change(
            &sift::dto::MessageRef::new(&account, &hex),
            &[],
            &["INBOX".into()],
        )
        .await
        .unwrap();
    ctx.db
        .outbox_enqueue(
            &account,
            "modify_labels",
            &serde_json::json!({"ids": [hex], "add": [], "remove": ["INBOX"]}).to_string(),
            None,
            0,
        )
        .await
        .unwrap();
    assert!(!labels_of(&ctx.db, &account, &hex)
        .await
        .contains(&"INBOX".to_string()));

    // The server still reports INBOX (an old snapshot) and a delta arrives.
    let bump = |st: &mut support::fake_imap::State| {
        st.modseq += 1;
        let ms = st.modseq;
        if let Some(m) = st.msgs.get_mut(&target) {
            m.modseq = ms;
            if !m.labels.contains(&"INBOX".to_string()) {
                m.labels.push("INBOX".into());
            }
        }
    };
    {
        let mut st = ctx.fake.state.lock().unwrap();
        bump(&mut st);
    }
    let sink = DbSink::new(ctx.db.clone());
    assert!(matches!(
        tick(&ctx, &sink).await,
        PartialOutcome::Synced { .. }
    ));
    assert!(
        !labels_of(&ctx.db, &account, &hex)
            .await
            .contains(&"INBOX".to_string()),
        "the queued archive intent must win over the stale snapshot"
    );

    // Once the op is acknowledged, the server state is authoritative again.
    let op = ctx
        .db
        .outbox_next(&account)
        .await
        .unwrap()
        .expect("the archive op is still queued");
    ctx.db.outbox_set(op.id, "done", 1, 0, None).await.unwrap();
    {
        let mut st = ctx.fake.state.lock().unwrap();
        bump(&mut st);
    }
    assert!(matches!(
        tick(&ctx, &sink).await,
        PartialOutcome::Synced { .. }
    ));
    assert!(
        labels_of(&ctx.db, &account, &hex)
            .await
            .contains(&"INBOX".to_string()),
        "with no queued intent the server state applies unchanged"
    );
}
