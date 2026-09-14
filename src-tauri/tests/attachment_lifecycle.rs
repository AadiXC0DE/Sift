//! Attachment lifecycle acceptance (P2.1–P2.4, P2.6).
//!
//! These exercise the real service, real SQLite (temp dir, real migrations) and
//! real filesystem, with a scripted transport standing in for IMAP/REST. Every
//! transport call is recorded, so "which locator did we actually ask for, for
//! which account" is asserted rather than assumed.

use async_trait::async_trait;
use sift::attachments::service::{self, AttachmentRuntime, AttachmentTransport, TransportSource};
use sift::db::{attachments::AttPut, Db};
use sift::dto::{AttachmentProgress, CacheState};
use sift::errors::SiftError;
use sift::provider::{AttachmentChunk, ProviderKind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

// ---------------------------------------------------------------- fake transport

struct FakeTransport {
    kind: ProviderKind,
    payload: Vec<u8>,
    chunk_size: usize,
    calls: Mutex<Vec<(String, String)>>,
    /// Fail (retryably) once this many chunks have been sent.
    fail_after: Option<usize>,
    /// Report this total in `Done` instead of the real one.
    done_override: Option<u64>,
    /// Hold the transfer open after the first chunk until notified.
    gate: Option<Arc<tokio::sync::Notify>>,
    sent: AtomicUsize,
}

impl FakeTransport {
    fn new(kind: ProviderKind, payload: impl Into<Vec<u8>>) -> Arc<Self> {
        Arc::new(Self {
            kind,
            payload: payload.into(),
            chunk_size: usize::MAX,
            calls: Mutex::new(Vec::new()),
            fail_after: None,
            done_override: None,
            gate: None,
            sent: AtomicUsize::new(0),
        })
    }

    fn with_chunk_size(self: &Arc<Self>, size: usize) -> Arc<Self> {
        Arc::new(Self {
            kind: self.kind,
            payload: self.payload.clone(),
            chunk_size: size,
            calls: Mutex::new(self.calls.lock().unwrap().clone()),
            fail_after: self.fail_after,
            done_override: self.done_override,
            gate: self.gate.clone(),
            sent: AtomicUsize::new(0),
        })
    }

    fn with_failure(self: &Arc<Self>, after_chunks: usize) -> Arc<Self> {
        Arc::new(Self {
            kind: self.kind,
            payload: self.payload.clone(),
            chunk_size: self.chunk_size,
            calls: Mutex::new(Vec::new()),
            fail_after: Some(after_chunks),
            done_override: None,
            gate: None,
            sent: AtomicUsize::new(0),
        })
    }

    fn with_done_override(self: &Arc<Self>, total: u64) -> Arc<Self> {
        Arc::new(Self {
            kind: self.kind,
            payload: self.payload.clone(),
            chunk_size: self.chunk_size,
            calls: Mutex::new(Vec::new()),
            fail_after: None,
            done_override: Some(total),
            gate: None,
            sent: AtomicUsize::new(0),
        })
    }

    fn with_gate(self: &Arc<Self>, gate: Arc<tokio::sync::Notify>) -> Arc<Self> {
        Arc::new(Self {
            kind: self.kind,
            payload: self.payload.clone(),
            chunk_size: self.chunk_size,
            calls: Mutex::new(Vec::new()),
            fail_after: None,
            done_override: None,
            gate: Some(gate),
            sent: AtomicUsize::new(0),
        })
    }

    fn recorded(&self) -> Vec<(String, String)> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl AttachmentTransport for FakeTransport {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    async fn fetch_bytes(&self, message_id: &str, locator: &str) -> Result<Vec<u8>, SiftError> {
        self.calls
            .lock()
            .unwrap()
            .push((message_id.to_string(), locator.to_string()));
        Ok(self.payload.clone())
    }

    async fn fetch_chunks(
        &self,
        message_id: &str,
        locator: &str,
        tx: tokio::sync::mpsc::Sender<AttachmentChunk>,
    ) -> Result<u64, SiftError> {
        self.calls
            .lock()
            .unwrap()
            .push((message_id.to_string(), locator.to_string()));
        let total = self.payload.len() as u64;
        let size = if self.chunk_size == 0 {
            usize::MAX
        } else {
            self.chunk_size
        };
        let mut sent = 0usize;
        for chunk in self.payload.chunks(size).map(<[u8]>::to_vec) {
            if self.fail_after.is_some_and(|n| sent >= n) {
                return Err(SiftError::app(
                    "attachment_offline",
                    "simulated transfer failure",
                    true,
                ));
            }
            sent += 1;
            self.sent.store(sent, Ordering::SeqCst);
            tx.send(AttachmentChunk::Data(chunk))
                .await
                .map_err(|_| SiftError::app("attachment_cancelled", "cancelled", false))?;
        }
        if let Some(gate) = &self.gate {
            gate.notified().await;
        }
        tx.send(AttachmentChunk::Done {
            total_bytes: self.done_override.unwrap_or(total),
        })
        .await
        .map_err(|_| SiftError::app("attachment_cancelled", "cancelled", false))?;
        Ok(total)
    }
}

struct FakeSource {
    transport: Arc<FakeTransport>,
    accounts: Mutex<Vec<String>>,
}

impl FakeSource {
    fn new(transport: Arc<FakeTransport>) -> Self {
        Self {
            transport,
            accounts: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl TransportSource for FakeSource {
    async fn transport(&self, account_id: &str) -> Result<Arc<dyn AttachmentTransport>, SiftError> {
        self.accounts.lock().unwrap().push(account_id.to_string());
        Ok(self.transport.clone())
    }
}

/// A source that must never be consulted (cache-hit assertions).
struct NoTransport;

#[async_trait]
impl TransportSource for NoTransport {
    async fn transport(&self, account_id: &str) -> Result<Arc<dyn AttachmentTransport>, SiftError> {
        panic!("transport consulted for {account_id}; the cache should have answered");
    }
}

// ---------------------------------------------------------------------- harness

struct Fixture {
    _dir: tempfile::TempDir,
    db: Db,
    account: String,
    runtime: AttachmentRuntime,
    progress: Arc<Mutex<Vec<AttachmentProgress>>>,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let account = db.new_account("a@x.com", None, None).await.unwrap().id;
        let progress = Arc::new(Mutex::new(Vec::new()));
        let sink: service::ProgressSink = {
            let progress = progress.clone();
            Arc::new(move |p: &AttachmentProgress| progress.lock().unwrap().push(p.clone()))
        };
        let runtime = AttachmentRuntime::with_progress(db.clone(), dir.path().to_path_buf(), sink);
        Self {
            _dir: dir,
            db,
            account,
            runtime,
            progress,
        }
    }

    async fn message(&self, id: &str) {
        let account = self.account.clone();
        let id = id.to_string();
        self.db
            .write(move |c| {
                c.execute(
                    "INSERT INTO messages (id,account_id,thread_id,internal_date) VALUES (?1,?2,'t1',1)",
                    rusqlite::params![id, account],
                )?;
                Ok(())
            })
            .await
            .unwrap();
    }

    async fn attach(
        &self,
        id: &str,
        message: &str,
        part: &str,
        filename: Option<&str>,
        mime: &str,
        data: Option<Vec<u8>>,
    ) {
        let size = data.as_ref().map(|d| d.len() as i64).unwrap_or(0);
        self.db
            .attachments_put(AttPut {
                id: id.into(),
                account_id: self.account.clone(),
                message_id: message.into(),
                gmail_att_id: None,
                part_id: part.into(),
                filename: filename.map(str::to_string),
                mime: mime.into(),
                size,
                content_id: None,
                is_inline: false,
                data,
            })
            .await
            .unwrap();
    }

    async fn record(&self, id: &str) -> sift::dto::AttachmentRecord {
        self.db
            .attachment_get(&sift::dto::AttachmentRefKey {
                account_id: self.account.clone(),
                attachment_id: id.into(),
            })
            .await
            .unwrap()
            .unwrap()
    }

    fn progress_states(&self) -> Vec<(String, String)> {
        self.progress
            .lock()
            .unwrap()
            .iter()
            .map(|p| (p.state.clone(), p.attachment_id.clone()))
            .collect()
    }
}

fn error_code(e: &SiftError) -> String {
    serde_json::to_value(e).unwrap()["code"]
        .as_str()
        .unwrap()
        .to_string()
}

// ------------------------------------------------------------------------- P2.2

/// Metadata → full body → reopen: one attachment, enriched, with bytes on disk.
#[tokio::test]
async fn metadata_then_body_then_reopen_keeps_one_attachment_with_bytes() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "att1",
        "m1",
        "1.2",
        Some("invoice.pdf"),
        "application/pdf",
        None,
    )
    .await;
    fx.attach(
        "att1",
        "m1",
        "1.2",
        None,
        "application/pdf",
        Some(b"%PDF-1.4 body".to_vec()),
    )
    .await;

    let rec = fx.record("att1").await;
    assert_eq!(rec.id, "att1", "row id survives enrichment");
    assert_eq!(rec.filename.as_deref(), Some("invoice.pdf"));
    assert_eq!(rec.data.as_deref(), Some(&b"%PDF-1.4 body"[..]));
    assert_eq!(
        fx.db
            .attachments_records(&sift::dto::MessageRef::new(fx.account.clone(), "m1"))
            .await
            .unwrap()
            .len(),
        1
    );

    let info = service::ensure_local(&fx.runtime, &NoTransport, &rec)
        .await
        .unwrap();
    assert_eq!(info.state, "ready");
    assert_eq!(info.cache_basename, "invoice.pdf");
    assert!(!info.requires_confirmation);
    let path = PathBuf::from(info.path.unwrap());
    assert_eq!(std::fs::read(&path).unwrap(), b"%PDF-1.4 body");

    // Reopen: the verified file answers, and the row now records it.
    let rec = fx.record("att1").await;
    assert_eq!(rec.cache_state, CacheState::Ready);
    assert_eq!(
        rec.local_path.as_deref(),
        Some(path.to_string_lossy().as_ref())
    );
    let again = service::ensure_local(&fx.runtime, &NoTransport, &rec)
        .await
        .unwrap();
    assert_eq!(again.path.unwrap(), path.to_string_lossy());
}

#[tokio::test]
async fn repeated_ingest_preserves_id_and_local_path() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "att1",
        "m1",
        "2",
        Some("photo.png"),
        "image/png",
        Some(vec![7, 7]),
    )
    .await;
    let rec = fx.record("att1").await;
    let first = service::ensure_local(&fx.runtime, &NoTransport, &rec)
        .await
        .unwrap();
    let first_path = first.path.unwrap();

    // A later metadata-only parse of the same message must not reset the row.
    fx.attach("att1", "m1", "2", None, "image/png", None).await;
    let rec = fx.record("att1").await;
    assert_eq!(rec.id, "att1");
    assert_eq!(rec.local_path.as_deref(), Some(first_path.as_str()));
    assert_eq!(rec.filename.as_deref(), Some("photo.png"));
    let again = service::ensure_local(&fx.runtime, &NoTransport, &rec)
        .await
        .unwrap();
    assert_eq!(again.path.unwrap(), first_path);
}

