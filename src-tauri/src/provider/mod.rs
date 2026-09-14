//! Transport abstraction (Phase 11 task 1).
//!
//! Every mailbox operation the app performs goes through [`Provider`]; the
//! sync engine, outbox drain, backfill, URI scheme, and commands never touch a
//! transport client directly. Providers are constructed **per account** (they
//! carry their own credentials), so methods take no account id.
//!
//! * [`gmail::GmailApiProvider`] - Gmail REST API (OAuth). Delegates to the
//!   long-tested REST code; behavior for OAuth accounts is unchanged.
//! * `imap::GmailImapProvider` - Gmail IMAP/SMTP (app password). Added in the
//!   IMAP step; same rows, same UI, same speed.
use crate::db::{
    attachments::AttPut, bodies::BodyPut, imap::FolderCursor, messages::MsgUpsert, Db,
};
use crate::dto::{Label, MessageRef, SyncStatus};
use crate::errors::SiftError;
use anyhow::Result;
use async_trait::async_trait;
use std::pin::Pin;

pub mod gmail;
pub mod imap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    GmailApi,
    GmailImap,
}

#[derive(Debug, Clone)]
pub struct ProfileInfo {
    pub email: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
}

/// Sync cursor. Persisted in `accounts.history_id`: REST keeps the legacy
/// plain history-id string; IMAP stores `{"imap":[...]}` JSON. [`Cursor::parse`]
/// reads both forms (and anything unparseable as an empty REST cursor, which
/// safely triggers reconcile).
#[derive(Debug, Clone, PartialEq)]
pub enum Cursor {
    Gmail { history_id: String },
    Imap { folders: Vec<FolderCursor> },
}

impl Cursor {
    pub fn parse(raw: &str) -> Self {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(raw) {
            if let Some(arr) = v.get("imap").and_then(|a| a.as_array()) {
                let folders: Vec<FolderCursor> = arr
                    .iter()
                    .filter_map(|f| {
                        Some(FolderCursor {
                            role: f.get("role")?.as_str()?.into(),
                            name: f.get("name")?.as_str()?.into(),
                            uidvalidity: f.get("uidvalidity")?.as_i64()?,
                            uidnext: f.get("uidnext")?.as_i64()?,
                            highestmodseq: f.get("highestmodseq").and_then(|m| m.as_i64()),
                            exists_count: f.get("exists_count")?.as_i64()?,
                            last_full_scan: f.get("last_full_scan").and_then(|m| m.as_i64()),
                        })
                    })
                    .collect();
                return Cursor::Imap { folders };
            }
        }
        Cursor::Gmail {
            history_id: raw.to_string(),
        }
    }

    pub fn render(&self) -> String {
        match self {
            Cursor::Gmail { history_id } => history_id.clone(),
            Cursor::Imap { folders } => {
                let arr: Vec<serde_json::Value> = folders
                    .iter()
                    .map(|f| {
                        serde_json::json!({
                            "role": f.role, "name": f.name,
                            "uidvalidity": f.uidvalidity, "uidnext": f.uidnext,
                            "highestmodseq": f.highestmodseq,
                            "exists_count": f.exists_count,
                            "last_full_scan": f.last_full_scan,
                        })
                    })
                    .collect();
                serde_json::json!({ "imap": arr }).to_string()
            }
        }
    }
}

/// One message that arrived in the Inbox during a partial sync (P8.4).
///
/// It carries the message id, not just the thread: "one message produces one
/// notification" is only verifiable if Sift can name the message it reported,
/// and the durable delivery record is keyed by it.
#[derive(Debug, Clone, PartialEq)]
pub struct NewMail {
    pub account_id: String,
    pub thread_id: String,
    pub message_id: String,
    pub from: String,
    pub subject: String,
}

#[derive(Debug, Clone)]
pub enum PartialOutcome {
    Synced {
        changed_threads: Vec<(String, String)>,
        new_inbox: Vec<NewMail>,
    },
    NeedsFull,
}

