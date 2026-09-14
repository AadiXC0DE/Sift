//! Gmail IMAP/SMTP provider (Phase 11): app-password sign-in, same rows as
//! the REST path. Folders are discovered once and cached; per-call state
//! (cursors, uid maps) lives in SQLite via the sink.
//!
//! Not yet wired (later steps): `apply` (task 10), `send` + drafts (task 11),
//! `watch` IDLE events (task 9). Those return a clear error until then;
//! nothing constructs this provider in production paths yet.
use super::{
    conn::{Caps, Conn, FetchItems, ImapPool, TaggedFailure, FOREGROUND_IDLE},
    errors as imap_errors,
    folders::{self, FolderMap},
    ids::{self, ImapSection},
    message,
    proto::{BodyStruct, FetchAttr},
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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::{Mutex as TokioMutex, OwnedMutexGuard};

/// Daily backfill download budget per account (spec task 8).
pub const DAILY_BUDGET_BYTES: u64 = 1_000_000_000;

/// Whole-operation guard for a user-initiated attachment read (P1.6). The
/// command layer keeps its own guard; this bounds the provider's share.
const ATTACHMENT_OP_TIMEOUT: Duration = Duration::from_secs(60);

/// Roles searched for a message's transport locator, in order (P1.4).
const PREFERRED_ROLES: [&str; 5] = ["all", "trash", "junk", "drafts", "sent"];

fn db_err(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

/// A folder that no longer exists is skipped; anything else propagates.
fn is_folder_missing(e: &SiftError) -> bool {
    matches!(e, SiftError::App { code, retryable, .. } if code == "imap_error" && !*retryable)
}

fn attr_uid(attrs: &[FetchAttr]) -> Option<u32> {
    attrs.iter().find_map(|a| match a {
        FetchAttr::Uid(u) => Some(*u),
        _ => None,
    })
}

fn attr_msgid(attrs: &[FetchAttr]) -> Option<u64> {
    attrs.iter().find_map(|a| match a {
        FetchAttr::GmailMsgId(v) => Some(*v),
        _ => None,
    })
}

/// Selected-mailbox batches opened without a caller cancellation token (the
/// account generation's own token cancels the whole loop instead).
fn no_cancel_token() -> tokio_util::sync::CancellationToken {
    tokio_util::sync::CancellationToken::new()
}

/// Transfer encoding of one section, from a BODYSTRUCTURE we already hold.
///
/// `None` means the section is not an addressable single part of this
/// BODYSTRUCTURE. The old code silently defaulted to `7bit`, which decoded
/// unknown/missing parts as if they were text (P2.4); callers must treat
/// `None` as a missing part.
fn encoding_for(attrs: &[FetchAttr], section: &ImapSection) -> Option<String> {
    let want = section.wire();
    for a in attrs {
        if let FetchAttr::BodyStructure(bs) = a {
            for (num, part) in super::proto::walk_parts(bs) {
                if num == want {
                    return match part {
                        BodyStruct::Single { encoding, .. } => Some(encoding.clone()),
                        // A multipart container has no bytes of its own.
                        BodyStruct::Multipart { .. } => None,
                    };
                }
            }
        }
    }
    None
}

/// A verified transport locator: the folder/uid Gmail uses for a message,
/// confirmed against its Gmail message id (P1.4).
struct Locator {
    uid: u32,
    gmail_id: u64,
    /// The verified `(UID X-GM-MSGID BODYSTRUCTURE)` row.
    attrs: Vec<FetchAttr>,
}

/// One shared attachment download. Extra consumers join it instead of
/// starting a second transfer; the work is cancelled only when the last
/// consumer goes away (P1.5).
struct SharedFetch {
    result: TokioMutex<Option<SharedOutcome>>,
    done: tokio::sync::watch::Sender<bool>,
    waiters: AtomicUsize,
    cancel: tokio_util::sync::CancellationToken,
    finished: AtomicBool,
}

enum SharedOutcome {
    Ok(Arc<Vec<u8>>),
    Err(Arc<SiftError>),
}

impl SharedFetch {
    fn new() -> Self {
        Self {
            result: TokioMutex::new(None),
            done: tokio::sync::watch::channel(false).0,
            waiters: AtomicUsize::new(0),
            cancel: tokio_util::sync::CancellationToken::new(),
            finished: AtomicBool::new(false),
        }
    }
}

/// Counts one consumer of a [`SharedFetch`]; abandons the download only when
/// the last consumer is gone.
struct WaitGuard(Arc<SharedFetch>);

impl WaitGuard {
    fn new(s: &Arc<SharedFetch>) -> Self {
        s.waiters.fetch_add(1, Ordering::AcqRel);
        Self(s.clone())
    }
}

impl Drop for WaitGuard {
    fn drop(&mut self) {
        if self.0.waiters.fetch_sub(1, Ordering::AcqRel) == 1
            && !self.0.finished.load(Ordering::Acquire)
        {
            self.0.cancel.cancel();
        }
    }
}

/// `SiftError` is not `Clone`; rebuild an equivalent for co-consumers.
fn duplicate_error(e: &SiftError) -> SiftError {
    if let Ok(v) = serde_json::to_value(e) {
        return SiftError::app(
            v["code"].as_str().unwrap_or("attachment_offline"),
            v["message"]
                .as_str()
                .unwrap_or("attachment download failed"),
            v["retryable"].as_bool().unwrap_or(false),
        );
    }
    SiftError::app("attachment_offline", "attachment download failed", true)
}

/// The account's single bounded foreground connection (P1.5). Sift owns at
/// most three IMAP connections per account: the sync worker, the IDLE slot,
/// and this lease.
struct Foreground {
    conn: Option<Conn>,
    idle_since: Instant,
}

impl Foreground {
    fn new() -> Self {
        Self {
            conn: None,
            idle_since: Instant::now(),
        }
    }

    /// Take the connection out for exclusive use. A connection that sat idle
    /// longer than [`FOREGROUND_IDLE`] is dropped first.
    async fn take(&mut self, pool: &ImapPool) -> Result<Conn, SiftError> {
        if self.conn.is_some() && self.idle_since.elapsed() > FOREGROUND_IDLE {
            log::debug!(target: "sift::imap", "dropping idle foreground connection");
            self.conn = None;
        }
        match self.conn.take() {
            Some(c) => Ok(c),
            None => pool.fresh_conn().await,
        }
    }

    fn give_back(&mut self, conn: Conn) {
        self.conn = Some(conn);
        self.idle_since = Instant::now();
    }
}

/// Exclusive use of the foreground connection for one exchange.
///
/// Dropping the lease without [`Lease::keep`] closes the socket: that is the
/// required behaviour for cancellation and for any error that may have left
/// the stream mis-framed.
struct Lease {
    guard: OwnedMutexGuard<Foreground>,
    conn: Option<Conn>,
}

impl Lease {
    fn conn(&mut self) -> &mut Conn {
        self.conn.as_mut().expect("leased connection")
    }

    /// Return a healthy connection to the slot for reuse.
    fn keep(mut self) {
        if let Some(c) = self.conn.take() {
            self.guard.give_back(c);
        }
    }
}

/// Clonable I/O context so a shared foreground read can run in its own task.
#[derive(Clone)]
struct FetchCtx {
    account_id: String,
    pool: ImapPool,
    folders: Arc<TokioMutex<Option<FolderMap>>>,
    db: Db,
    foreground: Arc<TokioMutex<Foreground>>,
}

impl FetchCtx {
    fn corr(&self, message_id: &str, part: &str) -> String {
        imap_errors::correlation_id(&self.account_id, message_id, part)
    }

    /// Take the account's single foreground connection (P1.5).
    ///
    /// P4.3 serializes worker checkout/connect here: this is the one obvious
    /// place every foreground read acquires a socket.
    async fn lease(&self) -> Result<Lease, SiftError> {
        let mut guard = self.foreground.clone().lock_owned().await;
        let conn = guard.take(&self.pool).await?;
        Ok(Lease {
            guard,
            conn: Some(conn),
        })
    }

    /// Folder map, discovering on the caller's connection when not cached.
    ///
    /// Taking the worker here would deadlock callers that already hold the
    /// lease (a body read resolving its locator holds it), so discovery runs
    /// on the connection the caller is using (P4.3).
    async fn folder_map(&self, conn: &mut Conn) -> Result<FolderMap, SiftError> {
        if let Some(map) = self.folders.lock().await.clone() {
            return Ok(map);
        }
        let map = folders::discover(conn).await?;
        *self.folders.lock().await = Some(map.clone());
        Ok(map)
    }

    async fn refresh_folder_map(&self, conn: &mut Conn) -> Result<FolderMap, SiftError> {
        let map = folders::discover(conn).await?;
        *self.folders.lock().await = Some(map.clone());
        Ok(map)
    }

    /// EXAMINE a folder; a folder that no longer exists is skipped rather
    /// than failing the whole read.
    async fn select_role(&self, conn: &mut Conn, name: &str) -> Result<bool, SiftError> {
        match conn.select(name, true).await {
            Ok(_) => Ok(true),
            Err(e) if is_folder_missing(&e) => Ok(false),
            Err(e) => Err(imap_errors::attachment_from_conn(e)),
        }
    }

    /// Fetch `(UID X-GM-MSGID BODYSTRUCTURE)` for `uid` and require BOTH the
    /// UID and the expected Gmail id to match (P1.4).
    async fn verified_locator_attrs(
        &self,
        conn: &mut Conn,
        uid: u32,
        dec: u64,
        corr: &str,
    ) -> Result<Option<Vec<FetchAttr>>, SiftError> {
        let ctx = imap_errors::OpCtx {
            command: "FETCH",
            stage: "locator-read",
            correlation: corr,
        };
        let items = FetchItems::new().uid().gmail_msgid().bodystructure();
        let rows = match conn.uid_fetch_typed(&uid.to_string(), &items).await {
            Ok(rows) => rows,
            Err(TaggedFailure::Tagged {
                completion,
                code,
                text,
            }) => {
                return Err(imap_errors::attachment_from_tagged(
                    &ctx,
                    completion,
                    code.as_ref(),
                    &text,
                ))
            }
            Err(TaggedFailure::Conn(e)) => return Err(imap_errors::attachment_from_conn(e)),
        };
        for (_seq, attrs) in rows {
            if attr_uid(&attrs) == Some(uid) && attr_msgid(&attrs) == Some(dec) {
                return Ok(Some(attrs));
            }
        }
        Ok(None)
    }

    /// Shared locator resolver for body/attachment/raw fetches (P1.4).
    ///
    /// Order: cached UID verified against the folder's UIDVALIDITY and Gmail
    /// id, then `UID SEARCH X-GM-MSGID <decimal>` across discovered folders
    /// on this same connection. Only a verified mapping is persisted.
    async fn resolve_message_on_connection(
        &self,
        message_id: &str,
        conn: &mut Conn,
    ) -> Result<Locator, SiftError> {
        let corr = self.corr(message_id, "resolve");
        let dec = ids::from_hex(message_id).ok_or_else(|| {
            imap_errors::attachment_locator_invalid(&format!(
                "message-id at resolve (code=- ref {corr})"
            ))
        })?;
        let mut map = self.folder_map(conn).await?;
        if PREFERRED_ROLES
            .iter()
            .any(|r| map.name_for_role(r).is_none())
        {
            if let Ok(fresh) = self.refresh_folder_map(conn).await {
                map = fresh;
            }
        }
        let roles: Vec<(String, String)> = PREFERRED_ROLES
            .iter()
            .filter_map(|r| {
                map.name_for_role(r)
                    .map(|n| ((*r).to_string(), n.to_string()))
            })
            .collect();

        // 1. Cached mapping, verified against epoch + identity.
        let known = self
            .db
            .uids_for_message(&self.account_id, message_id)
            .await
            .map_err(db_err)?;
        for (role, name) in &roles {
            let Some((_, uid)) = known.iter().find(|(r, _)| r == role) else {
                continue;
            };
            let uid = *uid as u32;
            let row = self
                .db
                .imap_get_folder(&self.account_id, role)
                .await
                .map_err(db_err)?;
            if !self.select_role(conn, name).await? {
                continue;
            }
            let epoch = conn.selected().map(|s| s.uidvalidity).unwrap_or(0);
            if let Some(row) = row {
                if row.uidvalidity as u32 != epoch {
                    // The epoch moved: only this folder's UID mappings are
                    // dropped. Bodies, cached files and mail are untouched.
                    log::warn!(
                        target: "sift::imap",
                        "UIDVALIDITY changed for role {role}; invalidating cached UIDs"
                    );
                    self.db
                        .imap_invalidate_epoch(&self.account_id, role, Some(epoch as i64))
                        .await
                        .map_err(db_err)?;
                    continue;
                }
            }
            match self.verified_locator_attrs(conn, uid, dec, &corr).await? {
                Some(attrs) => {
                    log::trace!(
                        target: "sift::imap",
                        "resolved message in role {} (epoch {})",
                        role,
                        epoch
                    );
                    return Ok(Locator {
                        uid,
                        gmail_id: dec,
                        attrs,
                    });
                }
                None => {
                    // Identity mismatch: drop only this locator.
                    let _ = self
                        .db
                        .imap_delete_uids(&self.account_id, role, &[uid as i64])
                        .await;
                }
            }
        }

        // 2. Rediscover by Gmail identity across the discovered folders.
        for (role, name) in &roles {
            if !self.select_role(conn, name).await? {
                continue;
            }
            let found = conn
                .uid_search_gmmsgid(dec)
                .await
                .map_err(imap_errors::attachment_from_conn)?;
            let Some(uid) = found.into_iter().next() else {
                continue;
            };
            if let Some(attrs) = self.verified_locator_attrs(conn, uid, dec, &corr).await? {
                // Persist only the verified result.
                self.db
                    .imap_put_uids(
                        &self.account_id,
                        role,
                        &[(uid as i64, message_id.to_string())],
                    )
                    .await
                    .map_err(db_err)?;
                let epoch = conn.selected().map(|s| s.uidvalidity).unwrap_or(0);
                log::trace!(
                    target: "sift::imap",
                    "rediscovered message in role {} (epoch {})",
                    role,
                    epoch
                );
                return Ok(Locator {
                    uid,
                    gmail_id: dec,
                    attrs,
                });
            }
        }
        Err(imap_errors::attachment_message_missing(&format!(
            "SEARCH at resolve (code=- ref {corr})"
        )))
    }

    /// Full raw MIME for a message on an already-selected-capable connection.
    async fn read_raw(
        &self,
        conn: &mut Conn,
        message_id: &str,
    ) -> Result<(Vec<u8>, u64), SiftError> {
        let corr = self.corr(message_id, "full");
        let loc = self.resolve_message_on_connection(message_id, conn).await?;
        let ctx = imap_errors::OpCtx {
            command: "FETCH",
            stage: "body-read",
            correlation: &corr,
        };
        let rows = match conn
            .uid_fetch_typed(&loc.uid.to_string(), &FetchItems::new().uid().peek_all())
            .await
        {
            Ok(rows) => rows,
            Err(TaggedFailure::Tagged {
                completion,
                code,
                text,
            }) => {
                return Err(imap_errors::attachment_from_tagged(
                    &ctx,
                    completion,
                    code.as_ref(),
                    &text,
                ))
            }
            Err(TaggedFailure::Conn(e)) => return Err(imap_errors::attachment_from_conn(e)),
        };
        for (_seq, attrs) in &rows {
            if attr_uid(attrs) != Some(loc.uid) {
                continue;
            }
            for a in attrs {
                match a {
                    // Only the whole-message sentinel satisfies a raw read: a
                    // numbered part or a header block must never stand in for
                    // the message (P2.7).
                    FetchAttr::WholeMessage { bytes, .. } if !bytes.is_empty() => {
                        return Ok((bytes.clone(), loc.gmail_id));
                    }
                    _ => {}
                }
            }
        }
        Err(imap_errors::attachment_part_missing(&format!(
            "FETCH at body-read (code=- ref {corr})"
        )))
    }

    /// One attachment section, decoded. `section` is already validated.
    async fn read_attachment(
        &self,
        conn: &mut Conn,
        message_id: &str,
        section: ImapSection,
    ) -> Result<Vec<u8>, SiftError> {
        let corr = self.corr(message_id, &section.wire());
        let loc = self.resolve_message_on_connection(message_id, conn).await?;
        // The encoding MUST come from an exact BODYSTRUCTURE match; anything
        // else means the locator does not name a part of this message (P2.4).
        let encoding = encoding_for(&loc.attrs, &section).ok_or_else(|| {
            imap_errors::attachment_part_missing(&format!(
                "BODYSTRUCTURE at section-lookup (code=- ref {corr})"
            ))
        })?;
        let ctx = imap_errors::OpCtx {
            command: "FETCH",
            stage: "section-read",
            correlation: &corr,
        };
        let items = FetchItems::new().uid().peek_section(section);
        let rows = match conn.uid_fetch_typed(&loc.uid.to_string(), &items).await {
            Ok(rows) => rows,
            Err(TaggedFailure::Tagged {
                completion,
                code,
                text,
            }) => {
                return Err(imap_errors::attachment_from_tagged(
                    &ctx,
                    completion,
                    code.as_ref(),
                    &text,
                ))
            }
            Err(TaggedFailure::Conn(e)) => return Err(imap_errors::attachment_from_conn(e)),
        };
        let want = section.wire();
        for (_seq, attrs) in &rows {
            // Rows the server volunteered for a different UID must never
            // supply bytes for this request.
            if attr_uid(attrs) != Some(loc.uid) {
                continue;
            }
            for a in attrs {
                if let FetchAttr::BodySection {
                    section: got,
                    bytes,
                    ..
                } = a
                {
                    if got.eq_ignore_ascii_case(&want) {
                        // Strict: unsupported/malformed transfer encodings are
                        // an error, never the encoded text (P2.4).
                        return message::TransferDecoder::decode_all(&encoding, bytes);
                    }
                }
            }
        }
        Err(imap_errors::attachment_part_missing(&format!(
            "FETCH at section-read (code=- ref {corr})"
        )))
    }

    /// Bounded, leased attachment read (P1.5/P1.6). Consumes `self` so it can
    /// be spawned as a shared task.
    async fn download_attachment(
        self,
        message_id: String,
        section: ImapSection,
    ) -> Result<Vec<u8>, SiftError> {
        let corr = self.corr(&message_id, &section.wire());
        let work = async move {
            let mut lease = self.lease().await?;
            let r = self
                .read_attachment(lease.conn(), &message_id, section)
                .await;
            if r.is_ok() {
                lease.keep();
            }
            r
        };
        match tokio::time::timeout(ATTACHMENT_OP_TIMEOUT, work).await {
            Ok(r) => r,
            Err(_) => Err(imap_errors::attachment_timeout(&format!(
                "FETCH at section-read (code=- ref {corr})"
            ))),
        }
    }

    /// Stream one attachment section as 256 KiB encoded partial reads,
    /// decoding incrementally and forwarding the decoded bytes (P2.4).
    ///
    /// Only one response is ever buffered (requested bytes + framing); the
    /// decoded total is reported exactly, never taken from the BODYSTRUCTURE
    /// octet count. The caller owns any overall transfer deadline.
    async fn stream_attachment(
        &self,
        conn: &mut Conn,
        message_id: &str,
        section: &ImapSection,
        tx: &tokio::sync::mpsc::Sender<crate::provider::AttachmentChunk>,
    ) -> Result<u64, SiftError> {
        use crate::provider::AttachmentChunk;
        /// Encoded bytes requested per partial read (P2.4).
        const CHUNK: u32 = 256 * 1024;
        let corr = self.corr(message_id, &section.wire());
        let loc = self.resolve_message_on_connection(message_id, conn).await?;
        let encoding = encoding_for(&loc.attrs, section).ok_or_else(|| {
            imap_errors::attachment_part_missing(&format!(
                "BODYSTRUCTURE at section-lookup (code=- ref {corr})"
            ))
        })?;
        let mut decoder = message::TransferDecoder::new(&encoding)?;
        let mut total: u64 = 0;
        let mut origin: u64 = 0;
        loop {
            let chunk_origin: u32 = origin.try_into().map_err(|_| {
                imap_errors::attachment_part_missing(&format!(
                    "BODY at origin-overflow (code=- ref {corr})"
                ))
            })?;
            let row = conn
                .uid_fetch_partial(loc.uid, *section, chunk_origin, CHUNK)
                .await
                .map_err(imap_errors::attachment_from_conn)?;
            let (got_origin, bytes) = match row {
                Some(v) => v,
                None => {
                    if origin == 0 {
                        // No bytes at all for a section BODYSTRUCTURE lists.
                        return Err(imap_errors::attachment_part_missing(&format!(
                            "BODY at section-read (code=- ref {corr})"
                        )));
                    }
                    // A server may omit the empty trailing literal.
                    break;
                }
            };
            if got_origin != chunk_origin {
                return Err(imap_errors::attachment_part_missing(&format!(
                    "BODY[{}] at origin-check (code=- ref {corr})",
                    section.wire()
                )));
            }
            if bytes.is_empty() {
                break;
            }
            let short = bytes.len() < CHUNK as usize;
            let decoded = decoder.feed(&bytes)?;
            if !decoded.is_empty() {
                total = total
                    .checked_add(decoded.len() as u64)
                    .ok_or_else(|| imap_errors::attachment_decode_failed("size overflow"))?;
                tx.send(AttachmentChunk::Data(decoded))
                    .await
                    .map_err(|_| imap_errors::attachment_cancelled("stream at chunk-send"))?;
            }
            origin = origin
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| imap_errors::attachment_decode_failed("origin overflow"))?;
            if short {
                break;
            }
        }
        let tail = decoder.finish()?;
        if !tail.is_empty() {
            total = total
                .checked_add(tail.len() as u64)
                .ok_or_else(|| imap_errors::attachment_decode_failed("size overflow"))?;
            tx.send(AttachmentChunk::Data(tail))
                .await
                .map_err(|_| imap_errors::attachment_cancelled("stream at chunk-send"))?;
        }
        tx.send(AttachmentChunk::Done { total_bytes: total })
            .await
            .map_err(|_| imap_errors::attachment_cancelled("stream at done-send"))?;
        Ok(total)
    }
}

pub struct GmailImapProvider {
    account_id: String,
    pool: ImapPool,
    folders: std::sync::Arc<TokioMutex<Option<FolderMap>>>,
    db: Db,
    budget: StdMutex<BudgetState>,
    polls: StdMutex<u64>,
    /// The one bounded foreground connection per account (P1.5).
    foreground: Arc<TokioMutex<Foreground>>,
    /// In-flight attachment downloads keyed by message+part, so a repeated
    /// click joins the running transfer.
    inflight: Arc<StdMutex<HashMap<String, Arc<SharedFetch>>>>,
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
            foreground: Arc::new(TokioMutex::new(Foreground::new())),
            inflight: Arc::new(StdMutex::new(HashMap::new())),
        }
    }

    pub fn account_id(&self) -> &str {
        &self.account_id
    }

    fn ctx(&self) -> FetchCtx {
        FetchCtx {
            account_id: self.account_id.clone(),
            pool: self.pool.clone(),
            folders: self.folders.clone(),
            db: self.db.clone(),
            foreground: self.foreground.clone(),
        }
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

    /// Raw MIME bytes on the account's foreground lease (user-initiated).
    async fn fetch_raw_leased(&self, message_id: &str) -> Result<(Vec<u8>, u64), SiftError> {
        let ctx = self.ctx();
        let mut lease = ctx.lease().await?;
        let r = ctx.read_raw(lease.conn(), message_id).await;
        if r.is_ok() {
            lease.keep();
        }
        r
    }

    /// Backfill is background traffic: it keeps the sync worker so it never
    /// competes with a user-visible foreground read (P1.5).
    async fn fetch_raw_worker(&self, message_id: &str) -> Result<(Vec<u8>, u64), SiftError> {
        let ctx = self.ctx();
        let mut guard = self.pool.worker().await?;
        let conn = guard.as_mut().expect("connected");
        ctx.read_raw(conn, message_id).await
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
                ..Default::default()
            });
        }
        Ok(out)
    }

    /// Rename a user label's folder (P8.5).
    ///
    /// The folder name is the label id for this transport, so the id legitimately
    /// changes with the name. Gmail may implement RENAME as delete+create; either
    /// way the fresh folder map is what the caller reconciles against, and
    /// `X-GM-LABELS` keeps the message membership.
    async fn rename_label(&self, id: &str, name: &str) -> Result<Label, SiftError> {
        if !crate::labels::valid_label_name(name) {
            return Err(SiftError::app(
                "bad_label_name",
                "A label name cannot be empty, cannot contain slashes at the ends, and cannot be longer than 225 characters.",
                false,
            ));
        }
        let from = id.strip_prefix("imap:").unwrap_or(id);
        if from.eq_ignore_ascii_case("INBOX")
            || matches!(
                from,
                "SENT" | "DRAFT" | "STARRED" | "IMPORTANT" | "TRASH" | "SPAM" | "UNREAD"
            )
        {
            return Err(SiftError::app(
                "unsupported_operation",
                "Sift cannot rename a system label.",
                false,
            ));
        }
        if crate::labels::is_system_label(from) {
            return Err(SiftError::app(
                "unsupported_operation",
                "Sift cannot rename a system label.",
                false,
            ));
        }
        let encoded_from = super::proto::encode_utf7_mailbox(from);
        let encoded_to = super::proto::encode_utf7_mailbox(name);
        {
            let mut guard = self.pool.worker().await?;
            let conn = guard.as_mut().expect("connected");
            match conn.rename(&encoded_from, &encoded_to).await {
                Ok(()) => {}
                // A server that cascaded the parent's rename has already moved
                // this folder: the requested end state is the one Sift has, so
                // this is a no-op rather than a failure the user must read.
                Err(SiftError::App { message, .. })
                    if message.to_uppercase().contains("NONEXISTENT")
                        || message.to_uppercase().contains("TRYCREATE") => {}
                Err(e) => return Err(e),
            }
        }
        // The discovered mapping is now stale; refresh it before anything else
        // reads a folder name, so a following operation cannot address the old
        // one.
        {
            let mut guard = self.pool.worker().await?;
            let conn = guard.as_mut().expect("connected");
            if let Ok(map) = folders::discover(conn).await {
                *self.folders.lock().await = Some(map);
            }
        }
        Ok(Label {
            account_id: self.account_id.clone(),
            id: format!("imap:{name}"),
            name: name.into(),
            kind: "user".into(),
            visible: true,
            sort_order: 200,
            ..Default::default()
        })
    }

    /// Delete a user label's folder (P8.5). Gmail keeps the messages; only the
    /// folder (the label) goes away.
    async fn delete_label(&self, id: &str) -> Result<(), SiftError> {
        let name = id.strip_prefix("imap:").unwrap_or(id);
        if name.eq_ignore_ascii_case("INBOX") || crate::labels::is_system_label(name) {
            return Err(SiftError::app(
                "unsupported_operation",
                "Sift cannot delete a system label.",
                false,
            ));
        }
        let encoded = super::proto::encode_utf7_mailbox(name);
        {
            let mut guard = self.pool.worker().await?;
            let conn = guard.as_mut().expect("connected");
            match conn.delete_folder(&encoded).await {
                Ok(()) => {}
                // Already absent is the requested end state.
                Err(SiftError::App { message, .. })
                    if message.to_uppercase().contains("NONEXISTENT")
                        || message.to_uppercase().contains("TRYCREATE") => {}
                Err(e) => return Err(e),
            }
        }
        {
            let mut guard = self.pool.worker().await?;
            let conn = guard.as_mut().expect("connected");
            if let Ok(map) = folders::discover(conn).await {
                *self.folders.lock().await = Some(map);
            }
        }
        Ok(())
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
            ..Default::default()
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
        let (raw, _) = self.fetch_raw_leased(message_id).await?;
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
        let (raw, _) = self.fetch_raw_worker(message_id).await.inspect_err(|e| {
            if matches!(e, SiftError::App { code, .. } if code == "imap_transient" || code == "attachment_offline" || code == "attachment_timeout") {
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
        // P1.1: the locator must be a valid IMAP section path. Reject it
        // BEFORE any connection work so a malformed id never reaches the
        // wire and cannot change error class on retry.
        let section = ImapSection::parse(attachment_id).ok_or_else(|| {
            imap_errors::attachment_locator_invalid(&format!(
                "locator at validate (code=- ref {})",
                imap_errors::correlation_id(&self.account_id, message_id, attachment_id)
            ))
        })?;

        // P1.5: simultaneous requests for the same part join one transfer.
        let key = format!("{message_id}\u{1}{attachment_id}");
        let (shared, leader) = {
            let mut map = self.inflight.lock().unwrap();
            match map.get(&key) {
                Some(s) if !s.finished.load(Ordering::Acquire) => (s.clone(), false),
                _ => {
                    let s = Arc::new(SharedFetch::new());
                    map.insert(key.clone(), s.clone());
                    (s, true)
                }
            }
        };
        if leader {
            let ctx = self.ctx();
            let s = shared.clone();
            let map = self.inflight.clone();
            let k = key.clone();
            let cancel = s.cancel.clone();
            let corr = imap_errors::correlation_id(&self.account_id, message_id, attachment_id);
            let msg = message_id.to_string();
            tokio::spawn(async move {
                let outcome = tokio::select! {
                    r = ctx.download_attachment(msg, section) => match r {
                        Ok(bytes) => SharedOutcome::Ok(Arc::new(bytes)),
                        Err(e) => SharedOutcome::Err(Arc::new(e)),
                    },
                    _ = cancel.cancelled() => SharedOutcome::Err(Arc::new(
                        imap_errors::attachment_cancelled(&format!(
                            "read at section-read (code=- ref {corr})"
                        )),
                    )),
                };
                *s.result.lock().await = Some(outcome);
                s.finished.store(true, Ordering::Release);
                let _ = s.done.send(true);
                map.lock().unwrap().remove(&k);
            });
        }

        // Consumer side: attach to the shared download. Dropping this future
        // (user cancelled the save) only detaches THIS consumer; the work
        // continues while any other consumer still waits.
        let _guard = WaitGuard::new(&shared);
        let mut rx = shared.done.subscribe();
        loop {
            if let Some(out) = shared.result.lock().await.as_ref() {
                return match out {
                    SharedOutcome::Ok(b) => Ok((**b).clone()),
                    SharedOutcome::Err(e) => Err(duplicate_error(e)),
                };
            }
            if *rx.borrow() {
                return match shared.result.lock().await.as_ref() {
                    Some(SharedOutcome::Ok(b)) => Ok((**b).clone()),
                    Some(SharedOutcome::Err(e)) => Err(duplicate_error(e)),
                    None => Err(imap_errors::attachment_offline("join at section-read")),
                };
            }
            if rx.changed().await.is_err() {
                return Err(imap_errors::attachment_offline("join at section-read"));
            }
        }
    }

    /// Real streaming (P2.4): 256 KiB encoded partial `BODY.PEEK[...]` reads
    /// through the bound foreground lease, decoded incrementally into the
    /// consumer's channel. The reported total is the exact decoded length.
    async fn fetch_attachment_chunks(
        &self,
        message_id: &str,
        attachment_id: &str,
        tx: tokio::sync::mpsc::Sender<crate::provider::AttachmentChunk>,
    ) -> Result<u64, SiftError> {
        // Same pre-network validation as `fetch_attachment`: a malformed
        // locator is rejected before any socket work.
        let section = ImapSection::parse(attachment_id).ok_or_else(|| {
            imap_errors::attachment_locator_invalid(&format!(
                "locator at validate (code=- ref {})",
                imap_errors::correlation_id(&self.account_id, message_id, attachment_id)
            ))
        })?;
        let ctx = self.ctx();
        let mut lease = ctx.lease().await?;
        let r = ctx
            .stream_attachment(lease.conn(), message_id, &section, &tx)
            .await;
        // A failed/unknown-framing stream must not be reused (P1.3).
        if r.is_ok() {
            lease.keep();
        }
        r
    }

    async fn fetch_raw(&self, message_id: &str) -> Result<String, SiftError> {
        let (raw, _) = self.fetch_raw_leased(message_id).await?;
        Ok(String::from_utf8_lossy(&raw).into_owned())
    }

    /// Byte-exact raw MIME (P9.3): the FETCH body is handed back exactly as it
    /// arrived. The text-shaped `fetch_raw` above is for display only, where a
    /// lossy decode can be labelled as such.
    async fn fetch_raw_bytes(&self, message_id: &str) -> Result<Vec<u8>, SiftError> {
        let (raw, _) = self.fetch_raw_leased(message_id).await?;
        Ok(raw)
    }

    async fn apply(&self, op: &OutboxOp) -> Result<ApplyOutcome, SiftError> {
        let folders = self.folders_cached().await?;
        match op.kind.as_str() {
            "modify_labels" => {
                let ids: Vec<String> =
                    serde_json::from_value(op.payload["ids"].clone()).unwrap_or_default();
                let add: Vec<String> =
                    serde_json::from_value(op.payload["add"].clone()).unwrap_or_default();
                let remove: Vec<String> =
                    serde_json::from_value(op.payload["remove"].clone()).unwrap_or_default();
                super::ops::apply_modify_labels(
                    &self.pool,
                    &self.db,
                    &folders,
                    &self.account_id,
                    &ids,
                    &add,
                    &remove,
                )
                .await
            }
            "trash" => {
                let threads: Vec<String> =
                    serde_json::from_value(op.payload["threads"].clone()).unwrap_or_default();
                // Trash via MOVE (same rows as modify TRASH, thread-grouped).
                super::ops::apply_trash_threads(
                    &self.pool,
                    &self.db,
                    &folders,
                    &self.account_id,
                    &threads,
                )
                .await
            }
            "untrash" => {
                let threads: Vec<String> =
                    serde_json::from_value(op.payload["threads"].clone()).unwrap_or_default();
                // Untrash: resolve each thread's messages back to \All + INBOX.
                for tid in &threads {
                    let mids: Vec<String> = self
                        .db
                        .read({
                            let (a, t) = (self.account_id.clone(), tid.clone());
                            move |c| {
                                Ok(c.prepare(
                                    "SELECT id FROM messages WHERE account_id=? AND thread_id=?",
                                )?
                                .query_map(rusqlite::params![a, t], |r| r.get(0))?
                                .collect::<Result<Vec<String>, _>>()?)
                            }
                        })
                        .await
                        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
                    if !mids.is_empty() {
                        let _ = super::ops::apply_modify_labels(
                            &self.pool,
                            &self.db,
                            &folders,
                            &self.account_id,
                            &mids,
                            &["INBOX".to_string()],
                            &["TRASH".to_string()],
                        )
                        .await?;
                    }
                }
                Ok(ApplyOutcome::Done)
            }
            "delete" => {
                let targets = super::ops::delete_targets_from_payload(
                    &self.db,
                    &self.account_id,
                    &op.payload,
                )
                .await?;
                super::ops::apply_delete_messages(
                    &self.pool,
                    &self.db,
                    &folders,
                    &self.account_id,
                    &targets,
                )
                .await
            }
            // A rule application is the same provider work as a gesture.
            "rule_apply" => {
                let ids: Vec<String> =
                    serde_json::from_value(op.payload["ids"].clone()).unwrap_or_default();
                let add: Vec<String> =
                    serde_json::from_value(op.payload["add"].clone()).unwrap_or_default();
                let remove: Vec<String> =
                    serde_json::from_value(op.payload["remove"].clone()).unwrap_or_default();
                if ids.is_empty() {
                    return Ok(ApplyOutcome::AlreadyApplied);
                }
                super::ops::apply_modify_labels(
                    &self.pool,
                    &self.db,
                    &folders,
                    &self.account_id,
                    &ids,
                    &add,
                    &remove,
                )
                .await
            }
            "label_rename" => {
                let id = op.payload["id"].as_str().unwrap_or_default();
                let name = op.payload["name"].as_str().unwrap_or_default();
                if id.is_empty() || name.is_empty() {
                    return Err(SiftError::app(
                        "op",
                        "a label rename needs a label and a name",
                        false,
                    ));
                }
                self.rename_label(id, name).await?;
                Ok(ApplyOutcome::Done)
            }
            "label_delete" => {
                let id = op.payload["id"].as_str().unwrap_or_default();
                if id.is_empty() {
                    return Err(SiftError::app("op", "a label delete needs a label", false));
                }
                self.delete_label(id).await?;
                Ok(ApplyOutcome::Done)
            }
            "send" => {
                // Executed by the outbox drain, which owns the prepared
                // payload and the retry policy.
                return Err(SiftError::app(
                    "op",
                    "send ops are applied by the outbox drain",
                    false,
                ));
            }
            "draft_sync" => {
                return Err(SiftError::app(
                    "op",
                    "draft_sync ops are applied by the outbox drain",
                    false,
                ));
            }
            "draft_upsert" | "draft_delete" => {
                // Superseded by `draft_sync` (P5.2). Ops left in the queue by
                // an older build complete as no-ops.
                Ok(ApplyOutcome::Done)
            }
            _ => Err(SiftError::app(
                "op",
                format!("unknown op {}", op.kind),
                false,
            )),
        }
    }

    async fn send(&self, req: &crate::provider::SendRequest) -> Result<SentInfo, SiftError> {
        let email = self.pool.email();
        let pw = self.pool.app_password();
        // The envelope is the prepared one; SMTP DATA gets the message without
        // its Bcc header (the envelope carries those blind recipients).
        super::smtp::send_gmail(&email, &pw, req).await?;
        // On success, run a partial tick so the Sent/All copy appears locally
        // with its real id within seconds (spec task 10/11).
        let folders = self.folders_cached().await?;
        let mut prev = std::collections::HashMap::new();
        for role in ["all", "trash", "junk"] {
            if let Ok(Some(c)) = self.db.imap_get_folder(&self.account_id, role).await {
                prev.insert(role.to_string(), c);
            }
        }
        let sink = crate::provider::DbSink::new(self.db.clone());
        let cancel = tokio_util::sync::CancellationToken::new();
        let _ = super::partial::run_partial_sync(
            &self.pool,
            &sink,
            &self.account_id,
            &folders,
            &prev,
            false,
            &cancel,
        )
        .await;
        // SMTP yields no id; callers use the synced Sent copy. Return the
        // Message-ID based placeholder (hex of nothing → empty) - outbox
        // treats send as Done regardless.
        Ok(SentInfo {
            id: String::new(),
            thread_id: String::new(),
        })
    }

    /// Reconcile an uncertain send against the server's own index (P6.1).
    ///
    /// IMAP cannot answer "did SMTP accept it?", but the Sent copy can: the
    /// prepared message carries a stable Message-ID, and Gmail indexes it.
    async fn sent_by_rfc_message_id(
        &self,
        rfc_message_id: &str,
    ) -> Result<Option<SentInfo>, SiftError> {
        let folders = self.folders_cached().await?;
        let Some((id, _)) =
            super::ops::sent_by_rfc_message_id(&self.pool, &folders, rfc_message_id).await?
        else {
            return Ok(None);
        };
        let thread_id = if id.is_empty() {
            String::new()
        } else {
            self.db
                .message_thread(&crate::dto::MessageRef::new(
                    self.account_id.clone(),
                    id.clone(),
                ))
                .await
                .unwrap_or(None)
                .unwrap_or_default()
        };
        Ok(Some(SentInfo { id, thread_id }))
    }

    async fn draft_upsert(
        &self,
        remote_id: Option<&str>,
        raw: &[u8],
        rfc_message_id: &str,
    ) -> Result<crate::dto::RemoteDraft, SiftError> {
        let folders = self.folders_cached().await?;
        super::ops::draft_upsert(
            &self.pool,
            &self.db,
            &folders,
            &self.account_id,
            remote_id,
            raw,
            rfc_message_id,
        )
        .await
    }

    async fn draft_delete(&self, remote_id: &str) -> Result<(), SiftError> {
        let folders = self.folders_cached().await?;
        super::ops::draft_delete(&self.pool, &self.db, &folders, &self.account_id, remote_id).await
    }

    /// Drafts the Drafts mailbox holds, read locally (P5.2).
    ///
    /// The folder is part of normal sync, so listing drafts costs no network
    /// round trip: one query joins the uid map to the synced message rows for
    /// the identity and the RFC Message-ID that reconciliation matches on.
    async fn draft_list(&self) -> Result<Vec<crate::dto::RemoteDraft>, SiftError> {
        let uidvalidity = self
            .db
            .imap_get_folder(&self.account_id, "drafts")
            .await
            .ok()
            .flatten()
            .map(|c| c.uidvalidity)
            .unwrap_or_default();
        let account = self.account_id.clone();
        let rows: Vec<(i64, String, Option<String>)> = self
            .db
            .read(move |c| {
                Ok(c.prepare(
                    "SELECT u.uid, u.message_id, m.rfc_message_id FROM imap_uids u \
                     LEFT JOIN messages m ON m.account_id=u.account_id AND m.id=u.message_id \
                     WHERE u.account_id=? AND u.role='drafts' ORDER BY u.uid",
                )?
                .query_map(rusqlite::params![account], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })?
                .collect::<Result<Vec<_>, _>>()?)
            })
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        Ok(rows
            .into_iter()
            .map(|(uid, hex, rfc)| crate::dto::RemoteDraft {
                remote_draft_id: super::ops::encode_draft_locator(
                    uidvalidity as u32,
                    uid as u32,
                    &hex,
                ),
                message_id: Some(hex),
                thread_id: None,
                rfc_message_id: rfc,
            })
            .collect())
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
        // The raw search, and every metadata FETCH that follows it, run in
        // \All: the lease is re-taken per chunk so the mailbox is always the
        // one this search was issued against (P4.3).
        let uids = {
            let mut w = self
                .pool
                .with_selected_worker(name, true, &no_cancel_token())
                .await?;
            w.conn().uid_search_raw(q).await?
        };
        let mut out = vec![];
        let mut count = 0u32;
        for chunk in uids.chunks(50) {
            if count >= limit {
                break;
            }
            let items = {
                let mut w = self
                    .pool
                    .with_selected_worker(name, true, &no_cancel_token())
                    .await?;
                message::fetch_meta(w.conn(), chunk).await?
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
        ..Default::default()
    })
    .collect()
}