/// A corrupt payload is recoverable: the row is marked, and the refetch
/// replaces it. It is never reported as a successful empty file.
#[tokio::test]
async fn corrupt_payload_refetches_and_never_yields_empty_file() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "att1",
        "m1",
        "3",
        Some("data.bin"),
        "application/octet-stream",
        Some(vec![1]),
    )
    .await;
    fx.db
        .write(|c| {
            c.execute(
                "UPDATE attachments SET data_z=?1 WHERE id='att1'",
                rusqlite::params![b"garbage".to_vec()],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    let rec = fx.record("att1").await;
    assert!(
        rec.data.is_none(),
        "corrupt bytes are never an empty payload"
    );
    assert!(rec.data_error.is_some());

    let transport = FakeTransport::new(ProviderKind::GmailImap, b"real bytes".to_vec());
    let sink = FakeSource::new(transport.clone());
    let info = service::ensure_local(&fx.runtime, &sink, &rec)
        .await
        .unwrap();
    assert_eq!(info.state, "ready");
    assert_eq!(std::fs::read(info.path.unwrap()).unwrap(), b"real bytes");
    assert_eq!(
        transport.recorded(),
        vec![("m1".to_string(), "3".to_string())]
    );
    assert_eq!(fx.record("att1").await.cache_state, CacheState::Ready);
}

/// Cache repair for a metadata change must not touch the network.
#[tokio::test]
async fn metadata_refresh_alone_does_not_redownload() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "att1",
        "m1",
        "2",
        Some("invoice.pdf"),
        "application/pdf",
        Some(vec![1, 2, 3]),
    )
    .await;
    let rec = fx.record("att1").await;
    service::ensure_local(&fx.runtime, &NoTransport, &rec)
        .await
        .unwrap();

    // Filename metadata arrives later; `NoTransport` fails the test if the
    // service decides to refetch.
    fx.attach("att1", "m1", "2", None, "application/pdf", None)
        .await;
    let rec = fx.record("att1").await;
    let info = service::ensure_local(&fx.runtime, &NoTransport, &rec)
        .await
        .unwrap();
    assert_eq!(info.state, "ready");
}