/// One queued mutation, as the outbox stores it.
#[derive(Debug, Clone)]
pub struct OutboxOp {
    pub id: i64,
    pub account_id: String,
    pub kind: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyOutcome {
    Done,
    AlreadyApplied,
}

#[derive(Debug, Clone)]
pub struct SentInfo {
    pub id: String,
    pub thread_id: String,
}

#[derive(Debug, Clone)]
pub struct ThreadRef {
    pub thread_id: String,
    pub message_id: String,
}

#[derive(Debug, Clone)]
pub struct SendAs {
    pub email: String,
    pub display_name: Option<String>,
    pub is_primary: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchEvent {
    Changed,
}

pub type BoxStream<'a, T> = Pin<Box<dyn futures::Stream<Item = T> + Send + 'a>>;

/// One completed partial-sync metadata batch (P4.5).
///
/// The message rows, their attachment metadata, the folder's UID map and the
/// folder checkpoint commit in ONE transaction. A failure therefore writes
/// none of them: the checkpoint stays *before* the batch, the failed row is
/// retried on the next tick, and no message can be skipped by a cursor that
/// advanced past a write that never landed.
#[derive(Debug, Clone)]
pub struct MetadataBatch {
    pub account_id: String,
    /// Checkpoint after this batch: the first UID not yet committed.
    pub cursor: FolderCursor,
    pub messages: Vec<MsgUpsert>,
    pub attachments: Vec<AttPut>,
    /// `(uid, message hex)` rows for this folder's UID map.
    pub uid_pairs: Vec<(i64, String)>,
}

/// Everything a sync or mutation path needs from the local store. The only
/// implementation is [`DbSink`] over [`Db`]; providers never touch SQLite
/// directly, which keeps both transports testable against temp DBs.
#[async_trait]
pub trait SyncSink: Send + Sync {
    // -- labels / messages -------------------------------------------------
    async fn upsert_labels(&self, labels: &[Label]) -> Result<()>;
    async fn insert_stub(&self, id: &str, account: &str, thread: &str) -> Result<()>;
    async fn upsert_message(&self, m: MsgUpsert) -> Result<()>;
    /// Commit one metadata batch together with its folder checkpoint (P4.5).
    ///
    /// `Ok` means every row and the checkpoint are durable together; `Err`
    /// means the batch wrote nothing, so the caller must keep the previous
    /// checkpoint and record a recoverable sync error.
    async fn commit_metadata_batch(&self, batch: &MetadataBatch) -> Result<()>;
    /// Pending local label intents for a message, oldest first (P4.5): the
    /// `(add, remove)` pairs the outbox still owes the server for it. Sync
    /// re-applies them over the server snapshot, in queue order, until the op
    /// is acknowledged; an old server snapshot can therefore never undo a
    /// local change that is still queued.
    async fn label_intents(
        &self,
        account: &str,
        message_id: &str,
    ) -> Result<Vec<(Vec<String>, Vec<String>)>>;
    async fn delete_message(&self, r: &MessageRef, thread: &str) -> Result<()>;
    async fn message_labels(&self, r: &MessageRef) -> Result<Vec<String>>;
    async fn apply_label_change(
        &self,
        r: &MessageRef,
        add: &[String],
        remove: &[String],
    ) -> Result<(String, String)>;
    /// Thread id of the addressed message, if it exists in that account.
    async fn message_thread(&self, r: &MessageRef) -> Result<Option<String>>;
    async fn message_snippet(&self, r: &MessageRef) -> Result<String>;
    async fn message_flags(&self, r: &MessageRef) -> Result<(bool, bool)>;
    async fn set_snippet(&self, r: &MessageRef, snippet: &str) -> Result<()>;
    async fn message_exists(&self, r: &MessageRef) -> Result<bool>;
    async fn list_local_messages(&self, account: &str) -> Result<Vec<(String, String)>>;
    // -- bodies / attachments -----------------------------------------------
    async fn next_bodies_to_fetch(
        &self,
        account: &str,
        limit: i64,
        min_date: i64,
    ) -> Result<Vec<MessageRef>>;
    async fn store_body(&self, b: BodyPut) -> Result<()>;
    async fn store_attachment(&self, a: AttPut) -> Result<()>;
    // -- account state --------------------------------------------------------
    async fn set_history_id(&self, account: &str, hid: &str) -> Result<()>;
    async fn set_sync_state(&self, account: &str, state: &str) -> Result<()>;
    async fn log_sync(&self, account: &str, kind: &str, detail: &str) -> Result<()>;
    // -- IMAP cursors / uid map --------------------------------------------------
    async fn imap_set_folder(&self, account: &str, cur: &FolderCursor) -> Result<()>;
    async fn imap_get_folder(&self, account: &str, role: &str) -> Result<Option<FolderCursor>>;
    async fn imap_clear_folder(&self, account: &str, role: &str) -> Result<()>;
    async fn imap_put_uids(&self, account: &str, role: &str, pairs: &[(i64, String)])
        -> Result<()>;
    async fn imap_delete_uids(&self, account: &str, role: &str, uids: &[i64]) -> Result<()>;
    async fn imap_uid_map(&self, account: &str, role: &str) -> Result<Vec<(i64, String)>>;
    async fn uids_for_message(&self, account: &str, message_id: &str)
        -> Result<Vec<(String, i64)>>;
    // -- progress ---------------------------------------------------------------
    fn progress(&self, status: SyncStatus);
    /// Thread rows changed (list refresh). Emitted per sync chunk so the
    /// inbox fills top-down during full sync.
    fn threads_changed(&self, account: &str, thread_ids: &[String]);
}

/// Sink notifications. `DbSink::with_progress` subscribes to progress only;
/// `with_events` gets both (used by full-sync commands).
#[derive(Debug, Clone)]
pub enum SyncEvent {
    Progress(SyncStatus),
    ThreadsChanged {
        account_id: String,
        thread_ids: Vec<String>,
    },
}

/// [`SyncSink`] backed by [`Db`]. Progress reporting is optional: pass
/// [`DbSink::with_progress`] in live paths, [`DbSink::new`] in tests.
#[derive(Clone)]
pub struct DbSink {
    db: Db,
    events: Option<std::sync::Arc<dyn Fn(SyncEvent) + Send + Sync>>,
}

impl DbSink {
    pub fn new(db: Db) -> Self {
        Self { db, events: None }
    }
    pub fn with_progress(db: Db, f: impl Fn(SyncStatus) + Send + Sync + 'static) -> Self {
        Self::with_events(db, move |e| {
            if let SyncEvent::Progress(s) = e {
                f(s);
            }
        })
    }
    pub fn with_events(db: Db, f: impl Fn(SyncEvent) + Send + Sync + 'static) -> Self {
        Self {
            db,
            events: Some(std::sync::Arc::new(f)),
        }
    }
    pub fn db(&self) -> &Db {
        &self.db
    }
}

#[async_trait]
impl SyncSink for DbSink {
    async fn upsert_labels(&self, labels: &[Label]) -> Result<()> {
        for l in labels {
            self.db.labels_upsert(l).await?;
        }
        Ok(())
    }
    async fn insert_stub(&self, id: &str, account: &str, thread: &str) -> Result<()> {
        let (id, aid, tid) = (id.to_string(), account.to_string(), thread.to_string());
        self.db
            .write(move |c| {
                c.execute(
                    "INSERT OR IGNORE INTO messages (id,account_id,thread_id,internal_date,body_state) VALUES (?,?,?,0,'none')",
                    rusqlite::params![id, aid, tid],
                )?;
                Ok(())
            })
            .await
    }
    async fn upsert_message(&self, m: MsgUpsert) -> Result<()> {
        self.db.messages_upsert(m).await
    }
    async fn commit_metadata_batch(&self, batch: &MetadataBatch) -> Result<()> {
        let b = batch.clone();
        self.db
            .write_tx(move |tx| {
                for m in &b.messages {
                    crate::db::messages::messages_upsert_conn(tx, m)?;
                }
                for a in &b.attachments {
                    crate::db::attachments::attachments_put_conn(tx, a)?;
                }
                crate::db::imap::imap_put_uids_conn(tx, &b.account_id, &b.cursor.role, &b.uid_pairs)?;
                crate::db::imap::imap_set_folder_conn(tx, &b.account_id, &b.cursor)?;
                // One recompute per affected thread, inside the same commit:
                // the list refresh sees either the whole batch or none of it.
                let mut seen: std::collections::HashSet<(&str, &str)> =
                    std::collections::HashSet::new();
                for m in &b.messages {
                    if seen.insert((m.account_id.as_str(), m.thread_id.as_str())) {
                        crate::db::threads::recompute_thread_conn(tx, &m.account_id, &m.thread_id)?;
                    }
                }
                Ok(())
            })
            .await
    }
    async fn label_intents(
        &self,
        account: &str,
        message_id: &str,
    ) -> Result<Vec<(Vec<String>, Vec<String>)>> {
        self.db.outbox_label_intents(account, message_id).await
    }
    async fn delete_message(&self, r: &MessageRef, thread: &str) -> Result<()> {
        self.db.messages_delete(r, thread).await
    }
    async fn message_labels(&self, r: &MessageRef) -> Result<Vec<String>> {
        let (aid, mid) = (r.account_id.clone(), r.message_id.clone());
        let j: String = self
            .db
            .read(move |c| {
                Ok(c.query_row(
                    "SELECT label_ids FROM messages WHERE account_id=? AND id=?",
                    rusqlite::params![aid, mid],
                    |r| r.get::<_, String>(0),
                )
                .unwrap_or("[]".into()))
            })
            .await?;
        Ok(serde_json::from_str(&j).unwrap_or_default())
    }
    async fn apply_label_change(
        &self,
        r: &MessageRef,
        add: &[String],
        remove: &[String],
    ) -> Result<(String, String)> {
        self.db.apply_label_change(r, add, remove).await
    }
    async fn message_thread(&self, r: &MessageRef) -> Result<Option<String>> {
        self.db.message_thread(r).await
    }
    async fn message_snippet(&self, r: &MessageRef) -> Result<String> {
        self.db.message_snippet(r).await
    }
    async fn set_snippet(&self, r: &MessageRef, snippet: &str) -> Result<()> {
        self.db.set_snippet(r, snippet).await
    }
    async fn message_flags(&self, r: &MessageRef) -> Result<(bool, bool)> {
        self.db.message_flags(r).await
    }
    async fn message_exists(&self, r: &MessageRef) -> Result<bool> {
        let (aid, mid) = (r.account_id.clone(), r.message_id.clone());
        self.db
            .read(move |c| {
                Ok(c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE account_id=? AND id=?)",
                    rusqlite::params![aid, mid],
                    |r| r.get(0),
                )
                .unwrap_or(false))
            })
            .await
    }
    async fn list_local_messages(&self, account: &str) -> Result<Vec<(String, String)>> {
        let aid = account.to_string();
        self.db
            .read(move |c| {
                let mut s = c.prepare("SELECT id,thread_id FROM messages WHERE account_id=?")?;
                let rows = s
                    .query_map(rusqlite::params![aid], |r| Ok((r.get(0)?, r.get(1)?)))?
                    .collect::<Result<Vec<(String, String)>, _>>()?;
                Ok(rows)
            })
            .await
    }
    async fn next_bodies_to_fetch(
        &self,
        account: &str,
        limit: i64,
        min_date: i64,
    ) -> Result<Vec<MessageRef>> {
        self.db.next_bodies_to_fetch(account, limit, min_date).await
    }
    async fn store_body(&self, b: BodyPut) -> Result<()> {
        self.db.bodies_put(b).await
    }
    async fn store_attachment(&self, a: AttPut) -> Result<()> {
        self.db.attachments_put(a).await
    }
    async fn set_history_id(&self, account: &str, hid: &str) -> Result<()> {
        self.db
            .accounts_set_history(account, hid, crate::db::now_ms())
            .await
    }
    async fn set_sync_state(&self, account: &str, state: &str) -> Result<()> {
        self.db.accounts_set_state(account, state).await
    }
    async fn log_sync(&self, account: &str, kind: &str, detail: &str) -> Result<()> {
        let (a, k, d) = (account.to_string(), kind.to_string(), detail.to_string());
        self.db
            .write(move |c| {
                c.execute(
                    "INSERT INTO sync_log (account_id,at,kind,detail) VALUES (?,?,?,?)",
                    rusqlite::params![a, crate::db::now_ms(), k, d],
                )?;
                Ok(())
            })
            .await
    }
    async fn imap_set_folder(&self, account: &str, cur: &FolderCursor) -> Result<()> {
        self.db.imap_set_folder(account, cur).await
    }
    async fn imap_get_folder(&self, account: &str, role: &str) -> Result<Option<FolderCursor>> {
        self.db.imap_get_folder(account, role).await
    }
    async fn imap_clear_folder(&self, account: &str, role: &str) -> Result<()> {
        self.db.imap_clear_folder(account, role).await
    }
    async fn imap_put_uids(
        &self,
        account: &str,
        role: &str,
        pairs: &[(i64, String)],
    ) -> Result<()> {
        self.db.imap_put_uids(account, role, pairs).await
    }
    async fn imap_delete_uids(&self, account: &str, role: &str, uids: &[i64]) -> Result<()> {
        self.db.imap_delete_uids(account, role, uids).await
    }
    async fn imap_uid_map(&self, account: &str, role: &str) -> Result<Vec<(i64, String)>> {
        self.db.imap_uid_map(account, role).await
    }
    async fn uids_for_message(
        &self,
        account: &str,
        message_id: &str,
    ) -> Result<Vec<(String, i64)>> {
        self.db.uids_for_message(account, message_id).await
    }
    fn progress(&self, status: SyncStatus) {
        if let Some(f) = &self.events {
            f(SyncEvent::Progress(status));
        }
    }
    fn threads_changed(&self, account: &str, thread_ids: &[String]) {
        if thread_ids.is_empty() {
            return;
        }
        if let Some(f) = &self.events {
            f(SyncEvent::ThreadsChanged {
                account_id: account.to_string(),
                thread_ids: thread_ids.to_vec(),
            });
        }
    }
}

