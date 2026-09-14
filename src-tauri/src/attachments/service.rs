//! Attachment lifecycle service (P2.1–P2.4, P2.6).
//!
//! One place decides, for a given attachment row: which local file satisfies a
//! read, whether the network is needed, which transport locator to ask for,
//! where the bytes land on disk, how progress and cancellation are surfaced,
//! and what filename the user sees.
//!
//! Identity rules (P2.1): the row id, the stored MIME section (`part_id`), a
//! Content-ID and the REST attachment id are four different things. A
//! UI-supplied key only *selects a row*; the transport locator always comes
//! from the row itself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::db::Db;
use crate::dto::{
    AttachmentCacheInfo, AttachmentProgress, AttachmentRecord, AttachmentRefKey, CacheState,
    SaveAllResult,
};
use crate::errors::SiftError;
use crate::provider::{AttachmentChunk, ProviderKind};

use super::{cache, naming, quarantine};

/// Queue/locator discovery: time allowed before the first byte arrives.
pub const DISCOVERY_DEADLINE: Duration = Duration::from_secs(60);
/// Connection inactivity: time allowed between chunks once bytes are flowing.
/// (P2.4 lists 30s for the connection-inactivity timer; this wave standardised
/// the user-visible budget on 60s — keep the single constant here.)
pub const INACTIVITY_DEADLINE: Duration = Duration::from_secs(60);
/// Explicit overall transfer deadline.
pub const TRANSFER_DEADLINE: Duration = Duration::from_secs(600);
/// Progress is emitted at most this often (10/s).
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
/// Payloads larger than this are never stored in the SQLite row cache; they
/// live only as cache files.
pub const ROW_CACHE_CAP: usize = 64 * 1024;
/// Inline fetches up to this size are written back to the row cache.
pub const INLINE_ROW_CACHE_CAP: usize = 1024 * 1024;

pub type ProgressSink = Arc<dyn Fn(&AttachmentProgress) + Send + Sync>;

/// Everything the service needs that is not per-call.
#[derive(Clone)]
pub struct AttachmentRuntime {
    pub db: Db,
    pub data_dir: PathBuf,
    emit: Option<ProgressSink>,
}

impl AttachmentRuntime {
    pub fn new(db: Db, data_dir: PathBuf) -> Self {
        Self {
            db,
            data_dir,
            emit: None,
        }
    }

    pub fn with_progress(db: Db, data_dir: PathBuf, sink: ProgressSink) -> Self {
        Self {
            db,
            data_dir,
            emit: Some(sink),
        }
    }

    pub fn progress(&self, p: &AttachmentProgress) {
        if let Some(sink) = &self.emit {
            sink(p);
        }
    }
}

/// What the service needs from a transport.
#[async_trait]
pub trait AttachmentTransport: Send + Sync {
    fn kind(&self) -> ProviderKind;
    /// Single-shot fetch, used by the inline/URI byte path.
    async fn fetch_bytes(&self, message_id: &str, locator: &str) -> Result<Vec<u8>, SiftError>;
    /// Incremental fetch. `Data` chunks arrive in order, then `Done`; the
    /// returned value is the exact decoded total.
    async fn fetch_chunks(
        &self,
        message_id: &str,
        locator: &str,
        tx: mpsc::Sender<AttachmentChunk>,
    ) -> Result<u64, SiftError>;
}

/// Resolves the transport for an account. Consulted only on a cache miss, so
/// a cached attachment opens with no provider work at all.
#[async_trait]
pub trait TransportSource: Send + Sync {
    async fn transport(&self, account_id: &str) -> Result<Arc<dyn AttachmentTransport>, SiftError>;
}

/// The real providers, adapted to [`AttachmentTransport`].
pub struct ProviderTransport(pub Arc<dyn crate::provider::Provider>);

#[async_trait]
impl AttachmentTransport for ProviderTransport {
    fn kind(&self) -> ProviderKind {
        self.0.kind()
    }
    async fn fetch_bytes(&self, message_id: &str, locator: &str) -> Result<Vec<u8>, SiftError> {
        self.0.fetch_attachment(message_id, locator).await
    }
    async fn fetch_chunks(
        &self,
        message_id: &str,
        locator: &str,
        tx: mpsc::Sender<AttachmentChunk>,
    ) -> Result<u64, SiftError> {
        self.0
            .fetch_attachment_chunks(message_id, locator, tx)
            .await
    }
}