/// A `ready` row whose file was deleted is repaired: from the row cache when
/// bytes are still stored, otherwise from the network. Never left `ready` with
/// nothing on disk.
#[tokio::test]
async fn deleted_cache_file_is_repaired() {
    let fx = Fixture::new().await;
    fx.message("m1").await;

    // Row-cached bytes: the row alone repairs the file, no network.
    fx.attach(
        "att1",
        "m1",
        "2",
        Some("cached.bin"),
        "application/octet-stream",
        Some(vec![1, 2, 3]),
    )
    .await;
    let rec = fx.record("att1").await;
    let info = service::ensure_local(&fx.runtime, &NoTransport, &rec)
        .await
        .unwrap();
    std::fs::remove_file(info.path.unwrap()).unwrap();
    let rec = fx.record("att1").await;
    assert_eq!(rec.cache_state, CacheState::Ready);
    let repaired = service::ensure_local(&fx.runtime, &NoTransport, &rec)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(repaired.path.unwrap()).unwrap(),
        vec![1, 2, 3]
    );

    // No row cache: the file must come back over the network.
    fx.attach(
        "att2",
        "m1",
        "3",
        Some("gone.bin"),
        "application/octet-stream",
        None,
    )
    .await;
    let transport = FakeTransport::new(ProviderKind::GmailImap, b"fresh".to_vec());
    let sink = FakeSource::new(transport.clone());
    let rec = fx.record("att2").await;
    let first = service::ensure_local(&fx.runtime, &sink, &rec)
        .await
        .unwrap();
    std::fs::remove_file(first.path.unwrap()).unwrap();

    let rec = fx.record("att2").await;
    assert_eq!(
        rec.cache_state,
        CacheState::Ready,
        "row still claims a file"
    );
    let again = service::ensure_local(&fx.runtime, &sink, &rec)
        .await
        .unwrap();
    assert_eq!(std::fs::read(again.path.unwrap()).unwrap(), b"fresh");
    assert_eq!(transport.recorded().len(), 2, "refetched after deletion");
}

/// A successful decode of zero bytes is a valid, ready, empty file.
#[tokio::test]
async fn empty_attachment_is_ready_with_zero_bytes() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "att1",
        "m1",
        "2",
        Some("empty.txt"),
        "text/plain",
        Some(Vec::new()),
    )
    .await;
    let rec = fx.record("att1").await;
    let info = service::ensure_local(&fx.runtime, &NoTransport, &rec)
        .await
        .unwrap();
    assert_eq!(info.state, "ready");
    assert_eq!(info.decoded_size, Some(0));
    let path = PathBuf::from(info.path.unwrap());
    assert!(path.exists());
    assert_eq!(std::fs::metadata(&path).unwrap().len(), 0);

    // A streamed zero-byte attachment behaves the same way.
    fx.attach("att2", "m1", "3", Some("empty2.txt"), "text/plain", None)
        .await;
    let rec = fx.record("att2").await;
    let transport = FakeTransport::new(ProviderKind::GmailImap, Vec::new());
    let sink = FakeSource::new(transport);
    let info = service::ensure_local(&fx.runtime, &sink, &rec)
        .await
        .unwrap();
    assert_eq!(info.state, "ready");
    assert_eq!(std::fs::metadata(info.path.unwrap()).unwrap().len(), 0);
}