/// One prepared delivery (P5.3). Built by `outgoing::prepare` from a draft at a
/// known revision, read back from the outbox payload, and handed to the
/// transport unchanged.
#[derive(Debug, Clone)]
pub struct SendRequest {
    /// Raw MIME including the `Bcc` header (Gmail's REST API delivers from it).
    pub raw: Vec<u8>,
    /// Thread to file the message into, where the transport supports it.
    pub thread_id: Option<String>,
    /// The envelope sender: the selected authorized identity.
    pub from: String,
    /// Every delivery recipient, de-duplicated, Bcc included.
    pub recipients: Vec<String>,
    /// How many `Bcc` headers the raw message is expected to carry, so the
    /// SMTP path can prove it removed them.
    pub bcc_count: usize,
}

/// Transport-agnostic body pipeline shared by backfill and foreground fetch:
/// sanitize → compress → store (+ attachments meta). `ParsedMessage` is the
/// single handoff type for both transports.
pub async fn store_parsed(
    sink: &dyn SyncSink,
    r: &MessageRef,
    parsed: &crate::provider::gmail::mime::ParsedMessage,
) -> Result<()> {
    let message_id = r.message_id.as_str();
    // Inline `sift-att://` references are account-qualified: the scheme
    // resolves within one account and validates ownership.
    let url_path = format!("{}/{}", r.account_id, r.message_id);
    let html = parsed.html.clone();
    let text = parsed.text.clone();
    let (final_html, remote, trackers, dark_safe) = if let Some(h) = html {
        let s = crate::render::sanitize::sanitize(&url_path, &h);
        (Some(s.html), s.remote_images, s.trackers, s.dark_safe)
    } else if let Some(t) = text.clone() {
        let (h, _) = crate::render::text::to_html(&t);
        let s = crate::render::sanitize::sanitize(&url_path, &h);
        (Some(s.html), s.remote_images, s.trackers, s.dark_safe)
    } else {
        (None, 0, 0, true)
    };
    for p in parsed.attachments.iter().chain(parsed.inline.iter()) {
        // A metadata-only parse (BODYSTRUCTURE walk) carries no bytes at all;
        // a genuinely zero-byte attachment has no bytes *and* declares size 0.
        // Storing an empty payload for the former would make the cache hand
        // back an empty file for a large attachment, so the declared size
        // disambiguates. Rows larger than the row-cache cap are fetched on
        // demand instead of being duplicated in SQLite.
        let payload = if p.data.is_empty() {
            (p.size <= 0).then(Vec::new)
        } else if p.data.len() <= crate::attachments::service::ROW_CACHE_CAP {
            Some(p.data.clone())
        } else {
            None
        };
        sink.store_attachment(crate::db::attachments::AttPut {
            id: uuid::Uuid::now_v7().to_string(),
            account_id: r.account_id.clone(),
            message_id: message_id.to_string(),
            gmail_att_id: p.attachment_id.clone(),
            part_id: p.part_id.clone(),
            filename: p.filename.clone(),
            mime: p.mime.clone(),
            size: p.size,
            content_id: p.content_id.clone(),
            is_inline: p.is_inline,
            data: payload,
        })
        .await?;
    }
    sink.store_body(crate::db::bodies::BodyPut {
        account_id: r.account_id.clone(),
        message_id: message_id.to_string(),
        html: final_html.clone(),
        text: text.clone(),
        remote_images: remote,
        trackers,
        dark_safe,
        quoted_from: parsed.quoted_from.map(|q| q as i64),
        unsubscribe: crate::db::bodies::UnsubscribeHeaders {
            list_unsubscribe: parsed.list_unsub.clone(),
            post_value: parsed.list_unsub_post_value.clone(),
            auth_results: parsed.auth_results.clone(),
            trusted: parsed.auth_results_trusted,
        },
    })
    .await?;
    // Snippet backfill: transports without server snippets (IMAP messages
    // past the snippet window) derive one from the fetched body. REST
    // snippets are never empty, so this is a no-op there.
    if sink
        .message_snippet(r)
        .await
        .unwrap_or_default()
        .is_empty()
    {
        let derived = text
            .filter(|s| !s.trim().is_empty())
            .map(|s| crate::provider::imap::message::snippet_of_text(&s))
            .or_else(|| {
                final_html.as_deref().map(|h| {
                    crate::provider::imap::message::snippet_of_text(
                        &crate::render::text::html_to_text(h),
                    )
                })
            })
            .unwrap_or_default();
        let derived: String = derived.chars().take(200).collect();
        if !derived.is_empty() {
            sink.set_snippet(r, &derived).await?;
        }
    }
    Ok(())
}

