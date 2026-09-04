//! Gmail IMAP/SMTP provider (Phase 11): app-password sign-in, same rows as
//! the REST path. Folders are discovered once and cached; per-call state
//! (cursors, uid maps) lives in SQLite via the sink.
//!
//! Not yet wired (later steps): `apply` (task 10), `send` + drafts (task 11),
//! `watch` IDLE events (task 9). Those return a clear error until then;
//! nothing constructs this provider in production paths yet.
use super::{
    conn::{Caps, ImapPool},
    folders::{self, FolderMap},
    message,
};
use crate::db::Db;
use crate::dto::Label;
use crate::errors::SiftError;
use crate::provider::gmail::mime::ParsedMessage;
use crate::provider::{
    ApplyOutcome, Cursor, OutboxOp, PartialOutcome, ProfileInfo, Provider, ProviderKind, SendAs,
    SentInfo, SyncSink, ThreadRef, WatchEvent,
};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex as StdMutex;
use tokio::sync::Mutex as TokioMutex;

/// Daily backfill download budget per account (spec task 8).
pub const DAILY_BUDGET_BYTES: u64 = 1_000_000_000;

pub struct GmailImapProvider {
    account_id: String,
    pool: ImapPool,
    folders: std::sync::Arc<TokioMutex<Option<FolderMap>>>,
    db: Db,
    budget: StdMutex<BudgetState>,
    polls: StdMutex<u64>,
}

#[derive(Default)]
struct BudgetState {
    paused_until: Option<std::time::Instant>,
}

impl GmailImapProvider {
    pub fn new(account_id: String, pool: ImapPool, db: Db) -> Self {
        Self {
            account_id,
            pool,
            folders: std::sync::Arc::new(TokioMutex::new(None)),
            db,
            budget: StdMutex::new(BudgetState::default()),
            polls: StdMutex::new(0),
        }
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    /// Discover (once) and cache the folder map.
    pub async fn folders_cached(&self) -> Result<FolderMap, SiftError> {
        let mut guard = self.folders.lock().await;
        if guard.is_none() {
            let mut conn = self.pool.worker().await?;
            let map = folders::discover(conn.as_mut().expect("connected")).await?;
            *guard = Some(map);
        }
        Ok(guard.clone().expect("cached"))
    }

    pub async fn caps(&self) -> Caps {
        self.pool.caps().await
    }

    /// Resolve a message to (folder server name, uid), preferring `\All`.
    /// Falls back to `UID SEARCH X-GM-MSGID` per folder for messages that
    /// arrived via another client since the last sync (spec task 10).
    async fn locate(&self, message_id: &str) -> Result<(String, String, u32), SiftError> {
        let known = self
            .db
            .uids_for_message(&self.account_id, message_id)
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        let folders = self.folders_cached().await?;
        let pick = known
            .iter()
            .find(|(r, _)| r == "all")
            .or_else(|| known.first());
        if let Some((role, uid)) = pick {
            let name = folders
                .name_for_role(role)
                .ok_or_else(|| SiftError::app("imap_protocol", "unknown folder role", false))?;
            return Ok((role.clone(), name.to_string(), *uid as u32));
        }
        // Unknown locally: search every folder by Gmail message id.
        let dec = u64::from_str_radix(message_id, 16)
            .map_err(|_| SiftError::app("bad_id", "not a Gmail message id", false))?;
        for role in ["all", "trash", "junk"] {
            let Some(name) = folders.name_for_role(role) else {
                continue;
            };
            let mut guard = self.pool.worker().await?;
            let conn = guard.as_mut().expect("connected");
            conn.select(name, true).await?;
            let found = conn.uid_search_gmmsgid(dec).await?;
            if let Some(uid) = found.into_iter().next() {
                self.db
                    .imap_put_uids(&self.account_id, role, &[(uid as i64, message_id.into())])
                    .await
                    .map_err(|e| SiftError::app("db", e.to_string(), false))?;
                return Ok((role.into(), name.into(), uid));
            }
        }
        Err(SiftError::NotFound("message".into()))
    }

    /// Raw MIME bytes for a message (shared by body/raw/attachment paths).
    async fn fetch_raw_bytes(&self, message_id: &str) -> Result<(Vec<u8>, u64), SiftError> {
        let (_role, folder, uid) = self.locate(message_id).await?;
        let mut guard = self.pool.worker().await?;
        let conn = guard.as_mut().expect("connected");
        conn.select(&folder, true).await?;
        let rows = conn
            .uid_fetch(&uid.to_string(), "(UID BODY.PEEK[])")
            .await?;
        for (_seq, attrs) in &rows {
            for a in attrs {
                if let super::proto::FetchAttr::BodySection { bytes, .. }
                | super::proto::FetchAttr::HeaderFields(bytes) = a
                {
                    if !bytes.is_empty() {
                        let dec = u64::from_str_radix(message_id, 16).unwrap_or(0);
                        return Ok((bytes.clone(), dec));
                    }
                }
            }
        }
        Err(SiftError::NotFound("body".into()))
    }

    fn budget_key(&self) -> String {
        format!("imap_budget_{}", self.account_id)
    }

    async fn budget_today(&self) -> (String, u64) {
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let raw = self
            .db
            .setting_get_raw(&self.budget_key())
            .await
            .ok()
            .flatten();
        if let Some(raw) = raw {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
                if v.get("date").and_then(|d| d.as_str()) == Some(today.as_str()) {
                    return (today, v.get("bytes").and_then(|b| b.as_u64()).unwrap_or(0));
                }
            }
        }
        (today, 0)
    }
}