// ------------------------------------------------------------------------- P2.3

#[tokio::test]
async fn filename_matrix_covers_unicode_quotes_traversal_and_missing_names() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    let cases: Vec<(&str, Option<&str>, &str, &str)> = vec![
        ("a1", Some("invoice.pdf"), "application/pdf", "invoice.pdf"),
        (
            "a2",
            Some("Résumé final.pdf"),
            "application/pdf",
            "Résumé final.pdf",
        ),
        (
            "a3",
            Some("\"report\" 2026.pdf"),
            "application/pdf",
            "\"report\" 2026.pdf",
        ),
        ("a4", None, "application/pdf", "attachment-a4.pdf"),
        ("a5", Some("../../x"), "application/octet-stream", "x"),
        (
            "a6",
            Some("/etc/passwd"),
            "application/octet-stream",
            "passwd",
        ),
        ("a7", Some("   "), "application/zip", "attachment-a7.zip"),
    ];
    for (id, name, mime, expected) in &cases {
        fx.attach(id, "m1", id, *name, mime, Some(vec![1])).await;
        let rec = fx.record(id).await;
        let info = service::ensure_local(&fx.runtime, &NoTransport, &rec)
            .await
            .unwrap();
        assert_eq!(&info.cache_basename, expected, "basename for {name:?}");
        let path = PathBuf::from(info.path.unwrap());
        assert_eq!(path.file_name().unwrap().to_string_lossy(), *expected);
        assert_eq!(
            info.display_name.as_deref(),
            *name,
            "the original display name is kept verbatim"
        );
        assert!(path.starts_with(fx.runtime.data_dir.join("attachments")));
    }
}

#[tokio::test]
async fn duplicate_names_are_distinct_per_attachment() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "att1",
        "m1",
        "2",
        Some("invoice.pdf"),
        "application/pdf",
        Some(vec![1]),
    )
    .await;
    fx.attach(
        "att2",
        "m1",
        "3",
        Some("invoice.pdf"),
        "application/pdf",
        Some(vec![2]),
    )
    .await;
    let mut paths = Vec::new();
    for id in ["att1", "att2"] {
        let rec = fx.record(id).await;
        let info = service::ensure_local(&fx.runtime, &NoTransport, &rec)
            .await
            .unwrap();
        paths.push(PathBuf::from(info.path.unwrap()));
    }
    assert_ne!(paths[0], paths[1], "each attachment owns its own directory");
    assert_eq!(paths[0].file_name().unwrap(), "invoice.pdf");
    assert_eq!(paths[1].file_name().unwrap(), "invoice.pdf");
}

/// A write failure leaves neither a `ready` row nor a half-file at the final
/// path — the temp file is removed too.
#[tokio::test]
async fn failed_write_leaves_no_ready_state_and_no_half_file() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "att1",
        "m1",
        "2",
        Some("big.bin"),
        "application/octet-stream",
        None,
    )
    .await;
    let rec = fx.record("att1").await;
    let transport = FakeTransport::new(ProviderKind::GmailImap, vec![9u8; 64])
        .with_chunk_size(8)
        .with_failure(2);
    let sink = FakeSource::new(transport);
    let err = service::ensure_local(&fx.runtime, &sink, &rec)
        .await
        .unwrap_err();
    assert_eq!(error_code(&err), "attachment_offline");

    let rec = fx.record("att1").await;
    assert_ne!(rec.cache_state, CacheState::Ready, "no false ready");
    assert!(rec.local_path.is_none());
    let dir = fx
        .runtime
        .data_dir
        .join("attachments")
        .join(&fx.account)
        .join("att1");
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .map(|rd| rd.filter_map(|e| e.ok()).collect::<Vec<_>>())
        .unwrap_or_default();
    assert!(
        leftovers.is_empty(),
        "cache dir holds no half-file or temp file"
    );

    // The failure is reported to the UI, ending in a terminal state.
    let states = fx.progress_states();
    assert!(states.iter().any(|(s, _)| s == "failed"), "{states:?}");
}

/// A transport that reports a total the bytes do not add up to is a protocol
/// failure, not a short-but-successful file.
#[tokio::test]
async fn completion_mismatch_is_an_error_not_a_short_file() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "att1",
        "m1",
        "2",
        Some("short.bin"),
        "application/octet-stream",
        None,
    )
    .await;
    let rec = fx.record("att1").await;
    let transport = FakeTransport::new(ProviderKind::GmailImap, vec![1u8; 16])
        .with_chunk_size(4)
        .with_done_override(999);
    let sink = FakeSource::new(transport);
    let err = service::ensure_local(&fx.runtime, &sink, &rec)
        .await
        .unwrap_err();
    assert_eq!(error_code(&err), "attachment_decode_failed");
    assert_ne!(fx.record("att1").await.cache_state, CacheState::Ready);
}