/// One incremental attachment payload step (P2.4). `Data` chunks arrive in
/// order, followed by exactly one `Done` that reports the exact decoded total.
#[derive(Debug, Clone)]
pub enum AttachmentChunk {
    Data(Vec<u8>),
    Done { total_bytes: u64 },
}

/// Send one chunk, mapping a closed receiver (the consumer cancelled or went
/// away) to a typed cancellation error rather than a silent success.
async fn send_chunk(
    tx: &tokio::sync::mpsc::Sender<AttachmentChunk>,
    chunk: AttachmentChunk,
) -> Result<(), SiftError> {
    tx.send(chunk)
        .await
        .map_err(|_| SiftError::app("attachment_cancelled", "Download cancelled", false))
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;

    /// Sign-in check: email + display name (+ avatar url if known).
    async fn verify(&self) -> Result<ProfileInfo, SiftError>;
    async fn list_labels(&self) -> Result<Vec<Label>, SiftError>;
    async fn create_label(&self, name: &str) -> Result<Label, SiftError>;
    /// Rename a label (P8.5).
    ///
    /// The returned label carries the provider's authoritative id and name:
    /// for a transport whose label id is derived from the folder name (IMAP),
    /// the id legitimately changes, and the caller reconciles its local
    /// mapping to what comes back rather than assuming.
    async fn rename_label(&self, id: &str, name: &str) -> Result<Label, SiftError>;
    /// Delete a label (P8.5). This removes organisation only: Gmail drops the
    /// label from every message, IMAP deletes the folder, and no message is
    /// ever removed by it.
    async fn delete_label(&self, id: &str) -> Result<(), SiftError>;
    /// Whether rename and delete can be offered for this account at all. A UI
    /// must never show a control that cannot work.
    fn label_management(&self) -> bool {
        true
    }
    async fn full_sync(
        &self,
        sink: &dyn SyncSink,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Cursor, SiftError>;
    /// Returns [`PartialOutcome::NeedsFull`] when the cursor is unusable; the
    /// caller (runtime/commands) decides recovery per transport.
    async fn partial_sync(
        &self,
        cursor: &Cursor,
        sink: &dyn SyncSink,
    ) -> Result<PartialOutcome, SiftError>;
    /// Full MIME, parsed per 7.6.
    async fn fetch_body(
        &self,
        message_id: &str,
    ) -> Result<crate::provider::gmail::mime::ParsedMessage, SiftError>;
    /// Backfill variant: transports with a download budget (IMAP 1 GB/day)
    /// enforce it here. Foreground `fetch_body` is never budgeted. The
    /// default is unlimited (REST path, unchanged behavior).
    async fn fetch_body_backfill(
        &self,
        message_id: &str,
    ) -> Result<crate::provider::gmail::mime::ParsedMessage, SiftError> {
        self.fetch_body(message_id).await
    }
    async fn fetch_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, SiftError>;
    /// Incremental attachment payload (P2.4). `Data` chunks are delivered in
    /// order, then exactly one `Done` carrying the exact decoded total; the
    /// return value repeats that total. A transport with real streaming
    /// overrides this; the default forwards [`Provider::fetch_attachment`] as
    /// a single chunk. Returning `Err` after some `Data` chunks means the
    /// payload is incomplete and must be discarded.
    async fn fetch_attachment_chunks(
        &self,
        message_id: &str,
        attachment_id: &str,
        tx: tokio::sync::mpsc::Sender<AttachmentChunk>,
    ) -> Result<u64, SiftError> {
        let bytes = self.fetch_attachment(message_id, attachment_id).await?;
        let total = bytes.len() as u64;
        if !bytes.is_empty() {
            send_chunk(&tx, AttachmentChunk::Data(bytes)).await?;
        }
        send_chunk(&tx, AttachmentChunk::Done { total_bytes: total }).await?;
        Ok(total)
    }
    /// Raw MIME for "View source". Text-shaped because the reader shows it;
    /// never use this for export, because decoding arbitrary bytes as UTF-8 is
    /// lossy and an exported `.eml` must be byte-exact (P9.3).
    async fn fetch_raw(&self, message_id: &str) -> Result<String, SiftError>;
    /// Raw MIME as bytes, for Save as `.eml` and for the offline raw cache.
    async fn fetch_raw_bytes(&self, message_id: &str) -> Result<Vec<u8>, SiftError>;
    /// One outbox op from the 8.4 table. Idempotent: re-applying an already
    /// applied op returns [`ApplyOutcome::AlreadyApplied`].
    async fn apply(&self, op: &OutboxOp) -> Result<ApplyOutcome, SiftError>;
    /// Deliver one prepared message.
    ///
    /// The envelope is explicit: recipients come from the prepared draft, not
    /// from re-parsing headers, and the SMTP transport removes the `Bcc`
    /// header before DATA because the envelope already carries those
    /// recipients ([`SendRequest::bcc_count`] is what the check compares
    /// against).
    ///
    /// A transport that loses contact after the message may have been
    /// submitted returns an error carrying the code `send_uncertain`: the
    /// outbox turns that into `uncertain` and never resubmits (P6.1).
    async fn send(&self, req: &SendRequest) -> Result<SentInfo, SiftError>;

    /// Did the provider accept a message carrying this stable RFC Message-ID?
    ///
    /// This is how an `uncertain` send is reconciled (P6.1). The default is
    /// "unknown": a transport that cannot answer leaves the operation
    /// uncertain rather than claiming an outcome it cannot prove.
    async fn sent_by_rfc_message_id(
        &self,
        _rfc_message_id: &str,
    ) -> Result<Option<SentInfo>, SiftError> {
        Ok(None)
    }
    /// Upsert the remote copy of a draft and return its identity: the remote
    /// draft id plus, where the transport knows it, the message id the copy
    /// carries locally.
    ///
    /// `rfc_message_id` is the stable Message-ID of the draft's lineage: a
    /// transport whose APPEND outcome is ambiguous reconciles against it
    /// instead of appending a second copy.
    async fn draft_upsert(
        &self,
        remote_id: Option<&str>,
        raw: &[u8],
        rfc_message_id: &str,
    ) -> Result<crate::dto::RemoteDraft, SiftError>;
    async fn draft_delete(&self, remote_id: &str) -> Result<(), SiftError>;
    /// Drafts that exist on the server, for import and reconciliation (P5.2).
    ///
    /// Message content is not fetched here: a remote draft's body is read from
    /// the message copy the normal sync already stores. Transports without a
    /// cheap listing return an empty list rather than an error.
    async fn draft_list(&self) -> Result<Vec<crate::dto::RemoteDraft>, SiftError> {
        Ok(Vec::new())
    }
    /// Server search that also hydrates + indexes hits, so the second run is
    /// local (P8-T04). Returns thread refs for row mapping.
    async fn server_search(
        &self,
        q: &str,
        limit: u32,
        sink: &dyn SyncSink,
    ) -> Result<Vec<ThreadRef>, SiftError>;
    /// Send-as identities. IMAP exposes the primary address only.
    async fn send_as_list(&self) -> Result<Vec<SendAs>, SiftError>;
    /// Push trigger. IMAP IDLE; None for REST.
    fn watch(&self) -> Option<BoxStream<'static, WatchEvent>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p11_cursor_json_roundtrip_and_legacy() {
        // Legacy plain history id passes through as Gmail cursor.
        assert_eq!(
            Cursor::parse("123456789"),
            Cursor::Gmail {
                history_id: "123456789".into()
            }
        );
        assert_eq!(Cursor::parse("123456789").render(), "123456789".to_string());
        // Empty string = no cursor (fresh account reconciles).
        assert_eq!(
            Cursor::parse(""),
            Cursor::Gmail {
                history_id: "".into()
            }
        );
        // IMAP form round-trips.
        let c = Cursor::Imap {
            folders: vec![crate::db::imap::FolderCursor {
                role: "all".into(),
                name: "[Gmail]/Alle Nachrichten".into(),
                uidvalidity: 987654,
                uidnext: 4523,
                highestmodseq: Some(89123),
                exists_count: 412,
                last_full_scan: None,
            }],
        };
        let rendered = c.render();
        assert!(rendered.contains("\"imap\""));
        assert_eq!(Cursor::parse(&rendered), c);
        // Garbage never crashes: degrades to empty Gmail cursor path.
        assert!(matches!(Cursor::parse("{oops"), Cursor::Gmail { .. }));
    }
}