#[async_trait]
impl Provider for GmailImapProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::GmailImap
    }

    async fn verify(&self) -> Result<ProfileInfo, SiftError> {
        // worker() connects + logs in; success is the verification.
        self.pool.worker().await?;
        Ok(ProfileInfo {
            email: self.pool.email(),
            display_name: None,
            avatar_url: None,
        })
    }

    async fn list_labels(&self) -> Result<Vec<Label>, SiftError> {
        // System labels keep REST ids so every query works unchanged.
        let mut out: Vec<Label> = sys_labels(&self.account_id);
        for (i, (name, id)) in self.folders_cached().await?.user.iter().enumerate() {
            out.push(Label {
                account_id: self.account_id.clone(),
                id: id.clone(),
                name: name.clone(),
                kind: "user".into(),
                color_bg: None,
                color_fg: None,
                visible: true,
                unread_count: 0,
                total_count: 0,
                sort_order: 200 + i as i64,
            });
        }
        Ok(out)
    }

    async fn create_label(&self, name: &str) -> Result<Label, SiftError> {
        let folders = self.folders_cached().await?;
        // IMAP folder names travel UTF-7; the label id uses the decoded name.
        let encoded = super::proto::encode_utf7_mailbox(name);
        {
            let mut guard = self.pool.worker().await?;
            let conn = guard.as_mut().expect("connected");
            match conn.create(&encoded).await {
                Ok(()) => {}
                Err(SiftError::App { code, .. }) if code == "imap_error" => {
                    // Best-effort idempotency is handled by callers checking
                    // existing labels first; surface anything else.
                    return Err(SiftError::app("imap_error", "CREATE failed", false));
                }
                Err(e) => return Err(e),
            }
        }
        // Refresh the cached map so the new folder resolves immediately.
        {
            let mut guard = self.pool.worker().await?;
            let conn = guard.as_mut().expect("connected");
            if let Ok(map) = folders::discover(conn).await {
                *self.folders.lock().await = Some(map);
            }
        }
        let _ = folders;
        Ok(Label {
            account_id: self.account_id.clone(),
            id: format!("imap:{name}"),
            name: name.into(),
            kind: "user".into(),
            color_bg: None,
            color_fg: None,
            visible: true,
            unread_count: 0,
            total_count: 0,
            sort_order: 200,
        })
    }

    async fn full_sync(
        &self,
        sink: &dyn SyncSink,
        cancel: tokio_util::sync::CancellationToken,
    ) -> Result<Cursor, SiftError> {
        // Fresh discovery: folders may have changed since caching.
        let folders = {
            let mut guard = self.pool.worker().await?;
            let map = folders::discover(guard.as_mut().expect("connected")).await?;
            *self.folders.lock().await = Some(map.clone());
            map
        };
        super::full::run_full_sync(&self.pool, sink, &self.account_id, &folders, cancel).await
    }

    async fn partial_sync(
        &self,
        cursor: &Cursor,
        sink: &dyn SyncSink,
    ) -> Result<PartialOutcome, SiftError> {
        let Cursor::Imap { .. } = cursor else {
            // Fresh account (or a REST cursor after re-adding): full sync.
            return Ok(PartialOutcome::NeedsFull);
        };
        let folders = self.folders_cached().await?;
        let mut prev = HashMap::new();
        for role in ["all", "trash", "junk"] {
            // Prefer the stored cursor (survives restarts); fall back to the
            // snapshot embedded in history_id.
            let stored = sink
                .imap_get_folder(&self.account_id, role)
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            if let Some(c) = stored.or_else(|| {
                if let Cursor::Imap { folders } = cursor {
                    folders.iter().find(|f| f.role == role).cloned()
                } else {
                    None
                }
            }) {
                prev.insert(role.to_string(), c);
            }
        }
        let force = {
            let mut polls = self.polls.lock().unwrap();
            *polls += 1;
            *polls % 10 == 0
        };
        let cancel = tokio_util::sync::CancellationToken::new();
        super::partial::run_partial_sync(
            &self.pool,
            sink,
            &self.account_id,
            &folders,
            &prev,
            force,
            &cancel,
        )
        .await
    }

    async fn fetch_body(&self, message_id: &str) -> Result<ParsedMessage, SiftError> {
        let (raw, _) = self.fetch_raw_bytes(message_id).await?;
        super::convert::parse_raw(&raw)
            .ok_or_else(|| SiftError::app("fetch", "message did not parse", false))
    }

    async fn fetch_body_backfill(&self, message_id: &str) -> Result<ParsedMessage, SiftError> {
        // Backfill budget: 1 GB/day/account, then pause until local midnight.
        // Foreground fetch_body is never budgeted.
        if let Some(until) = self.budget.lock().unwrap().paused_until {
            if std::time::Instant::now() < until {
                return Err(SiftError::app("imap_transient", "backfill paused", true));
            }
        }
        let (today, used) = self.budget_today().await;
        if used >= DAILY_BUDGET_BYTES {
            return Err(SiftError::app(
                "backfill_budget",
                "Daily backfill budget used up; resumes after midnight.",
                false,
            ));
        }
        let (raw, _) = self
            .fetch_raw_bytes(message_id)
            .await
            .inspect_err(|e| {
                if matches!(e, SiftError::App { code, .. } if code == "imap_transient") {
                    self.budget.lock().unwrap().paused_until =
                        Some(std::time::Instant::now() + std::time::Duration::from_secs(600));
                }
            })?;
        let n = raw.len() as u64;
        let parsed = super::convert::parse_raw(&raw)
            .ok_or_else(|| SiftError::app("fetch", "message did not parse", false))?;
        let v = serde_json::json!({ "date": today, "bytes": used + n });
        let _ = self
            .db
            .setting_set_raw(&self.budget_key(), &v.to_string())
            .await;
        Ok(parsed)
    }

    async fn fetch_attachment(
        &self,
        message_id: &str,
        attachment_id: &str,
    ) -> Result<Vec<u8>, SiftError> {
        // attachment_id is the IMAP section path (stored in part_id).
        let (_role, folder, uid) = self.locate(message_id).await?;
        let mut guard = self.pool.worker().await?;
        let conn = guard.as_mut().expect("connected");
        conn.select(&folder, true).await?;
        // Encoding comes from a fresh BODYSTRUCTURE (cheap, small).
        let rows = conn
            .uid_fetch(&uid.to_string(), "(UID BODYSTRUCTURE)")
            .await?;
        let mut encoding = "7bit".to_string();
        for (_seq, attrs) in &rows {
            for a in attrs {
                if let super::proto::FetchAttr::BodyStructure(bs) = a {
                    for (num, part) in super::proto::walk_parts(bs) {
                        if num == attachment_id {
                            if let super::proto::BodyStruct::Single { encoding: e, .. } = part {
                                encoding = e.clone();
                            }
                        }
                    }
                }
            }
        }
        let rows = conn
            .uid_fetch(&uid.to_string(), &format!("UID BODY.PEEK[{attachment_id}]"))
            .await?;
        for (_seq, attrs) in &rows {
            for a in attrs {
                if let super::proto::FetchAttr::BodySection { bytes, .. } = a {
                    if !bytes.is_empty() {
                        return Ok(message::decode_transfer(bytes, &encoding));
                    }
                }
                if let super::proto::FetchAttr::HeaderFields(bytes) = a {
                    if !bytes.is_empty() {
                        return Ok(bytes.clone());
                    }
                }
            }
        }
        Err(SiftError::NotFound("attachment".into()))
    }

    async fn fetch_raw(&self, message_id: &str) -> Result<String, SiftError> {
        let (raw, _) = self.fetch_raw_bytes(message_id).await?;
        Ok(String::from_utf8_lossy(&raw).into_owned())
    }

    async fn apply(&self, _op: &OutboxOp) -> Result<ApplyOutcome, SiftError> {
        // Task 10 (outbox ops over IMAP).
        Err(SiftError::app(
            "unimplemented",
            "IMAP ops land in task 10",
            false,
        ))
    }

    async fn send(&self, _raw: &[u8], _thread_id: Option<&str>) -> Result<SentInfo, SiftError> {
        // Task 11 (SMTP via lettre).
        Err(SiftError::app(
            "unimplemented",
            "IMAP send lands in task 11",
            false,
        ))
    }

    async fn draft_upsert(
        &self,
        _remote_id: Option<&str>,
        _raw: &[u8],
    ) -> Result<String, SiftError> {
        // Task 10 (draft APPEND).
        Err(SiftError::app(
            "unimplemented",
            "IMAP drafts land in task 10",
            false,
        ))
    }

    async fn draft_delete(&self, _remote_id: &str) -> Result<(), SiftError> {
        Err(SiftError::app(
            "unimplemented",
            "IMAP drafts land in task 10",
            false,
        ))
    }

    async fn server_search(
        &self,
        q: &str,
        limit: u32,
        sink: &dyn SyncSink,
    ) -> Result<Vec<ThreadRef>, SiftError> {
        let folders = self.folders_cached().await?;
        let name = folders
            .name_for_role("all")
            .ok_or_else(|| SiftError::app("imap_protocol", "no All folder", false))?;
        let uids = {
            let mut guard = self.pool.worker().await?;
            let conn = guard.as_mut().expect("connected");
            conn.select(name, true).await?;
            conn.uid_search_raw(q).await?
        };
        let mut out = vec![];
        let mut count = 0u32;
        for chunk in uids.chunks(50) {
            if count >= limit {
                break;
            }
            let items = {
                let mut guard = self.pool.worker().await?;
                let conn = guard.as_mut().expect("connected");
                message::fetch_meta(conn, chunk).await?
            };
            for item in &items {
                if count >= limit {
                    break;
                }
                let extra: Vec<String> = vec![];
                if let Some(up) =
                    message::meta_to_upsert(&self.account_id, item, String::new(), &extra, false)
                {
                    let tid = up.thread_id.clone();
                    let mid = up.id.clone();
                    let _ = sink.upsert_message(up).await;
                    out.push(ThreadRef {
                        thread_id: tid,
                        message_id: mid,
                    });
                    count += 1;
                }
            }
        }
        Ok(out)
    }

    async fn send_as_list(&self) -> Result<Vec<SendAs>, SiftError> {
        // Gmail exposes no alias list over IMAP: primary address only.
        Ok(vec![SendAs {
            email: self.pool.email(),
            display_name: None,
            is_primary: true,
        }])
    }

    fn watch(&self) -> Option<crate::provider::BoxStream<'static, WatchEvent>> {
        let (tx, rx) = futures::channel::mpsc::unbounded();
        let pool = self.pool.clone();
        tokio::spawn(async move {
            super::idle::idle_loop(
                pool,
                tx,
                std::time::Duration::from_secs(25 * 60),
                std::time::Duration::from_secs(30),
            )
            .await;
        });
        Some(Box::pin(rx))
    }
}

/// System labels shared by full sync and list_labels (REST ids everywhere).
pub(crate) fn sys_labels(account_id: &str) -> Vec<Label> {
    [
        "INBOX",
        "STARRED",
        "SENT",
        "DRAFT",
        "SPAM",
        "TRASH",
        "UNREAD",
        "IMPORTANT",
    ]
    .iter()
    .enumerate()
    .map(|(i, id)| Label {
        account_id: account_id.into(),
        id: id.to_string(),
        name: id.to_string(),
        kind: "system".into(),
        color_bg: None,
        color_fg: None,
        visible: *id != "UNREAD",
        unread_count: 0,
        total_count: 0,
        sort_order: i as i64,
    })
    .collect()
}