/// Cancellation is immediate: the transfer aborts, the temp file goes away and
/// the row is never left `downloading` or `ready`.
#[tokio::test]
async fn save_cancel_aborts_immediately_and_cleans_up() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "att1",
        "m1",
        "2",
        Some("slow.bin"),
        "application/octet-stream",
        None,
    )
    .await;
    let gate = Arc::new(tokio::sync::Notify::new());
    let transport = FakeTransport::new(ProviderKind::GmailImap, vec![3u8; 32])
        .with_chunk_size(4)
        .with_gate(gate.clone());
    let sink: Arc<dyn TransportSource> = Arc::new(FakeSource::new(transport.clone()));
    let rec = fx.record("att1").await;
    let rt = fx.runtime.clone();

    let task = tokio::spawn(async move { service::ensure_local(&rt, &*sink, &rec).await });
    // Wait until the first chunk has been written by the producer.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while transport.sent.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert!(
        transport.sent.load(Ordering::SeqCst) > 0,
        "transfer never started"
    );
    assert!(service::cancel(&fx.account, "att1"), "registered transfer");

    let err = task.await.unwrap().unwrap_err();
    assert_eq!(error_code(&err), "attachment_cancelled");
    let rec = fx.record("att1").await;
    assert_eq!(
        rec.cache_state,
        CacheState::Missing,
        "not left downloading/ready"
    );
    assert!(rec.local_path.is_none());
    assert!(fx.progress_states().iter().any(|(s, _)| s == "cancelled"));
    let dir = fx
        .runtime
        .data_dir
        .join("attachments")
        .join(&fx.account)
        .join("att1");
    let leftovers = std::fs::read_dir(&dir).map(|rd| rd.count()).unwrap_or(0);
    assert_eq!(leftovers, 0, "temp file removed on cancel");
}

#[tokio::test]
async fn read_only_destination_fails_without_partial_file() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "att1",
        "m1",
        "2",
        Some("invoice.pdf"),
        "application/pdf",
        Some(vec![1, 2, 3]),
    )
    .await;
    let rec = fx.record("att1").await;

    let dir = tempfile::tempdir().unwrap();
    let read_only = dir.path().join("ro");
    std::fs::create_dir(&read_only).unwrap();
    let mut perms = std::fs::metadata(&read_only).unwrap().permissions();
    {
        use std::os::unix::fs::PermissionsExt;
        perms.set_mode(0o555);
    }
    std::fs::set_permissions(&read_only, perms).unwrap();

    let dest = read_only.join("invoice.pdf");
    let err = service::copy_to(&fx.runtime, &NoTransport, &rec, &dest)
        .await
        .unwrap_err();
    assert_eq!(error_code(&err), "attachment_write_failed");
    assert!(!dest.exists(), "nothing written to a read-only destination");
    assert_eq!(std::fs::read_dir(&read_only).unwrap().count(), 0);
}

#[cfg(target_os = "macos")]
struct TinyVolume {
    mount: PathBuf,
    device: String,
}

#[cfg(target_os = "macos")]
impl TinyVolume {
    /// A ~1 MB HFS+ volume: the only portable way to get a real ENOSPC here.
    fn create(workdir: &Path) -> Self {
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        let name = format!(
            "SiftTiny{}{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::SeqCst)
        );
        let dmg = workdir.join(format!("{name}.dmg"));
        let status = std::process::Command::new("/usr/bin/hdiutil")
            .args([
                "create", "-size", "1m", "-fs", "HFS+", "-volname", &name, "-quiet",
            ])
            .arg(&dmg)
            .status()
            .expect("hdiutil create");
        assert!(status.success(), "could not create the test disk image");
        let out = std::process::Command::new("/usr/bin/hdiutil")
            .args(["attach", "-nobrowse"])
            .arg(&dmg)
            .output()
            .expect("hdiutil attach");
        let text = String::from_utf8_lossy(&out.stdout);
        let device = text
            .lines()
            .find(|l| l.contains("/Volumes/"))
            .and_then(|l| l.split_whitespace().next())
            .expect("attached device")
            .to_string();
        let mount = PathBuf::from(format!("/Volumes/{name}"));
        assert!(mount.exists(), "volume mounted at {}", mount.display());
        Self { mount, device }
    }
}

#[cfg(target_os = "macos")]
impl Drop for TinyVolume {
    fn drop(&mut self) {
        let _ = std::process::Command::new("/usr/bin/hdiutil")
            .args(["detach", "-force", &self.device])
            .status();
    }
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn disk_full_fails_without_ready_or_half_file() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    let payload = vec![0x41u8; 3 * 1024 * 1024];
    fx.attach(
        "att1",
        "m1",
        "2",
        Some("big.pdf"),
        "application/pdf",
        Some(payload),
    )
    .await;
    let rec = fx.record("att1").await;

    let work = tempfile::tempdir().unwrap();
    let volume = TinyVolume::create(work.path());
    let dest = volume.mount.join("big.pdf");
    let err = service::copy_to(&fx.runtime, &NoTransport, &rec, &dest)
        .await
        .unwrap_err();
    assert_eq!(error_code(&err), "attachment_write_failed");
    assert!(!dest.exists(), "no half-file at the destination");
    let leftovers: Vec<String> = std::fs::read_dir(&volume.mount)
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(
        !leftovers
            .iter()
            .any(|n| n.contains("big") || n.contains(".part-")),
        "temp copy removed after ENOSPC: {leftovers:?}"
    );
}

// ------------------------------------------------------------------------- P2.1