#[async_trait]
impl TransportSource for crate::app_state::AppState {
    async fn transport(&self, account_id: &str) -> Result<Arc<dyn AttachmentTransport>, SiftError> {
        let provider = crate::app_state::AppState::provider_for(self, account_id).await?;
        Ok(Arc::new(ProviderTransport(provider)))
    }
}

// ---------------------------------------------------------------- cancellation

static CANCELS: LazyLock<Mutex<HashMap<String, CancellationToken>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn cancels() -> &'static Mutex<HashMap<String, CancellationToken>> {
    &CANCELS
}

fn cancel_key(account_id: &str, attachment_id: &str) -> String {
    format!("{account_id}\u{1}{attachment_id}")
}

fn register(account_id: &str, attachment_id: &str) -> (String, CancellationToken) {
    let key = cancel_key(account_id, attachment_id);
    let token = CancellationToken::new();
    if let Ok(mut map) = cancels().lock() {
        map.insert(key.clone(), token.clone());
    }
    (key, token)
}

fn unregister(key: &str) {
    if let Ok(mut map) = cancels().lock() {
        map.remove(key);
    }
}

/// Cancel an in-flight transfer by its account-qualified attachment id (the
/// `requestId` the progress event reports). Immediate: the transfer future is
/// aborted and its temp file removed.
pub fn cancel(account_id: &str, request_id: &str) -> bool {
    let map = match cancels().lock() {
        Ok(m) => m,
        Err(_) => return false,
    };
    match map.get(&cancel_key(account_id, request_id)) {
        Some(token) => {
            token.cancel();
            true
        }
        None => false,
    }
}

// -------------------------------------------------------------- error helpers

fn write_failed(e: impl std::fmt::Display) -> SiftError {
    SiftError::app("attachment_write_failed", e.to_string(), false)
}

fn locator_invalid(msg: impl Into<String>) -> SiftError {
    SiftError::app("attachment_locator_invalid", msg, false)
}

fn decode_failed(msg: impl Into<String>) -> SiftError {
    SiftError::app("attachment_decode_failed", msg, true)
}

fn timeout_error(progressing: bool) -> SiftError {
    let msg = if progressing {
        "This attachment stopped transferring. Check your connection and try again."
    } else {
        "This attachment is taking too long to download. Check your connection and try again."
    };
    SiftError::app("attachment_timeout", msg, true)
}

fn cancelled() -> SiftError {
    SiftError::app("attachment_cancelled", "Download cancelled", false)
}

fn confirmation_required(name: &str) -> SiftError {
    SiftError::app(
        "attachment_confirmation_required",
        format!(
            "“{name}” is an executable download. Confirm before opening it, or save it instead."
        ),
        false,
    )
}

