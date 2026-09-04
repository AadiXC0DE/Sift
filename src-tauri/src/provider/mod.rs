//! Transport abstraction (Phase 11 task 1).
//!
//! Every mailbox operation the app performs goes through [`Provider`]; the
//! sync engine, outbox drain, backfill, URI scheme, and commands never touch a
//! transport client directly. Providers are constructed **per account** (they
//! carry their own credentials), so methods take no account id.
//!
//! * [`gmail::GmailApiProvider`] — Gmail REST API (OAuth). Delegates to the
//!   long-tested REST code; behavior for OAuth accounts is unchanged.
//! * `imap::GmailImapProvider` — Gmail IMAP/SMTP (app password). Added in the
//!   IMAP step; same rows, same UI, same speed.
use crate::db::{
    attachments::AttPut, bodies::BodyPut, imap::FolderCursor, messages::MsgUpsert, Db,
};
use crate::dto::{Label, SyncStatus};
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

#[derive(Debug, Clone)]
pub enum PartialOutcome {
    Synced {
        changed_threads: Vec<(String, String)>,
        new_inbox: Vec<(String, String, String, String)>,
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

/// Everything a sync or mutation path needs from the local store. The only
/// implementation is [`DbSink`] over [`Db`]; providers never touch SQLite
/// directly, which keeps both transports testable against temp DBs.
#[async_trait]
pub trait SyncSink: Send + Sync {
    // -- labels / messages -------------------------------------------------
    async fn upsert_labels(&self, labels: &[Label]) -> Result<()>;
    async fn insert_stub(&self, id: &str, account: &str, thread: &str) -> Result<()>;
    async fn upsert_message(&self, m: MsgUpsert) -> Result<()>;
    async fn delete_message(&self, id: &str, account: &str, thread: &str) -> Result<()>;
    async fn message_labels(&self, id: &str) -> Result<Vec<String>>;
    async fn apply_label_change(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<(String, String)>;
    async fn message_thread(&self, id: &str) -> Result<Option<(String, String)>>;
    async fn message_exists(&self, id: &str) -> Result<bool>;
    async fn list_local_messages(&self, account: &str) -> Result<Vec<(String, String)>>;
    // -- bodies / attachments -----------------------------------------------
    async fn next_bodies_to_fetch(
        &self,
        account: &str,
        limit: i64,
        min_date: i64,
    ) -> Result<Vec<String>>;
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
}

/// [`SyncSink`] backed by [`Db`]. Progress reporting is optional: pass
/// [`DbSink::with_progress`] in live paths, [`DbSink::new`] in tests.
#[derive(Clone)]
pub struct DbSink {
    db: Db,
    progress: Option<std::sync::Arc<dyn Fn(SyncStatus) + Send + Sync>>,
}

impl DbSink {
    pub fn new(db: Db) -> Self {
        Self { db, progress: None }
    }
    pub fn with_progress(db: Db, f: impl Fn(SyncStatus) + Send + Sync + 'static) -> Self {
        Self {
            db,
            progress: Some(std::sync::Arc::new(f)),
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
    async fn delete_message(&self, id: &str, account: &str, thread: &str) -> Result<()> {
        self.db.messages_delete(id, account, thread).await
    }
    async fn message_labels(&self, id: &str) -> Result<Vec<String>> {
        let mid = id.to_string();
        let j: String = self
            .db
            .read(move |c| {
                Ok(c.query_row(
                    "SELECT label_ids FROM messages WHERE id=?",
                    rusqlite::params![mid],
                    |r| r.get::<_, String>(0),
                )
                .unwrap_or("[]".into()))
            })
            .await?;
        Ok(serde_json::from_str(&j).unwrap_or_default())
    }
    async fn apply_label_change(
        &self,
        id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<(String, String)> {
        self.db.apply_label_change(id, add, remove).await
    }
    async fn message_thread(&self, id: &str) -> Result<Option<(String, String)>> {
        self.db.message_thread(id).await
    }
    async fn message_exists(&self, id: &str) -> Result<bool> {
        let mid = id.to_string();
        self.db
            .read(move |c| {
                Ok(c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?)",
                    rusqlite::params![mid],
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
    ) -> Result<Vec<String>> {
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
        if let Some(f) = &self.progress {
            f(status);
        }
    }
}

/// Transport-agnostic body pipeline shared by backfill and foreground fetch:
/// sanitize → compress → store (+ attachments meta). `ParsedMessage` is the
/// single handoff type for both transports.
pub async fn store_parsed(
    sink: &dyn SyncSink,
    message_id: &str,
    parsed: &crate::provider::gmail::mime::ParsedMessage,
) -> Result<()> {
    let html = parsed.html.clone();
    let text = parsed.text.clone();
    let (final_html, remote, trackers, dark_safe) = if let Some(h) = html {
        let s = crate::render::sanitize::sanitize(message_id, &h);
        (Some(s.html), s.remote_images, s.trackers, s.dark_safe)
    } else if let Some(t) = text.clone() {
        let (h, _) = crate::render::text::to_html(&t);
        let s = crate::render::sanitize::sanitize(message_id, &h);
        (Some(s.html), s.remote_images, s.trackers, s.dark_safe)
    } else {
        (None, 0, 0, true)
    };
    for p in parsed.attachments.iter().chain(parsed.inline.iter()) {
        let small = if p.data.len() < 64 * 1024 {
            Some(p.data.clone())
        } else {
            None
        };
        sink.store_attachment(crate::db::attachments::AttPut {
            id: uuid::Uuid::now_v7().to_string(),
            message_id: message_id.to_string(),
            gmail_att_id: p.attachment_id.clone(),
            part_id: p.part_id.clone(),
            filename: p.filename.clone(),
            mime: p.mime.clone(),
            size: p.size,
            content_id: p.content_id.clone(),
            is_inline: p.is_inline,
            data: small,
        })
        .await?;
    }
    sink.store_body(crate::db::bodies::BodyPut {
        message_id: message_id.to_string(),
        html: final_html,
        text,
        remote_images: remote,
        trackers,
        dark_safe,
        quoted_from: parsed.quoted_from.map(|q| q as i64),
    })
    .await?;
    Ok(())
}

#[async_trait]
pub trait Provider: Send + Sync {
    fn kind(&self) -> ProviderKind;
    /// Sign-in check: email + display name (+ avatar url if known).
    async fn verify(&self) -> Result<ProfileInfo, SiftError>;
    async fn list_labels(&self) -> Result<Vec<Label>, SiftError>;
    async fn create_label(&self, name: &str) -> Result<Label, SiftError>;
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
    async fn fetch_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, SiftError>;
    /// "View source".
    async fn fetch_raw(&self, message_id: &str) -> Result<String, SiftError>;
    /// One outbox op from the 8.4 table. Idempotent: re-applying an already
    /// applied op returns [`ApplyOutcome::AlreadyApplied`].
    async fn apply(&self, op: &OutboxOp) -> Result<ApplyOutcome, SiftError>;
    async fn send(&self, raw: &[u8], thread_id: Option<&str>) -> Result<SentInfo, SiftError>;
    /// Upsert a remote draft; returns the remote draft id.
    async fn draft_upsert(&self, remote_id: Option<&str>, raw: &[u8]) -> Result<String, SiftError>;
    async fn draft_delete(&self, remote_id: &str) -> Result<(), SiftError>;
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