/// The same CID on two messages resolves per message, and never becomes a
/// section locator.
#[tokio::test]
async fn same_cid_on_two_messages_resolves_to_each_rows_section() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.message("m2").await;
    for (msg, id, part) in [("m1", "a1", "2"), ("m2", "a2", "3.1")] {
        fx.attach(id, msg, part, Some("logo.png"), "image/png", None)
            .await;
        fx.db
            .write({
                let id = id.to_string();
                move |c| {
                    c.execute(
                        "UPDATE attachments SET content_id='<logo@example.test>', is_inline=1 WHERE id=?1",
                        rusqlite::params![id],
                    )?;
                    Ok(())
                }
            })
            .await
            .unwrap();
    }
    let transport = FakeTransport::new(ProviderKind::GmailImap, vec![1u8; 4]);
    let sink = FakeSource::new(transport.clone());
    for (msg, id, part) in [("m1", "a1", "2"), ("m2", "a2", "3.1")] {
        // Resolve through the CID so the lookup key is the CID, not the section.
        let rec = fx
            .db
            .attachment_resolve(
                &sift::dto::MessageRef::new(fx.account.clone(), msg),
                "logo@example.test",
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rec.id, id);
        assert_eq!(rec.part_id, part);
        let info = service::ensure_local(&fx.runtime, &sink, &rec)
            .await
            .unwrap();
        assert_eq!(std::fs::read(info.path.unwrap()).unwrap(), vec![1u8; 4]);
    }
    let mut calls = transport.recorded();
    calls.sort();
    assert_eq!(
        calls,
        vec![
            ("m1".to_string(), "2".to_string()),
            ("m2".to_string(), "3.1".to_string())
        ],
        "each message fetched its own stored section"
    );
    assert_eq!(
        sink.accounts.lock().unwrap().clone(),
        vec![fx.account.clone(), fx.account.clone()],
        "each fetch resolved the owning account"
    );
}

/// Row id, dotted part, and API attachment id each select the row; the locator
/// handed to the transport depends only on the transport.
#[tokio::test]
async fn lookup_by_row_id_part_and_api_id_records_the_right_locator() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "att1",
        "m1",
        "1.2",
        Some("doc.pdf"),
        "application/pdf",
        None,
    )
    .await;
    fx.db
        .write(|c| {
            c.execute(
                "UPDATE attachments SET gmail_att_id='API-att-9' WHERE id='att1'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();

    for key in ["att1", "1.2"] {
        let rec = fx
            .db
            .attachment_resolve(&sift::dto::MessageRef::new(fx.account.clone(), "m1"), key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rec.part_id, "1.2");
        assert_eq!(rec.imap_locator(), Some("1.2"));
        assert_eq!(rec.rest_locator(), Some("API-att-9"));
    }

    // IMAP transport asks for the section.
    let imap = FakeTransport::new(ProviderKind::GmailImap, b"imap".to_vec());
    let imap_sink = FakeSource::new(imap.clone());
    let rec = fx.record("att1").await;
    service::ensure_local(&fx.runtime, &imap_sink, &rec)
        .await
        .unwrap();
    assert_eq!(imap.recorded(), vec![("m1".to_string(), "1.2".to_string())]);
    assert_eq!(
        imap_sink.accounts.lock().unwrap().clone(),
        vec![fx.account.clone()]
    );

    // REST transport asks for the API id, for the same row.
    let fx2 = Fixture::new().await;
    fx2.message("m1").await;
    fx2.attach(
        "att1",
        "m1",
        "1.2",
        Some("doc.pdf"),
        "application/pdf",
        None,
    )
    .await;
    fx2.db
        .write(|c| {
            c.execute(
                "UPDATE attachments SET gmail_att_id='API-att-9' WHERE id='att1'",
                [],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    let rest = FakeTransport::new(ProviderKind::GmailApi, b"rest".to_vec());
    let rest_sink = FakeSource::new(rest.clone());
    let rec = fx2.record("att1").await;
    service::ensure_local(&fx2.runtime, &rest_sink, &rec)
        .await
        .unwrap();
    assert_eq!(
        rest.recorded(),
        vec![("m1".to_string(), "API-att-9".to_string())]
    );
    assert_eq!(
        rest_sink.accounts.lock().unwrap().clone(),
        vec![fx2.account.clone()]
    );
}

/// A row without a locator for the account's transport is a typed error before
/// any network work, not a fabricated request.
#[tokio::test]
async fn missing_locator_for_the_transport_is_a_typed_error() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach("att1", "m1", "2", Some("doc.pdf"), "application/pdf", None)
        .await;
    let rest = FakeTransport::new(ProviderKind::GmailApi, b"rest".to_vec());
    let rec = fx.record("att1").await;
    let err = service::ensure_local(&fx.runtime, &FakeSource::new(rest.clone()), &rec)
        .await
        .unwrap_err();
    assert_eq!(error_code(&err), "attachment_locator_invalid");
    assert!(rest.recorded().is_empty(), "no request was attempted");
    assert_eq!(fx.record("att1").await.cache_state, CacheState::Missing);
}

// ------------------------------------------------------------------------- P2.4

/// Progress is throttled (never per chunk), monotone, and carries no bytes.
#[tokio::test]
async fn progress_events_are_throttled_and_never_carry_bytes() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "att1",
        "m1",
        "2",
        Some("big.bin"),
        "application/octet-stream",
        None,
    )
    .await;
    let payload = vec![7u8; 200 * 64];
    let transport =
        FakeTransport::new(ProviderKind::GmailImap, payload.clone()).with_chunk_size(64);
    let rec = fx.record("att1").await;
    let info = service::ensure_local(&fx.runtime, &FakeSource::new(transport), &rec)
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(info.path.unwrap()).unwrap().len(),
        payload.len()
    );

    let events = fx.progress.lock().unwrap().clone();
    let downloading: Vec<_> = events.iter().filter(|p| p.state == "downloading").collect();
    assert!(
        downloading.len() <= 10,
        "200 chunks must not produce 200 events: {}",
        downloading.len()
    );
    assert!(downloading
        .windows(2)
        .all(|w| w[0].transferred_bytes <= w[1].transferred_bytes));
    let last = events.last().unwrap();
    assert_eq!(last.state, "ready");
    assert_eq!(last.transferred_bytes, payload.len() as u64);
    assert_eq!(last.total_bytes, Some(payload.len() as u64));
    assert_eq!(last.request_id, "att1");
    for p in &events {
        assert_eq!(p.account_id, fx.account);
    }
}

// ------------------------------------------------------------------------- P2.6

#[tokio::test]
async fn save_all_writes_every_non_inline_attachment_and_avoids_collisions() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "a1",
        "m1",
        "2",
        Some("invoice.pdf"),
        "application/pdf",
        Some(vec![1]),
    )
    .await;
    fx.attach(
        "a2",
        "m1",
        "3",
        Some("notes.txt"),
        "text/plain",
        Some(vec![2]),
    )
    .await;
    // Unnamed attachments are not filtered out.
    fx.attach("a3", "m1", "4", None, "application/zip", Some(vec![3]))
        .await;
    // Inline parts are not part of Save All.
    fx.attach(
        "a4",
        "m1",
        "5",
        Some("logo.png"),
        "image/png",
        Some(vec![4]),
    )
    .await;
    fx.db
        .write(|c| {
            c.execute("UPDATE attachments SET is_inline=1 WHERE id='a4'", [])?;
            Ok(())
        })
        .await
        .unwrap();

    let dir = fx.runtime.data_dir.join("saved");
    let first = service::save_all_into(
        &fx.runtime,
        &NoTransport,
        &sift::dto::MessageRef::new(fx.account.clone(), "m1"),
        &dir,
    )
    .await
    .unwrap();
    assert_eq!(first.saved, 3);
    assert!(first.failed.is_empty());
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, vec!["attachment-a3.zip", "invoice.pdf", "notes.txt"]);
    assert_eq!(std::fs::read(dir.join("invoice.pdf")).unwrap(), vec![1]);
    assert!(!dir.join("logo.png").exists());

    // A second Save All never clobbers what is already there.
    let second = service::save_all_into(
        &fx.runtime,
        &NoTransport,
        &sift::dto::MessageRef::new(fx.account.clone(), "m1"),
        &dir,
    )
    .await
    .unwrap();
    assert_eq!(second.saved, 3);
    assert!(dir.join("invoice (2).pdf").exists());
    assert_eq!(std::fs::read(dir.join("invoice.pdf")).unwrap(), vec![1]);
}