fn db_error(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

fn error_code(e: &SiftError) -> Option<&str> {
    match e {
        SiftError::App { code, .. } => Some(code.as_str()),
        _ => None,
    }
}

// ------------------------------------------------------------------- lookups

/// Load a row and check the caller's account owns it.
///
/// `key` is the account-qualified key from appendix A; the row is looked up by
/// both halves, so a stale key from another account is a plain lookup failure —
/// never a cross-account fallback.
pub async fn owned_record(db: &Db, key: &AttachmentRefKey) -> Result<AttachmentRecord, SiftError> {
    db.attachment_get(key)
        .await
        .map_err(db_error)?
        .ok_or_else(|| SiftError::NotFound("attachment".into()))
}

/// The account-qualified key of a row, for the update paths that address a
/// cache row rather than a message.
pub fn rec_key(rec: &AttachmentRecord) -> AttachmentRefKey {
    AttachmentRefKey {
        account_id: rec.account_id.clone(),
        attachment_id: rec.id.clone(),
    }
}

fn info(
    rec: &AttachmentRecord,
    basename: &str,
    state: CacheState,
    path: Option<&Path>,
    decoded_size: Option<i64>,
) -> AttachmentCacheInfo {
    AttachmentCacheInfo {
        account_id: rec.account_id.clone(),
        attachment_id: rec.id.clone(),
        state: state.as_str().to_string(),
        path: path.map(|p| p.to_string_lossy().into_owned()),
        display_name: rec.filename.clone(),
        cache_basename: basename.to_string(),
        mime: rec.mime.clone(),
        size: rec.size,
        decoded_size,
        requires_confirmation: naming::is_executable_like(basename),
    }
}

fn progress(
    rec: &AttachmentRecord,
    request_id: &str,
    state: &str,
    transferred: u64,
    total: Option<u64>,
    error: Option<&str>,
) -> AttachmentProgress {
    AttachmentProgress {
        account_id: rec.account_id.clone(),
        attachment_id: rec.id.clone(),
        request_id: request_id.to_string(),
        state: state.to_string(),
        transferred_bytes: transferred,
        total_bytes: total,
        error_code: error.map(str::to_string),
    }
}

// --------------------------------------------------------------- ensure local

/// Guarantee a verified local file for `rec`, downloading if necessary.
/// Returns typed cache metadata; callers read bytes only when they must.
pub async fn ensure_local(
    rt: &AttachmentRuntime,
    source: &dyn TransportSource,
    rec: &AttachmentRecord,
) -> Result<AttachmentCacheInfo, SiftError> {
    let basename = naming::basename(rec.filename.as_deref(), &rec.mime, &rec.id);
    let key = rec_key(rec);
    // The file this call is about to hand out must not be evicted while it is
    // being read (P10.4). The lease is released when this future is dropped.
    let _lease = crate::attachments::in_use::acquire(&rec.account_id, &rec.id);

    // 1. A cache file already satisfies the read. Legacy rows are promoted
    //    from `unverified` to `ready` once the file checks out.
    if rec.cache_state.is_local() {
        if let Some(path) = rec.local_path.as_deref().filter(|p| !p.is_empty()) {
            let path = Path::new(path);
            if let Ok(meta) = tokio::fs::metadata(path).await {
                let size_ok = rec
                    .decoded_size
                    .map(|d| d.max(0) as u64 == meta.len())
                    .unwrap_or(true);
                if meta.is_file() && size_ok {
                    if rec.cache_state == CacheState::Unverified {
                        let _ = rt
                            .db
                            .attachment_set_ready(&key, &path.to_string_lossy(), meta.len())
                            .await;
                        quarantine::apply(path, None);
                    } else {
                        let _ = rt.db.attachment_touch(&key).await;
                    }
                    return Ok(info(
                        rec,
                        &basename,
                        CacheState::Ready,
                        Some(path),
                        Some(meta.len() as i64),
                    ));
                }
            }
        }
    }

    // 2. Row-cached bytes. A decode failure is recoverable: mark the payload
    //    corrupt, then refetch. An empty decoded payload is a valid file.
    if let Some(err) = rec.data_error.as_deref() {
        log::warn!(
            "attachment {} payload unreadable ({err}); refetching",
            rec.id
        );
        let _ = rt.db.attachment_set_state(&key, CacheState::Corrupt).await;
    } else if let Some(data) = rec.data.as_deref() {
        let dir =
            cache::attachment_dir(&rt.data_dir, &rec.account_id, &rec.id).map_err(write_failed)?;
        let path = cache::write_atomic(&dir, &basename, data)
            .await
            .map_err(write_failed)?;
        rt.db
            .attachment_set_ready(&key, &path.to_string_lossy(), data.len() as u64)
            .await
            .map_err(db_error)?;
        quarantine::apply(&path, None);
        return Ok(info(
            rec,
            &basename,
            CacheState::Ready,
            Some(&path),
            Some(data.len() as i64),
        ));
    }

    // 3. Network. The locator comes from the row's own identity, chosen for
    //    the transport that owns the account.
    let transport = source.transport(&rec.account_id).await?;
    let locator = rec
        .locator_for(transport.kind())
        .ok_or_else(|| {
            locator_invalid(format!(
                "attachment {} has no {} locator",
                rec.id,
                match transport.kind() {
                    ProviderKind::GmailApi => "REST attachment",
                    ProviderKind::GmailImap => "IMAP section",
                }
            ))
        })?
        .to_string();
    download(rt, transport, rec, &locator, &basename).await
}

async fn download(
    rt: &AttachmentRuntime,
    transport: Arc<dyn AttachmentTransport>,
    rec: &AttachmentRecord,
    locator: &str,
    basename: &str,
) -> Result<AttachmentCacheInfo, SiftError> {
    let dir =
        cache::attachment_dir(&rt.data_dir, &rec.account_id, &rec.id).map_err(write_failed)?;
    let key = rec_key(rec);
    let request_id = rec.id.clone();
    let (key_reg, token) = register(&rec.account_id, &rec.id);
    let _ = rt
        .db
        .attachment_set_state(&key, CacheState::Downloading)
        .await;
    rt.progress(&progress(rec, &request_id, "downloading", 0, None, None));

    let result = stream(
        rt,
        transport,
        rec,
        locator,
        &dir,
        basename,
        &token,
        &request_id,
    )
    .await;
    unregister(&key_reg);

    match result {
        Ok((path, size)) => {
            rt.db
                .attachment_set_ready(&key, &path.to_string_lossy(), size)
                .await
                .map_err(db_error)?;
            quarantine::apply(&path, None);
            rt.progress(&progress(rec, &request_id, "ready", size, Some(size), None));
            Ok(info(
                rec,
                basename,
                CacheState::Ready,
                Some(&path),
                Some(size as i64),
            ))
        }
        Err(e) => {
            // A failed transfer leaves `downloading` behind; clear it so the
            // row never claims a file it does not have.
            let _ = rt.db.attachment_set_state(&key, CacheState::Missing).await;
            let code = error_code(&e);
            let state = if code == Some("attachment_cancelled") {
                "cancelled"
            } else {
                "failed"
            };
            rt.progress(&progress(rec, &request_id, state, 0, None, code));
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn stream(
    rt: &AttachmentRuntime,
    transport: Arc<dyn AttachmentTransport>,
    rec: &AttachmentRecord,
    locator: &str,
    dir: &Path,
    basename: &str,
    token: &CancellationToken,
    request_id: &str,
) -> Result<(PathBuf, u64), SiftError> {
    let final_path = dir.join(basename);
    let mut temp = cache::TempFile::create(dir, basename)
        .await
        .map_err(write_failed)?;
    let (tx, mut rx) = mpsc::channel::<AttachmentChunk>(4);
    let (done_tx, mut done_rx) = oneshot::channel::<Result<u64, SiftError>>();
    let (message_id, loc) = (rec.message_id.clone(), locator.to_string());
    let producer = tokio::spawn(async move {
        let result = transport.fetch_chunks(&message_id, &loc, tx).await;
        let _ = done_tx.send(result);
    });

    let overall_at = Instant::now() + TRANSFER_DEADLINE;
    let mut transferred: u64 = 0;
    let mut total: Option<u64> = None;
    let mut saw_first = false;
    let mut senders_closed = false;
    let mut last_emit: Option<Instant> = None;
    let mut producer_result: Option<Result<u64, SiftError>> = None;

    'outer: loop {
        let idle_at = Instant::now()
            + if saw_first {
                INACTIVITY_DEADLINE
            } else {
                DISCOVERY_DEADLINE
            };
        tokio::select! {
            _ = token.cancelled() => {
                producer.abort();
                return Err(cancelled());
            }
            // Always-on overall bound: it also covers a producer that closes
            // its sender and then stalls.
            _ = tokio::time::sleep_until(overall_at) => {
                producer.abort();
                return Err(timeout_error(saw_first));
            }
            r = &mut done_rx, if producer_result.is_none() => {
                producer_result = Some(match r {
                    Ok(inner) => inner,
                    Err(_) => Err(decode_failed("attachment transfer task failed")),
                });
            }
            r = tokio::time::timeout_at(idle_at.min(overall_at), rx.recv()), if producer_result.is_none() && !senders_closed => {
                match r {
                    Err(_) => {
                        producer.abort();
                        return Err(timeout_error(saw_first));
                    }
                    Ok(None) => senders_closed = true,
                    Ok(Some(chunk)) => {
                        saw_first = true;
                        consume(
                            &mut temp, chunk, &mut transferred, &mut total, &mut last_emit,
                            rt, rec, request_id,
                        ).await?;
                        if total.is_some() {
                            break 'outer;
                        }
                    }
                }
            }
        }

        if let Some(result) = producer_result.take() {
            // The producer finished: drain anything still buffered, in order.
            while let Ok(chunk) = rx.try_recv() {
                consume(
                    &mut temp,
                    chunk,
                    &mut transferred,
                    &mut total,
                    &mut last_emit,
                    rt,
                    rec,
                    request_id,
                )
                .await?;
                if total.is_some() {
                    break 'outer;
                }
            }
            return match result {
                Err(e) => Err(e),
                // Completion without a marker (or with the marker drained but
                // not seen) is a protocol violation, not a silent success.
                Ok(_) => Err(decode_failed("transfer ended without a completion marker")),
            };
        }
    }

    let expected =
        total.ok_or_else(|| decode_failed("transfer ended without a completion marker"))?;
    if expected != transferred {
        return Err(decode_failed(format!(
            "attachment is incomplete: expected {expected} bytes, received {transferred}"
        )));
    }
    temp.publish(&final_path).await.map_err(write_failed)?;
    Ok((final_path, transferred))
}

#[allow(clippy::too_many_arguments)]
async fn consume(
    temp: &mut cache::TempFile,
    chunk: AttachmentChunk,
    transferred: &mut u64,
    total: &mut Option<u64>,
    last_emit: &mut Option<Instant>,
    rt: &AttachmentRuntime,
    rec: &AttachmentRecord,
    request_id: &str,
) -> Result<(), SiftError> {
    match chunk {
        AttachmentChunk::Data(bytes) => {
            temp.write_all(&bytes).await.map_err(write_failed)?;
            *transferred = (*transferred)
                .checked_add(bytes.len() as u64)
                .ok_or_else(|| decode_failed("attachment size overflow"))?;
            let due = last_emit
                .map(|t| t.elapsed() >= PROGRESS_INTERVAL)
                .unwrap_or(true);
            if due {
                *last_emit = Some(Instant::now());
                rt.progress(&progress(
                    rec,
                    request_id,
                    "downloading",
                    *transferred,
                    *total,
                    None,
                ));
            }
        }
        AttachmentChunk::Done { total_bytes } => *total = Some(total_bytes),
    }
    Ok(())
}

// -------------------------------------------------------------- byte helpers

/// Bad-key guard for URI input (defence in depth; the key is only ever used to
/// select a row).
fn bad_key(s: &str) -> bool {
    s.is_empty() || s.contains("..") || s.contains('/') || s.contains('\\')
}

/// Bytes for the `sift-att://` scheme and the inline-image path. The incoming
/// key only selects a row; the network locator is the row's own.
pub async fn resolve_bytes(
    db: &Db,
    provider: &dyn crate::provider::Provider,
    message: &crate::dto::MessageRef,
    key: &str,
) -> Result<(Vec<u8>, String), SiftError> {
    if bad_key(&message.account_id) || bad_key(&message.message_id) || bad_key(key) {
        return Err(locator_invalid("invalid attachment id"));
    }
    let rec = db
        .attachment_resolve(message, key)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
        .ok_or_else(|| SiftError::NotFound("attachment".into()))?;
    // A file being read for a preview is in use: eviction skips it (P10.4).
    let _lease = crate::attachments::in_use::acquire(&rec.account_id, &rec.id);
    if let Some(data) = rec.data.as_ref() {
        return Ok((data.clone(), rec.mime));
    }
    if let Some(path) = rec.local_path.as_deref().filter(|p| !p.is_empty()) {
        if let Ok(bytes) = tokio::fs::read(path).await {
            return Ok((bytes, rec.mime));
        }
    }
    let locator = rec.locator_for(provider.kind()).ok_or_else(|| {
        locator_invalid(format!("attachment {} has no transport locator", rec.id))
    })?;
    let bytes = provider.fetch_attachment(&rec.message_id, locator).await?;
    if bytes.len() <= INLINE_ROW_CACHE_CAP {
        let db = db.clone();
        let (message, key, payload) = (message.clone(), rec.id.clone(), bytes.clone());
        // Cache repair is best-effort; the bytes are already in hand.
        let _ = db.attachment_cache_data(&message, &key, &payload).await;
    }
    Ok((bytes, rec.mime))
}

// ------------------------------------------------------------------- saving

/// Copy a verified cache file to a user-chosen destination, publishing
/// atomically so a failure never leaves a half-file there.
pub async fn copy_to(
    rt: &AttachmentRuntime,
    source: &dyn TransportSource,
    rec: &AttachmentRecord,
    dest: &Path,
) -> Result<(), SiftError> {
    let info = ensure_local(rt, source, rec).await?;
    let src = PathBuf::from(info.path.ok_or_else(|| write_failed("no cached file"))?);
    cache::copy_atomic(&src, dest).await.map_err(write_failed)?;
    // Preserve the download marking on the user's copy.
    quarantine::apply(dest, quarantine::read(&src).as_deref());
    Ok(())
}

/// `name (2).ext` collision handling (P2.6).
pub fn unique_destination(dir: &Path, name: &str) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) if !s.is_empty() && !e.is_empty() && e.len() <= 16 => (s, format!(".{e}")),
        _ => (name, String::new()),
    };
    for n in 2..10_000u32 {
        let candidate = dir.join(format!("{stem} ({n}){ext}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    dir.join(name)
}

/// Save every non-inline attachment of a message into one folder (P2.6).
/// The folder prompt itself belongs to the command; this is the queue.
pub async fn save_all_into(
    rt: &AttachmentRuntime,
    source: &dyn TransportSource,
    message: &crate::dto::MessageRef,
    dir: &Path,
) -> Result<SaveAllResult, SiftError> {
    let mut recs = rt.db.attachments_records(message).await.map_err(db_error)?;
    let owner = message.account_id.clone();
    recs.retain(|r| !r.is_inline && r.account_id == owner);
    if recs.len() < 2 {
        return Err(SiftError::app(
            "attachment_save_all_not_applicable",
            "Save All needs at least two attachments",
            false,
        ));
    }
    recs.sort_by_key(AttachmentRecord::order_key);
    tokio::fs::create_dir_all(dir).await.map_err(write_failed)?;

    let mut saved = 0usize;
    let mut failed = Vec::new();
    for rec in &recs {
        let name = naming::basename(rec.filename.as_deref(), &rec.mime, &rec.id);
        let dest = unique_destination(dir, &name);
        match copy_to(rt, source, rec, &dest).await {
            Ok(()) => saved += 1,
            Err(e) => {
                log::warn!("save all: {} failed: {e}", rec.id);
                failed.push(AttachmentRefKey {
                    account_id: rec.account_id.clone(),
                    attachment_id: rec.id.clone(),
                });
            }
        }
    }
    Ok(SaveAllResult { saved, failed })
}

// --------------------------------------------------------------------- open

/// Open policy + system opener call (P2.3). The confirmation result is
/// enforced here, in the backend: a downloaded executable type is never handed
/// to the OS opener without it.
pub async fn open(
    rt: &AttachmentRuntime,
    source: &dyn TransportSource,
    rec: &AttachmentRecord,
    confirmed_executable: bool,
    open_with_system: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), SiftError> {
    let info = ensure_local(rt, source, rec).await?;
    if info.requires_confirmation && !confirmed_executable {
        return Err(confirmation_required(&info.cache_basename));
    }
    let path = PathBuf::from(info.path.ok_or_else(|| write_failed("no cached file"))?);
    open_with_system(&path).map_err(|e| SiftError::app("open", e, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed(db: &Db, mid: &str) -> String {
        let a = db.new_account("a@x.com", None, None).await.unwrap();
        let aid = a.id.clone();
        let mid = mid.to_string();
        db.write(move |c| {
            c.execute(
                "INSERT INTO messages (id,account_id,thread_id,internal_date) VALUES (?1,?2,'t1',1)",
                rusqlite::params![mid, aid],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        a.id
    }

    #[test]
    fn unique_destination_avoids_collisions() {
        let dir = tempfile::tempdir().unwrap();
        let one = unique_destination(dir.path(), "invoice.pdf");
        assert_eq!(one.file_name().unwrap(), "invoice.pdf");
        std::fs::write(&one, b"x").unwrap();
        let two = unique_destination(dir.path(), "invoice.pdf");
        assert_eq!(two.file_name().unwrap(), "invoice (2).pdf");
        std::fs::write(&two, b"x").unwrap();
        let three = unique_destination(dir.path(), "invoice.pdf");
        assert_eq!(three.file_name().unwrap(), "invoice (3).pdf");
        // nameless fallbacks collide the same way
        let a = unique_destination(dir.path(), "notes");
        std::fs::write(&a, b"x").unwrap();
        assert_eq!(
            unique_destination(dir.path(), "notes").file_name().unwrap(),
            "notes (2)"
        );
    }

    #[tokio::test]
    async fn ownership_is_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let aid = seed(&db, "m1").await;
        db.attachments_put(crate::db::attachments::AttPut {
            id: "att1".into(),
            account_id: aid.clone(),
            message_id: "m1".into(),
            gmail_att_id: Some("rest1".into()),
            part_id: "1.2".into(),
            filename: Some("invoice.pdf".into()),
            mime: "application/pdf".into(),
            size: 4,
            content_id: None,
            is_inline: false,
            data: Some(b"%PDF".to_vec()),
        })
        .await
        .unwrap();
        let key = AttachmentRefKey {
            account_id: aid.clone(),
            attachment_id: "att1".into(),
        };
        assert!(owned_record(&db, &key).await.is_ok());
        // A key from another account never resolves, even for a real row id.
        let foreign = AttachmentRefKey {
            account_id: "other".into(),
            attachment_id: "att1".into(),
        };
        assert!(matches!(
            owned_record(&db, &foreign).await,
            Err(SiftError::NotFound(_))
        ));
        let missing = AttachmentRefKey {
            account_id: aid.clone(),
            attachment_id: "missing".into(),
        };
        assert!(owned_record(&db, &missing).await.is_err());
    }
}