#[tokio::test]
async fn save_all_needs_two_non_inline_attachments() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "a1",
        "m1",
        "2",
        Some("only.pdf"),
        "application/pdf",
        Some(vec![1]),
    )
    .await;
    let dir = fx.runtime.data_dir.join("saved");
    let err = service::save_all_into(
        &fx.runtime,
        &NoTransport,
        &sift::dto::MessageRef::new(fx.account.clone(), "m1"),
        &dir,
    )
    .await
    .unwrap_err();
    assert_eq!(error_code(&err), "attachment_save_all_not_applicable");

    // A foreign account cannot save another account's message.
    let err = service::save_all_into(
        &fx.runtime,
        &NoTransport,
        &sift::dto::MessageRef::new("other", "m1"),
        &dir,
    )
    .await
    .unwrap_err();
    assert_eq!(error_code(&err), "attachment_save_all_not_applicable");
}

// ------------------------------------------------------------------------- P2.3 open

/// Downloaded executables require a backend-enforced confirmation; ordinary
/// documents do not, and Save As is untouched by the policy.
#[tokio::test]
async fn open_requires_confirmation_for_downloaded_executables() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "a1",
        "m1",
        "2",
        Some("Installer.dmg"),
        "application/octet-stream",
        Some(vec![1]),
    )
    .await;
    fx.attach(
        "a2",
        "m1",
        "3",
        Some("invoice.pdf"),
        "application/pdf",
        Some(vec![2]),
    )
    .await;

    let opened = Arc::new(Mutex::new(Vec::new()));
    let rec = fx.record("a1").await;
    let err = service::open(&fx.runtime, &NoTransport, &rec, false, {
        let opened = opened.clone();
        move |p: &Path| {
            opened.lock().unwrap().push(p.to_path_buf());
            Ok(())
        }
    })
    .await
    .unwrap_err();
    assert_eq!(error_code(&err), "attachment_confirmation_required");
    assert!(opened.lock().unwrap().is_empty(), "opener never invoked");

    service::open(&fx.runtime, &NoTransport, &rec, true, {
        let opened = opened.clone();
        move |p: &Path| {
            opened.lock().unwrap().push(p.to_path_buf());
            Ok(())
        }
    })
    .await
    .unwrap();
    assert_eq!(opened.lock().unwrap().len(), 1, "confirmed open proceeds");

    // Save As stays available for executable types: the copy succeeds.
    let dir = fx.runtime.data_dir.join("saved");
    let dest = dir.join("Installer.dmg");
    service::copy_to(&fx.runtime, &NoTransport, &rec, &dest)
        .await
        .unwrap();
    assert!(dest.exists());

    // A PDF opens without confirmation.
    let rec = fx.record("a2").await;
    service::open(&fx.runtime, &NoTransport, &rec, false, |_: &Path| Ok(()))
        .await
        .unwrap();
}

/// The open path uses the same verified cache as Save As; a cancelled preview
/// must not delete a file the user already saved.
#[tokio::test]
async fn open_uses_the_verified_cache_and_leaves_saved_copies_alone() {
    let fx = Fixture::new().await;
    fx.message("m1").await;
    fx.attach(
        "a1",
        "m1",
        "2",
        Some("invoice.pdf"),
        "application/pdf",
        Some(vec![1, 2, 3]),
    )
    .await;
    let rec = fx.record("a1").await;
    let saved = fx.runtime.data_dir.join("saved").join("invoice.pdf");
    service::copy_to(&fx.runtime, &NoTransport, &rec, &saved)
        .await
        .unwrap();
    let rec = fx.record("a1").await;
    assert_eq!(rec.cache_state, CacheState::Ready);
    service::open(&fx.runtime, &NoTransport, &rec, false, |_: &Path| Ok(()))
        .await
        .unwrap();
    assert!(saved.exists(), "the user's copy is not a cache entry");
}

// --------------------------------------------------------------------- P2.2 migration

/// Migration 0007 against a real pre-migration database: duplicate
/// `(message_id, part_id)` rows collapse onto the first stable row id while the
/// group's richest metadata, payload and cache file survive, and the natural
/// key becomes unique.
#[tokio::test]
async fn migration_0007_dedupes_and_preserves_richest_row() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sift.db");
    let payload = zstd::encode_all(&b"payload"[..], 3).unwrap();
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        for sql in [
            include_str!("../src/db/migrations/0001_init.sql"),
            include_str!("../src/db/migrations/0002_contacts_backfill.sql"),
            include_str!("../src/db/migrations/0003_imap.sql"),
            include_str!("../src/db/migrations/0004_mail_rendering.sql"),
            include_str!("../src/db/migrations/0005_remote_images_default.sql"),
            include_str!("../src/db/migrations/0006_rerender_bodies.sql"),
        ] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute("UPDATE schema_version SET version=6", [])
            .unwrap();
        conn.execute(
            "INSERT INTO accounts (id,email,created_at) VALUES ('acc','a@x.com',1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO messages (id,account_id,thread_id,internal_date) VALUES ('m1','acc','t1',1)",
            [],
        )
        .unwrap();
        // Group 1: a metadata-only first row, then richer duplicates.
        conn.execute(
            "INSERT INTO attachments (id,message_id,part_id,mime,size) VALUES ('keep','m1','1','application/octet-stream',100)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO attachments (id,message_id,part_id,mime,size,filename,gmail_att_id,content_id,data_z)
             VALUES ('dup-b','m1','1','application/pdf',150,'invoice.pdf','API-1','<cid@x>',?1)",
            rusqlite::params![payload],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO attachments (id,message_id,part_id,mime,size,local_path)
             VALUES ('dup-c','m1','1','application/pdf',200,'/cache/x/invoice.pdf')",
            [],
        )
        .unwrap();
        // Group 2: nothing cached at all.
        conn.execute(
            "INSERT INTO attachments (id,message_id,part_id,mime,size) VALUES ('meta','m1','2','text/plain',0)",
            [],
        )
        .unwrap();
    }

    // The real runner path: an existing v6 database upgrades on open.
    let db = Db::open(dir.path()).unwrap();
    let recs = db
        .attachments_records(&sift::dto::MessageRef::new("acc", "m1"))
        .await
        .unwrap();
    assert_eq!(recs.len(), 2, "duplicates collapsed");
    let one = recs.iter().find(|r| r.part_id == "1").unwrap();
    assert_eq!(one.id, "keep", "the first stable row id survives");
    assert_eq!(one.account_id, "acc");
    assert_eq!(one.filename.as_deref(), Some("invoice.pdf"));
    assert_eq!(one.gmail_att_id.as_deref(), Some("API-1"));
    assert_eq!(one.content_id.as_deref(), Some("<cid@x>"));
    assert_eq!(one.mime, "application/pdf");
    assert_eq!(one.size, 200);
    assert_eq!(one.local_path.as_deref(), Some("/cache/x/invoice.pdf"));
    assert_eq!(one.data.as_deref(), Some(&b"payload"[..]));
    assert_eq!(one.cache_state, CacheState::Unverified);
    assert_eq!(one.decoded_size, None, "verification is lazy");
    let meta = recs.iter().find(|r| r.part_id == "2").unwrap();
    assert_eq!(meta.cache_state, CacheState::Missing);
    assert!(meta.data.is_none());

    let version: i64 = db
        .read(|c| Ok(c.query_row("SELECT version FROM schema_version", [], |r| r.get(0))?))
        .await
        .unwrap();
    // Hardcoded version updated for 0008_account_scoping.
    assert_eq!(version, sift::db::SCHEMA_VERSION);
    let indexes: i64 = db
        .read(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='index'
                   AND name IN ('attachments_msg_idx','attachments_msg_part_uidx')",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(indexes, 2, "lookup and natural-key indexes exist");

    let duplicate_rejected = db
        .write(|c| {
            Ok(c.execute(
                "INSERT INTO attachments (id,account_id,message_id,part_id,mime) VALUES ('x','acc','m1','1','application/pdf')",
                [],
            )
            .is_err())
        })
        .await
        .unwrap();
    assert!(
        duplicate_rejected,
        "natural key is enforced after migration"
    );
}
