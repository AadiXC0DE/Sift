//! IMAP connection management (Phase 11 task 4).
//!
//! Exactly two connections per account: `worker` (all sync/fetch/op traffic)
//! and the idle slot owned by `idle.rs`. TLS via the platform verifier
//! (macOS keychain); plaintext TCP to loopback is test-only.
//!
//! P4.3 makes the worker a *lease*: the slot is held from the moment a caller
//! checks it out (including the initial connect, so ten simultaneous checkouts
//! open one connection) until the whole selected-mailbox batch is done. Callers
//! that need a mailbox use [`ImapPool::with_selected_worker`], which SELECTs
//! inside the lease so no other task can reselect between a SELECT and its
//! FETCH. A command whose future is dropped mid-response (cancellation) leaves
//! the connection poisoned, never reusable.
//!
//! Retry policy lives INSIDE [`Conn`]: every command runs once, and on a
//! connection-level failure the connection rebuilds (login included, with the
//! 1/2/5/15/60 s backoff) and the command retries exactly once. Callers only
//! ever see fatal errors. No pool lock is held across reconnects, so this
//! cannot deadlock.
use super::{
    errors,
    proto::{self, Response, ResponseCode, Untagged},
};
use crate::errors::SiftError;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex as TokioMutex, RwLock};
use tokio_util::sync::CancellationToken;

const BACKOFFS: [u64; 5] = [1, 2, 5, 15, 60];
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const KEEPALIVE_IDLE: Duration = Duration::from_secs(5 * 60);
/// Foreground connections idle longer than this are dropped (P1.5).
pub const FOREGROUND_IDLE: Duration = Duration::from_secs(60);

/// Absolute cap on non-literal response framing (status lines and FETCH item
/// text). Equivalent to the old per-line cap but enforced as a typed error
/// and applied to the whole response, not just one line (P2.4).
const MAX_FRAME_BYTES: usize = 8 * 1024 * 1024;
/// Absolute cap on a whole-message `BODY[]` literal (P2.4). Gmail refuses to
/// transfer messages over ~50 MB, so 64 MiB admits every real message while
/// still rejecting an unbounded server-declared length before allocating.
const MAX_WHOLE_MESSAGE_BYTES: usize = 64 * 1024 * 1024;
/// Framing tolerated around a requested partial literal (the
/// `BODY[<section>]<origin> {n}` line plus the closing paren). Generous but
/// bounded; a server declaring more than the requested bytes is rejected.
const PARTIAL_FRAMING_BYTES: usize = 64 * 1024;

/// Anything we can read IMAP responses from and write commands to.
trait ImapStream: AsyncRead + AsyncWrite + Unpin + Send {}
impl<T: AsyncRead + AsyncWrite + Unpin + Send> ImapStream for T {}

pub struct ImapCreds {
    pub email: String,
    pub pw: String,
}

#[derive(Debug, Clone, Default)]
pub struct Caps {
    pub condstore: bool,
    pub idle: bool,
    pub mov: bool,
    pub uidplus: bool,
    pub gm_ext: bool,
    pub appendlimit: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct SelectInfo {
    pub exists: u32,
    pub uidvalidity: u32,
    pub uidnext: u32,
    pub highestmodseq: Option<u64>,
    pub readonly: bool,
}

/// The mailbox a connection is currently selected onto (P1.3). `Conn` keeps
/// this so a reconnect can restore the selection and compare epochs before
/// any UID is reused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectedMailbox {
    pub name: String,
    pub readonly: bool,
    pub uidvalidity: u32,
}

impl SelectedMailbox {
    pub fn new(name: impl Into<String>, readonly: bool, uidvalidity: u32) -> Self {
        Self {
            name: name.into(),
            readonly,
            uidvalidity,
        }
    }
}

/// One FETCH data item (P1.1). Multiple items must render as a parenthesized
/// list; a single item may travel bare.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FetchItem {
    Uid,
    Flags,
    InternalDate,
    Rfc822Size,
    ModSeq,
    GmailMsgId,
    GmailThrId,
    GmailLabels,
    BodyStructure,
    /// `BODY.PEEK[<section>]` — never sets `\Seen`.
    PeekSection(super::ids::ImapSection),
    /// `BODY.PEEK[]` — whole message, never sets `\Seen`.
    PeekAll,
    /// `BODY.PEEK[<section>]<origin[.len]>` — partial section read. `None`
    /// section means the whole-message partial (`BODY.PEEK[]<origin.len>`).
    PeekPartial {
        section: Option<super::ids::ImapSection>,
        origin: u32,
        len: Option<u32>,
    },
    /// Already-formed item text (legacy callers migrating in P4.3).
    Raw(String),
}

impl FetchItem {
    fn write(&self, out: &mut String) {
        match self {
            FetchItem::Uid => out.push_str("UID"),
            FetchItem::Flags => out.push_str("FLAGS"),
            FetchItem::InternalDate => out.push_str("INTERNALDATE"),
            FetchItem::Rfc822Size => out.push_str("RFC822.SIZE"),
            FetchItem::ModSeq => out.push_str("MODSEQ"),
            FetchItem::GmailMsgId => out.push_str("X-GM-MSGID"),
            FetchItem::GmailThrId => out.push_str("X-GM-THRID"),
            FetchItem::GmailLabels => out.push_str("X-GM-LABELS"),
            FetchItem::BodyStructure => out.push_str("BODYSTRUCTURE"),
            FetchItem::PeekSection(s) => {
                out.push_str("BODY.PEEK[");
                s.write_wire(out);
                out.push(']');
            }
            FetchItem::PeekAll => out.push_str("BODY.PEEK[]"),
            FetchItem::PeekPartial {
                section,
                origin,
                len,
            } => {
                out.push_str("BODY.PEEK[");
                if let Some(s) = section {
                    s.write_wire(out);
                }
                out.push(']');
                use std::fmt::Write;
                out.push('<');
                let _ = write!(out, "{origin}");
                if let Some(l) = len {
                    let _ = write!(out, ".{l}");
                }
                out.push('>');
            }
            FetchItem::Raw(r) => out.push_str(r),
        }
    }
}

/// A balanced FETCH item list. This is the ONLY builder for the wire form;
/// callers must not concatenate item text by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchItems {
    items: Vec<FetchItem>,
}

impl FetchItems {
    pub fn new() -> Self {
        Self { items: vec![] }
    }

    pub fn of(items: impl IntoIterator<Item = FetchItem>) -> Self {
        Self {
            items: items.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn uid(mut self) -> Self {
        self.items.push(FetchItem::Uid);
        self
    }

    #[must_use]
    pub fn gmail_msgid(mut self) -> Self {
        self.items.push(FetchItem::GmailMsgId);
        self
    }

    #[must_use]
    pub fn bodystructure(mut self) -> Self {
        self.items.push(FetchItem::BodyStructure);
        self
    }

    #[must_use]
    pub fn peek_section(mut self, section: super::ids::ImapSection) -> Self {
        self.items.push(FetchItem::PeekSection(section));
        self
    }

    #[must_use]
    pub fn peek_all(mut self) -> Self {
        self.items.push(FetchItem::PeekAll);
        self
    }

    #[must_use]
    pub fn flags(mut self) -> Self {
        self.items.push(FetchItem::Flags);
        self
    }

    #[must_use]
    pub fn internaldate(mut self) -> Self {
        self.items.push(FetchItem::InternalDate);
        self
    }

    #[must_use]
    pub fn rfc822_size(mut self) -> Self {
        self.items.push(FetchItem::Rfc822Size);
        self
    }

    #[must_use]
    pub fn modseq(mut self) -> Self {
        self.items.push(FetchItem::ModSeq);
        self
    }

    #[must_use]
    pub fn gmail_thrid(mut self) -> Self {
        self.items.push(FetchItem::GmailThrId);
        self
    }

    #[must_use]
    pub fn gmail_labels(mut self) -> Self {
        self.items.push(FetchItem::GmailLabels);
        self
    }

    /// Partial section read, e.g. `BODY.PEEK[1]<0.2048>` (P1.1).
    #[must_use]
    pub fn peek_partial(
        mut self,
        section: Option<super::ids::ImapSection>,
        origin: u32,
        len: Option<u32>,
    ) -> Self {
        self.items.push(FetchItem::PeekPartial {
            section,
            origin,
            len,
        });
        self
    }

    /// Append one already-formed item atom (used for header-field sections
    /// that have no dedicated variant). The result is still one item.
    #[must_use]
    pub fn raw(mut self, item: impl Into<String>) -> Self {
        self.items.push(FetchItem::Raw(item.into()));
        self
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Wire form: bare for a single item, parenthesized for two or more.
    pub fn wire(&self) -> String {
        let mut out = String::with_capacity(32);
        if self.items.len() == 1 {
            self.items[0].write(&mut out);
        } else {
            out.push('(');
            for (i, it) in self.items.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                it.write(&mut out);
            }
            out.push(')');
        }
        out
    }
}

impl Default for FetchItems {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for FetchItems {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.wire())
    }
}

/// A failed FETCH that preserved its tagged completion (P1.6): the
/// attachment layer needs the kind and machine code, not just a string.
#[derive(Debug)]
pub enum TaggedFailure {
    Tagged {
        completion: errors::Completion,
        code: Option<ResponseCode>,
        text: String,
    },
    Conn(SiftError),
}

pub struct TaggedOutcome {
    pub ok: bool,
    pub completion: errors::Completion,
    pub code: Option<ResponseCode>,
    pub text: String,
}

impl TaggedOutcome {
    /// Completion kind of this tagged response (P1.6).
    pub fn kind(&self) -> errors::Completion {
        self.completion
    }
}

/// Completion kind of a tagged outcome (OK wins over NO/BAD).
pub fn completion_of(t: &TaggedOutcome) -> errors::Completion {
    t.completion
}

pub struct CommandResult {
    pub tagged: TaggedOutcome,
    pub untagged: Vec<Response>,
}

struct PoolInner {
    email: String,
    pw: String,
    host: String,
    port: u16,
    plaintext: bool,
    backoff_idx: StdMutex<usize>,
    caps: RwLock<Caps>,
    last_used: StdMutex<Instant>,
}

#[derive(Clone)]
pub struct ImapPool {
    inner: Arc<PoolInner>,
    worker: std::sync::Arc<TokioMutex<Option<Conn>>>,
    idle_slot: std::sync::Arc<TokioMutex<Option<Conn>>>,
}

/// Owned guard over the worker connection. Unlike a borrowed guard this can
/// be held across awaits by sync loops without lifetime friction.
pub struct WorkerGuard {
    guard: tokio::sync::OwnedMutexGuard<Option<Conn>>,
}

impl std::ops::Deref for WorkerGuard {
    type Target = Option<Conn>;
    fn deref(&self) -> &Self::Target {
        &self.guard
    }
}

impl std::ops::DerefMut for WorkerGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.guard
    }
}

/// A worker connection leased to one selected-mailbox batch (P4.3).
///
/// The lease is taken *before* the SELECT and released only when the caller
/// drops it, so a background tick, a foreground read and a drain-triggered sync
/// can never interleave a SELECT and the FETCHes that depend on it.
pub struct SelectedWorker {
    guard: WorkerGuard,
    folder: String,
    readonly: bool,
    info: SelectInfo,
}

impl SelectedWorker {
    /// The leased connection, already selected onto `folder`.
    pub fn conn(&mut self) -> &mut Conn {
        self.guard.as_mut().expect("leased worker is connected")
    }

    /// Values reported by the SELECT that opened this lease.
    pub fn info(&self) -> &SelectInfo {
        &self.info
    }

    /// Re-SELECT the leased mailbox and refresh [`Self::info`] (used for the
    /// end-of-pass checkpoint read).
    pub async fn reselect(&mut self) -> Result<&SelectInfo, SiftError> {
        let (folder, readonly) = (self.folder.clone(), self.readonly);
        let info = self.conn().select(&folder, readonly).await?;
        self.info = info;
        Ok(&self.info)
    }
}

/// Marks a connection that a command may have left mid-response when the
/// command's future is dropped (caller cancellation or timeout). The flag is
/// shared with the [`Conn`], which invalidates itself before its next command,
/// so another task can never read a half-consumed response (P4.3).
struct PoisonOnDrop {
    flag: Arc<AtomicBool>,
    armed: bool,
}

impl PoisonOnDrop {
    fn new(flag: Arc<AtomicBool>) -> Self {
        Self { flag, armed: true }
    }

    /// The command completed normally: the stream is framed correctly.
    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for PoisonOnDrop {
    fn drop(&mut self) {
        if self.armed {
            self.flag.store(true, Ordering::SeqCst);
        }
    }
}

pub struct Conn {
    io: BufReader<Box<dyn ImapStream>>,
    inner: Arc<PoolInner>,
    tag: u32,
    /// Selected mailbox, updated only after a successful SELECT/EXAMINE and
    /// cleared before any reconnect or on a poisoned connection (P1.3).
    selected: Option<SelectedMailbox>,
    /// A failed literal read leaves the stream framed wrong; the connection
    /// must be rebuilt before any reuse (P1.3).
    poisoned: bool,
    /// Set by a dropped (cancelled) command future; folded into `poisoned`
    /// before the next command runs (P4.3).
    poison_flag: Arc<AtomicBool>,
    /// While set, a response literal larger than this many bytes is refused
    /// (P2.4). [`Conn::uid_fetch_partial`] sets it to the requested byte
    /// count plus bounded framing; `None` means a whole-message/header read,
    /// bounded by [`MAX_WHOLE_MESSAGE_BYTES`] / [`MAX_FRAME_BYTES`].
    pending_literal_cap: Option<usize>,
}

impl ImapPool {
    pub fn new(email: String, pw: String, host: String, port: u16, plaintext: bool) -> Self {
        Self {
            inner: Arc::new(PoolInner {
                email,
                pw,
                host,
                port,
                plaintext,
                backoff_idx: StdMutex::new(0),
                caps: RwLock::new(Caps::default()),
                last_used: StdMutex::new(Instant::now()),
            }),
            worker: std::sync::Arc::new(TokioMutex::new(None)),
            idle_slot: std::sync::Arc::new(TokioMutex::new(None)),
        }
    }

    pub fn gmail(email: String, pw: String) -> Self {
        Self::new(email, pw, "imap.gmail.com".into(), 993, false)
    }

    pub async fn caps(&self) -> Caps {
        self.inner.caps.read().await.clone()
    }

    pub fn email(&self) -> String {
        self.inner.email.clone()
    }

    /// App password for SMTP AUTH (memory only; never logged).
    pub fn app_password(&self) -> String {
        self.inner.pw.clone()
    }

    /// Check out the worker, connecting (with backoff) and NOOP-keepalive
    /// as needed. Uncancelable; cancellation-aware callers use
    /// [`ImapPool::worker_cancellable`].
    pub async fn worker(&self) -> Result<WorkerGuard, SiftError> {
        self.worker_cancellable(&CancellationToken::new()).await
    }

    /// Check out the worker **holding the connection slot across the
    /// connect** (P4.3): ten simultaneous checkouts therefore open one
    /// connection, and a later caller waits instead of dialing its own.
    ///
    /// Cancellable while waiting for the slot, while connecting, and during
    /// the reconnect backoff; a cancelled connect never installs a connection
    /// for the generation that cancelled it.
    pub async fn worker_cancellable(
        &self,
        cancel: &CancellationToken,
    ) -> Result<WorkerGuard, SiftError> {
        let mut guard = tokio::select! {
            guard = self.worker.clone().lock_owned() => guard,
            _ = cancel.cancelled() => return Err(cancelled_worker()),
        };
        if guard.is_some() {
            let idle_for = self.inner.last_used.lock().unwrap().elapsed();
            if idle_for <= KEEPALIVE_IDLE {
                return Ok(WorkerGuard { guard });
            }
            // NOOP keep-alive when the connection sat idle (spec task 4).
            // A failed keepalive drops the connection and reconnects.
            let alive = match guard.as_mut() {
                Some(conn) => conn.noop().await.is_ok(),
                None => false,
            };
            if alive {
                *self.inner.last_used.lock().unwrap() = Instant::now();
                return Ok(WorkerGuard { guard });
            }
            *guard = None;
        }
        // Connected under the same slot: no competing connection can be
        // opened while this one is being established.
        let conn = tokio::select! {
            conn = self.connect_loop() => conn?,
            _ = cancel.cancelled() => return Err(cancelled_worker()),
        };
        *guard = Some(conn);
        Ok(WorkerGuard { guard })
    }

    /// Lease the worker with `folder` selected and hold it across the whole
    /// dependent command batch (P4.3).
    ///
    /// This is the ONLY way a selected-mailbox batch runs: the SELECT and the
    /// FETCH/STORE commands that depend on it share one exclusive lease, so a
    /// concurrent Trash sync cannot re-SELECT the mailbox under a read.
    pub async fn with_selected_worker(
        &self,
        folder: &str,
        readonly: bool,
        cancel: &CancellationToken,
    ) -> Result<SelectedWorker, SiftError> {
        let mut guard = self.worker_cancellable(cancel).await?;
        let info = guard
            .as_mut()
            .expect("worker checkout connects")
            .select(folder, readonly)
            .await?;
        Ok(SelectedWorker {
            guard,
            folder: folder.to_string(),
            readonly,
            info,
        })
    }

    /// Take the idle slot connection (idle.rs owns its lifecycle).
    pub async fn take_idle(&self) -> Option<Conn> {
        self.idle_slot.lock().await.take()
    }

    /// Return (or replace) the idle slot connection.
    pub async fn put_idle(&self, conn: Option<Conn>) {
        *self.idle_slot.lock().await = conn;
    }

    /// Build a fresh, logged-in worker connection (also used by idle.rs).
    pub async fn fresh_conn(&self) -> Result<Conn, SiftError> {
        self.connect_loop().await
    }

    /// Drop the worker so the next use reconnects (after a failed op already
    /// retried inside [`Conn`]).
    pub async fn drop_worker(&self) {
        *self.worker.lock().await = None;
    }

    fn backoff_next(&self) -> Duration {
        let mut idx = self.inner.backoff_idx.lock().unwrap();
        let d = Duration::from_secs(BACKOFFS[(*idx).min(BACKOFFS.len() - 1)]);
        *idx = idx.saturating_add(1);
        d
    }

    fn backoff_reset(&self) {
        *self.inner.backoff_idx.lock().unwrap() = 0;
    }

    /// Bounded reconnect: 1/2/5 s backoff, then the last error surfaces and
    /// the periodic loops (poll/drain) retry on their own cadence. Terminal
    /// auth errors return immediately so the wizard shows its fix at once.
    async fn connect_loop(&self) -> Result<Conn, SiftError> {
        const MAX_ATTEMPTS: usize = 3;
        let mut attempt = 0;
        loop {
            attempt += 1;
            match self.connect_once().await {
                Ok(conn) => {
                    self.backoff_reset();
                    *self.inner.last_used.lock().unwrap() = Instant::now();
                    return Ok(conn);
                }
                Err(e) if is_terminal(&e) || attempt >= MAX_ATTEMPTS => return Err(e),
                Err(e) => {
                    let wait = self.backoff_next();
                    log::warn!(target: "sift::imap", "connect failed ({e}); retry in {wait:?}");
                    tokio::time::sleep(wait).await;
                }
            }
        }
    }

    async fn connect_once(&self) -> Result<Conn, SiftError> {
        // NOTE: `plaintext` exists solely for the fake server in tests.
        // Every non-test constructor passes false (grep `ImapPool::new` /
        // `ImapPool::gmail` to audit); Gmail itself is TLS-only.
        if self.inner.plaintext {
            let stream = tokio::time::timeout(
                CONNECT_TIMEOUT,
                tokio::net::TcpStream::connect((self.inner.host.as_str(), self.inner.port)),
            )
            .await
            .map_err(|_| SiftError::app("offline", "No connection to Gmail.", true))?
            .map_err(errors::io_err)?;
            let mut conn = Conn::plain(stream, self.inner.clone());
            conn.greet().await?;
            conn.login().await?;
            return Ok(conn);
        }
        let stream = tokio::time::timeout(
            CONNECT_TIMEOUT,
            tokio::net::TcpStream::connect((self.inner.host.as_str(), self.inner.port)),
        )
        .await
        .map_err(|_| SiftError::app("offline", "No connection to Gmail.", true))?
        .map_err(errors::io_err)?;
        use rustls_platform_verifier::ConfigVerifierExt;
        crate::install_crypto_provider();
        let config = rustls::ClientConfig::with_platform_verifier().map_err(errors::tls_err)?;
        let connector = tokio_rustls::TlsConnector::from(std::sync::Arc::new(config));
        let domain = rustls_pki_types::ServerName::try_from(self.inner.host.as_str())
            .map_err(errors::tls_err)?
            .to_owned();
        let tls = tokio::time::timeout(CONNECT_TIMEOUT, connector.connect(domain, stream))
            .await
            .map_err(|_| SiftError::app("offline", "No connection to Gmail.", true))?
            .map_err(errors::tls_err)?;
        let mut conn = Conn::tls(tls, self.inner.clone());
        conn.greet().await?;
        conn.login().await?;
        Ok(conn)
    }
}

/// A checkout cancelled while waiting for (or establishing) the worker.
fn cancelled_worker() -> SiftError {
    SiftError::app(
        "cancelled",
        "Cancelled while waiting for the account's IMAP connection.",
        true,
    )
}

/// Errors that must NOT be retried by connect_loop (auth/config, not network).
fn is_terminal(e: &SiftError) -> bool {
    match e {
        SiftError::App { code, .. } => matches!(
            code.as_str(),
            "imap_bad_password"
                | "imap_needs_app_password"
                | "imap_web_login_required"
                | "imap_disabled_by_admin"
                | "imap_tls"
        ),
        _ => false,
    }
}

impl Conn {
    fn tls(
        stream: tokio_rustls::client::TlsStream<tokio::net::TcpStream>,
        inner: Arc<PoolInner>,
    ) -> Self {
        Self {
            io: BufReader::new(Box::new(stream)),
            inner,
            tag: 0,
            selected: None,
            poisoned: false,
            poison_flag: Arc::new(AtomicBool::new(false)),
            pending_literal_cap: None,
        }
    }

    fn plain(stream: tokio::net::TcpStream, inner: Arc<PoolInner>) -> Self {
        Self {
            io: BufReader::new(Box::new(stream)),
            inner,
            tag: 0,
            selected: None,
            poisoned: false,
            poison_flag: Arc::new(AtomicBool::new(false)),
            pending_literal_cap: None,
        }
    }

    /// The mailbox this connection has selected, if any.
    pub fn selected(&self) -> Option<&SelectedMailbox> {
        self.selected.as_ref()
    }

    /// Mark the connection unusable before reuse (failed literal read, or a
    /// cancellation/timeout mid-literal).
    pub fn invalidate(&mut self) {
        self.selected = None;
        self.poisoned = true;
    }

    fn next_tag(&mut self) -> String {
        self.tag += 1;
        format!("s{:04}", self.tag)
    }

    /// Rebuild the transport + re-login, preserving the tag counter. The
    /// selected mailbox is cleared: it is only meaningful on the old socket
    /// and must be restored explicitly by [`Conn::read_cmd`] (P1.3).
    async fn reconnect(&mut self) -> Result<(), SiftError> {
        self.selected = None;
        self.poisoned = false;
        // A fresh stream is framed correctly by definition: a poison flag set
        // for the *old* socket must not invalidate this one.
        self.poison_flag.store(false, Ordering::SeqCst);
        // Fresh inner (new caps/backoff): sharing `self.inner` here would
        // deadlock copying caps (write + read on the same RwLock).
        let fresh_inner = std::sync::Arc::new(PoolInner {
            email: self.inner.email.clone(),
            pw: self.inner.pw.clone(),
            host: self.inner.host.clone(),
            port: self.inner.port,
            plaintext: self.inner.plaintext,
            backoff_idx: StdMutex::new(0),
            caps: RwLock::new(Caps::default()),
            last_used: StdMutex::new(Instant::now()),
        });
        let pool = ImapPool {
            inner: fresh_inner,
            worker: std::sync::Arc::new(TokioMutex::new(None)),
            idle_slot: std::sync::Arc::new(TokioMutex::new(None)),
        };
        let fresh = pool.connect_loop().await?;
        self.io = fresh.io;
        let caps = pool.inner.caps.read().await.clone();
        *self.inner.caps.write().await = caps;
        Ok(())
    }

    pub(crate) async fn read_response(&mut self) -> Result<Response, SiftError> {
        let mut buf = vec![];
        // Non-literal framing bytes accumulated for this response; a spliced
        // literal is tracked separately so a large BODY[] never trips the
        // 8 MiB metadata cap.
        let mut frame_bytes: usize = 0;
        loop {
            // Read one line (or fail on timeout/close).
            let read_line = async {
                let mut line: Vec<u8> = vec![];
                loop {
                    let b = self.io.read_u8().await?;
                    line.push(b);
                    if line.ends_with(b"\r\n") {
                        break;
                    }
                    if line.len() > MAX_FRAME_BYTES {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "line too long",
                        ));
                    }
                }
                Ok::<Vec<u8>, std::io::Error>(line)
            };
            let line = match tokio::time::timeout(CONNECT_TIMEOUT, read_line).await {
                Ok(Ok(line)) => line,
                Ok(Err(e)) if e.kind() == std::io::ErrorKind::InvalidData => {
                    // Typed, not a panic/assert (P2.4).
                    return Err(protocol_error("IMAP response line exceeds the 8 MiB cap"));
                }
                Ok(Err(e)) => {
                    self.invalidate();
                    return Err(errors::io_err(e));
                }
                Err(_) => {
                    self.invalidate();
                    return Err(SiftError::app("offline", "No connection to Gmail.", true));
                }
            };
            frame_bytes = frame_bytes
                .checked_add(line.len())
                .ok_or_else(|| protocol_error("IMAP response framing length overflow"))?;
            if frame_bytes > MAX_FRAME_BYTES {
                return Err(protocol_error(
                    "IMAP response framing exceeds the 8 MiB metadata cap",
                ));
            }
            buf.extend_from_slice(&line);
            // Splice `{n}` / `{n+}` literals inline so the parser sees whole
            // responses.
            let literal = match trailing_literal(&line) {
                Trailing::None => None,
                Trailing::Overflow => {
                    return Err(protocol_error(
                        "server declared a literal length Sift cannot represent",
                    ))
                }
                Trailing::Len(n) => Some(n),
            };
            if let Some(n) = literal {
                // NOTE: no CRLF follows literal bytes on the wire - the
                // enclosing `)` (or the next response) comes immediately.
                // A short/aborted literal leaves the stream mis-framed, so
                // the connection is invalidated before any reuse (P1.3).
                let cap = match self.pending_literal_cap {
                    Some(cap) => cap,
                    None => literal_cap_for(&line),
                };
                if n > cap {
                    return Err(protocol_error(&format!(
                        "server declared a {n}-byte literal over Sift's {cap}-byte cap"
                    )));
                }
                let end = buf
                    .len()
                    .checked_add(n)
                    .ok_or_else(|| protocol_error("IMAP literal length overflow"))?;
                buf.try_reserve(n).map_err(|_| {
                    protocol_error("IMAP literal allocation exceeded available memory")
                })?;
                buf.resize(end, 0);
                let start = end - n;
                match tokio::time::timeout(
                    CONNECT_TIMEOUT,
                    self.io.read_exact(&mut buf[start..end]),
                )
                .await
                {
                    Ok(Ok(_)) => {}
                    Ok(Err(e)) => {
                        self.invalidate();
                        return Err(errors::io_err(e));
                    }
                    Err(_) => {
                        self.invalidate();
                        return Err(SiftError::app("offline", "No connection to Gmail.", true));
                    }
                }
                continue;
            }
            match proto::parse_response(&buf) {
                Ok((used, r)) => {
                    debug_assert_eq!(used, buf.len());
                    let _ = used;
                    return Ok(r);
                }
                Err(proto::ParseError::Incomplete) => continue,
                Err(proto::ParseError::Malformed(msg)) => {
                    return Err(protocol_error(&format!(
                        "Could not parse Gmail response: {msg}"
                    )));
                }
            }
        }
    }

    /// Bounded partial section read (P2.4): `UID FETCH <uid>
    /// (UID BODY.PEEK[<section>]<origin.len>)`. The server may not send more
    /// than the requested byte count plus bounded framing; a larger declared
    /// literal is a typed error, never an unbounded allocation.
    ///
    /// Returns the section bytes and the server-echoed origin when the
    /// response carries them for exactly this UID and section; `Ok(None)`
    /// means the server returned a row without that section (so the caller
    /// can treat the start of a stream as a missing part and a later origin
    /// as end-of-data).
    pub async fn uid_fetch_partial(
        &mut self,
        uid: u32,
        section: super::ids::ImapSection,
        origin: u32,
        len: u32,
    ) -> Result<Option<(u32, Vec<u8>)>, SiftError> {
        let cap = (len as usize)
            .checked_add(PARTIAL_FRAMING_BYTES)
            .ok_or_else(|| protocol_error("partial literal cap overflow"))?;
        let want = section.wire();
        let items = FetchItems::new()
            .uid()
            .peek_partial(Some(section), origin, Some(len));
        self.pending_literal_cap = Some(cap);
        let result = self.read_cmd(&format!("UID FETCH {uid} {items}")).await;
        self.pending_literal_cap = None;
        let r = result?;
        if !r.tagged.ok {
            errors::map_response("UID FETCH", &r.tagged.text)?;
        }
        for u in r.untagged {
            if let Response::Untagged(Untagged::Fetch { attrs, .. }) = u {
                let row_uid = attrs.iter().find_map(|a| match a {
                    proto::FetchAttr::Uid(v) => Some(*v),
                    _ => None,
                });
                if row_uid != Some(uid) {
                    continue;
                }
                for a in attrs {
                    if let proto::FetchAttr::BodySection {
                        section: got,
                        origin: got_origin,
                        bytes,
                    } = a
                    {
                        if got.eq_ignore_ascii_case(&want) {
                            return Ok(Some((got_origin, bytes)));
                        }
                    }
                }
                // Matching row without the requested section data.
                return Ok(None);
            }
        }
        Ok(None)
    }

    /// Send one command **without** any transparent recovery (P1.3).
    ///
    /// Mutations (APPEND/MOVE/STORE/EXPUNGE/COPY/CREATE) must never be
    /// replayed blindly: the first attempt may already have been accepted.
    /// Safe reads use [`Conn::read_cmd`], which restores the selected
    /// mailbox and verifies its epoch before reissuing.
    pub async fn cmd(&mut self, body: &str) -> Result<CommandResult, SiftError> {
        self.cmd_once(body).await
    }

    /// Read-operation wrapper (P1.3): reconnect once, restore the previously
    /// selected mailbox, verify UIDVALIDITY, then reissue the read.
    ///
    /// Returns `imap_uidvalidity_changed` when the epoch moved, so the
    /// locator resolver can invalidate the stale UID mapping instead of
    /// fetching a UID that now names a different message.
    pub async fn read_cmd(&mut self, body: &str) -> Result<CommandResult, SiftError> {
        // Capture the selection BEFORE the read: a failed literal/stream
        // invalidates it, but the recovery still has to restore the mailbox
        // the caller was reading from.
        let want = self.selected.clone();
        match self.cmd_once(body).await {
            Err(e) if is_conn_error(&e) => {
                log::debug!(
                    target: "sift::imap",
                    "connection lost on read; reconnecting and restoring selection"
                );
                // Boxed: reconnect() can reach connect_loop(), which reaches
                // back here - boxing breaks the async type recursion.
                Box::pin(self.reconnect()).await?;
                if let Some(sel) = want {
                    let info = Box::pin(self.select_once(&sel.name, sel.readonly)).await?;
                    if info.uidvalidity != sel.uidvalidity {
                        log::warn!(
                            target: "sift::imap",
                            "UIDVALIDITY changed on reconnect ({} -> {}); refusing to reuse UIDs",
                            sel.uidvalidity,
                            info.uidvalidity
                        );
                        return Err(errors::uidvalidity_changed(&format!(
                            "EXAMINE at restore-epoch (code=- ref {})",
                            errors::correlation_id("", &sel.name, "uidvalidity")
                        )));
                    }
                }
                self.cmd_once(body).await
            }
            other => other,
        }
    }

    async fn cmd_once(&mut self, body: &str) -> Result<CommandResult, SiftError> {
        // A cancelled command leaves the stream mid-response; the flag makes
        // sure no other task can ever reuse that connection (P4.3).
        let guard = PoisonOnDrop::new(self.poison_flag.clone());
        let out = self.cmd_once_inner(body).await;
        guard.disarm();
        out
    }

    async fn cmd_once_inner(&mut self, body: &str) -> Result<CommandResult, SiftError> {
        if self.poison_flag.swap(false, Ordering::SeqCst) {
            self.invalidate();
        }
        if self.poisoned {
            return Err(SiftError::app(
                "imap_transient",
                "Stale connection; reconnecting.",
                true,
            ));
        }
        let tag = self.next_tag();
        let line = format!("{tag} {body}\r\n");
        log::trace!(target: "sift::imap", "C: {}", errors::redact_cmd(&format!("{tag} {body}")));
        tokio::time::timeout(CONNECT_TIMEOUT, self.io.write_all(line.as_bytes()))
            .await
            .map_err(|_| SiftError::app("offline", "No connection to Gmail.", true))?
            .map_err(errors::io_err)?;
        self.io.flush().await.map_err(errors::io_err)?;
        let mut untagged = vec![];
        loop {
            match self.read_response().await? {
                Response::TaggedOk { tag: t, code, text } if t == tag => {
                    return Ok(CommandResult {
                        tagged: TaggedOutcome {
                            ok: true,
                            completion: errors::Completion::Ok,
                            code,
                            text,
                        },
                        untagged,
                    });
                }
                Response::TaggedNo { tag: t, code, text } if t == tag => {
                    return Ok(CommandResult {
                        tagged: TaggedOutcome {
                            ok: false,
                            completion: errors::Completion::No,
                            code,
                            text,
                        },
                        untagged,
                    });
                }
                Response::TaggedBad { tag: t, code, text } if t == tag => {
                    return Ok(CommandResult {
                        tagged: TaggedOutcome {
                            ok: false,
                            completion: errors::Completion::Bad,
                            code,
                            text,
                        },
                        untagged,
                    });
                }
                Response::Untagged(u) => untagged.push(Response::Untagged(u)),
                Response::Cont(_) => {}
                // Tagged completion for a *different* tag: protocol desync.
                r => {
                    self.invalidate();
                    return Err(SiftError::app(
                        "imap_protocol",
                        format!("Unexpected response: {r:?}"),
                        true,
                    ));
                }
            }
        }
    }

    /// Expect OK; map NO/BAD through the task-4 table.
    pub async fn cmd_ok(&mut self, command: &str, body: &str) -> Result<CommandResult, SiftError> {
        let r = self.cmd(body).await?;
        if r.tagged.ok {
            Ok(r)
        } else {
            errors::map_response(command, &r.tagged.text)?;
            unreachable!("map_response always errors")
        }
    }

    /// Begin IDLE: sends a tagged IDLE and consumes the `+` continuation.
    /// Returns the tag so the caller can match the terminating completion.
    pub async fn start_idle(&mut self) -> Result<String, SiftError> {
        let guard = PoisonOnDrop::new(self.poison_flag.clone());
        let out = self.start_idle_inner().await;
        guard.disarm();
        out
    }

    async fn start_idle_inner(&mut self) -> Result<String, SiftError> {
        if self.poison_flag.swap(false, Ordering::SeqCst) {
            self.invalidate();
        }
        if self.poisoned {
            return Err(SiftError::app(
                "imap_transient",
                "Stale connection; reconnecting.",
                true,
            ));
        }
        let tag = self.next_tag();
        self.write_raw(format!("{tag} IDLE\r\n").as_bytes()).await?;
        match self.read_response().await? {
            Response::Cont(_) => Ok(tag),
            r => Err(SiftError::app(
                "imap_protocol",
                format!("IDLE rejected: {r:?}"),
                true,
            )),
        }
    }

    /// Raw write (IDLE/DONE/APPEND continuation handling lives in callers).
    pub async fn write_raw(&mut self, bytes: &[u8]) -> Result<(), SiftError> {
        self.io.write_all(bytes).await.map_err(errors::io_err)?;
        self.io.flush().await.map_err(errors::io_err)?;
        *self.inner.last_used.lock().unwrap() = Instant::now();
        Ok(())
    }

    // -- handshake ----------------------------------------------------------

    async fn greet(&mut self) -> Result<(), SiftError> {
        match self.read_response().await? {
            Response::Untagged(_) => Ok(()),
            r => Err(SiftError::app(
                "imap_protocol",
                format!("Bad greeting: {r:?}"),
                true,
            )),
        }
    }

    pub async fn capability(&mut self) -> Result<Vec<String>, SiftError> {
        // NOTE: cmd_once, not cmd(): this runs inside reconnect() and must
        // not retry-or-reconnect itself (unbounded nesting on sticky failure).
        let r = self.cmd_once("CAPABILITY").await?;
        let mut caps = vec![];
        for u in r.untagged {
            if let Response::Untagged(Untagged::Capability(c)) = u {
                caps.extend(c);
            }
        }
        if !r.tagged.ok {
            errors::map_response("CAPABILITY", &r.tagged.text)?;
        }
        Ok(caps)
    }

    pub async fn login(&mut self) -> Result<(), SiftError> {
        let caps = self.capability().await?;
        let has = |n: &str| caps.iter().any(|c| c == n);
        // Server ID (ignored response, logged at trace).
        let _ = self
            .cmd_once("ID (\"name\" \"Sift\" \"version\" \"1.1\")")
            .await;
        // LOGIN with the app password; the line never reaches logs raw.
        let cmd = format!(
            "LOGIN {} {}",
            imap_quote(&self.inner.email),
            imap_quote(&self.inner.pw)
        );
        let r = self.cmd_once(&cmd).await?;
        if !r.tagged.ok {
            errors::map_response("LOGIN", &r.tagged.text)?;
        }
        if has("CONDSTORE") {
            let e = self.cmd_once("ENABLE CONDSTORE").await;
            if e.map(|r| r.tagged.ok).unwrap_or(false) {
                self.inner.caps.write().await.condstore = true;
            }
        }
        {
            let mut c = self.inner.caps.write().await;
            c.idle = has("IDLE");
            c.mov = has("MOVE");
            c.uidplus = has("UIDPLUS");
            c.gm_ext = has("X-GM-EXT-1");
        }
        *self.inner.last_used.lock().unwrap() = Instant::now();
        Ok(())
    }

    pub async fn noop(&mut self) -> Result<(), SiftError> {
        let r = self.read_cmd("NOOP").await?;
        if r.tagged.ok {
            *self.inner.last_used.lock().unwrap() = Instant::now();
            Ok(())
        } else {
            errors::map_response("NOOP", &r.tagged.text)?;
            unreachable!()
        }
    }

    pub async fn logout(&mut self) -> Result<(), SiftError> {
        let _ = self.cmd("LOGOUT").await;
        Ok(())
    }

    // -- mailbox commands -----------------------------------------------------

    pub async fn select(&mut self, folder: &str, readonly: bool) -> Result<SelectInfo, SiftError> {
        match self.select_once(folder, readonly).await {
            Err(e) if is_conn_error(&e) => {
                Box::pin(self.reconnect()).await?;
                self.select_once(folder, readonly).await
            }
            other => other,
        }
    }

    /// One SELECT/EXAMINE. `selected` is cleared before the command and set
    /// only on success, so a failed reselect can never leave a stale mailbox
    /// epoch behind.
    async fn select_once(
        &mut self,
        folder: &str,
        readonly: bool,
    ) -> Result<SelectInfo, SiftError> {
        self.selected = None;
        let verb = if readonly { "EXAMINE" } else { "SELECT" };
        let r = self
            .cmd_once(&format!("{verb} {}", quote_folder(folder)))
            .await?;
        if !r.tagged.ok {
            errors::map_response(verb, &r.tagged.text)?;
        }
        let mut info = SelectInfo {
            exists: 0,
            uidvalidity: 0,
            uidnext: 0,
            highestmodseq: None,
            readonly,
        };
        for u in r.untagged {
            match u {
                Response::Untagged(Untagged::Exists(n)) => info.exists = n,
                Response::Untagged(Untagged::UidValidity(v)) => info.uidvalidity = v,
                Response::Untagged(Untagged::UidNext(v)) => info.uidnext = v,
                Response::Untagged(Untagged::HighestModSeq(v)) => info.highestmodseq = Some(v),
                _ => {}
            }
        }
        *self.inner.last_used.lock().unwrap() = Instant::now();
        self.selected = Some(SelectedMailbox::new(folder, readonly, info.uidvalidity));
        Ok(info)
    }

    /// LIST with SPECIAL-USE return options. Returns (attributes,
    /// delimiter, raw server name) per folder; see folders::map_folders.
    pub async fn list_special(
        &mut self,
    ) -> Result<Vec<(Vec<String>, Option<String>, String)>, SiftError> {
        let r = self.read_cmd("LIST \"\" \"*\" RETURN (SPECIAL-USE)").await?;
        if !r.tagged.ok {
            super::errors::map_response("LIST", &r.tagged.text)?;
        }
        let mut out = vec![];
        for u in r.untagged {
            if let Response::Untagged(Untagged::List { attrs, delim, name }) = u {
                out.push((attrs, delim, name));
            }
        }
        Ok(out)
    }

    /// Typed UID FETCH (P1.1). The item list is built by [`FetchItems`], so a
    /// multi-item command is always parenthesized.
    pub async fn uid_fetch_items(
        &mut self,
        uid_set: &str,
        items: &FetchItems,
    ) -> Result<Vec<(u32, Vec<super::proto::FetchAttr>)>, SiftError> {
        match self.uid_fetch_typed(uid_set, items).await {
            Ok(rows) => Ok(rows),
            Err(TaggedFailure::Tagged {
                completion,
                code,
                text,
            }) => {
                let ctx = errors::OpCtx {
                    command: "UID FETCH",
                    stage: "fetch",
                    correlation: "-",
                };
                Err(errors::map_tagged(&ctx, completion, code.as_ref(), &text))
            }
            Err(TaggedFailure::Conn(e)) => Err(e),
        }
    }

    /// UID FETCH that preserves the tagged completion kind and machine
    /// response code on failure (P1.6). Safe read: it goes through the
    /// reconnect+reselect wrapper.
    pub async fn uid_fetch_typed(
        &mut self,
        uid_set: &str,
        items: &FetchItems,
    ) -> Result<Vec<(u32, Vec<super::proto::FetchAttr>)>, TaggedFailure> {
        self.uid_fetch_typed_ext(uid_set, items, None).await
    }

    /// [`Conn::uid_fetch_typed`] with an extra trailing modifier such as
    /// `(CHANGEDSINCE n)`. This is the single wire-assembly point for FETCH.
    pub async fn uid_fetch_typed_ext(
        &mut self,
        uid_set: &str,
        items: &FetchItems,
        suffix: Option<&str>,
    ) -> Result<Vec<(u32, Vec<super::proto::FetchAttr>)>, TaggedFailure> {
        let body = match suffix {
            Some(s) => format!("UID FETCH {uid_set} {items} {s}"),
            None => format!("UID FETCH {uid_set} {items}"),
        };
        let r = match self.read_cmd(&body).await {
            Ok(r) => r,
            Err(e) => return Err(TaggedFailure::Conn(e)),
        };
        if !r.tagged.ok {
            return Err(TaggedFailure::Tagged {
                completion: completion_of(&r.tagged),
                code: r.tagged.code.clone(),
                text: r.tagged.text.clone(),
            });
        }
        let mut out = vec![];
        for u in r.untagged {
            if let Response::Untagged(Untagged::Fetch { seq, attrs }) = u {
                out.push((seq, attrs));
            }
        }
        Ok(out)
    }

    /// All UIDs in the selected folder, ascending.
    pub async fn uid_search_all(&mut self) -> Result<Vec<u32>, SiftError> {
        let r = self.read_cmd("UID SEARCH ALL").await?;
        if !r.tagged.ok {
            super::errors::map_response("UID SEARCH", &r.tagged.text)?;
        }
        let mut out = vec![];
        for u in r.untagged {
            if let Response::Untagged(Untagged::Search(uids)) = u {
                out.extend(uids);
            }
        }
        out.sort_unstable();
        Ok(out)
    }

    /// UIDs in the selected folder at or above `from` (optionally bound by
    /// `to`), ascending — the *actual* UIDs, not the numbers in between.
    ///
    /// P4.5: a partial sync must never materialize `uidnext - prev_uidnext`
    /// UIDs; a UIDNEXT jump of a billion with two new messages is one SEARCH
    /// and two rows here.
    pub async fn uid_search_range(
        &mut self,
        from: u32,
        to: Option<u32>,
    ) -> Result<Vec<u32>, SiftError> {
        let range = match to {
            Some(to) => format!("{from}:{to}"),
            None => format!("{from}:*"),
        };
        let r = self.read_cmd(&format!("UID SEARCH UID {range}")).await?;
        if !r.tagged.ok {
            super::errors::map_response("UID SEARCH", &r.tagged.text)?;
        }
        let mut out = vec![];
        for u in r.untagged {
            if let Response::Untagged(Untagged::Search(uids)) = u {
                out.extend(uids);
            }
        }
        // A conforming server returns only UIDs in the requested interval;
        // enforcing it here keeps a sloppy server from widening the scan.
        out.retain(|u| *u >= from && to.map(|t| *u <= t).unwrap_or(true));
        out.sort_unstable();
        out.dedup();
        Ok(out)
    }

    /// Flag/label delta since a modseq (CONDSTORE). Empty vec when the
    /// server reports nothing newer.
    pub async fn uid_fetch_changed(
        &mut self,
        modseq: u64,
    ) -> Result<Vec<(u32, Vec<super::proto::FetchAttr>)>, SiftError> {
        let items = FetchItems::new()
            .uid()
            .flags()
            .gmail_labels()
            .gmail_thrid()
            .modseq();
        match self
            .uid_fetch_typed_ext("1:*", &items, Some(&format!("(CHANGEDSINCE {modseq})")))
            .await
        {
            Ok(rows) => Ok(rows),
            Err(TaggedFailure::Tagged {
                completion,
                code,
                text,
            }) => {
                let ctx = errors::OpCtx {
                    command: "UID FETCH",
                    stage: "changed",
                    correlation: "-",
                };
                Err(errors::map_tagged(&ctx, completion, code.as_ref(), &text))
            }
            Err(TaggedFailure::Conn(e)) => Err(e),
        }
    }

    /// UIDs in the selected folder with a given Gmail message id.
    pub async fn uid_search_gmmsgid(&mut self, msgid: u64) -> Result<Vec<u32>, SiftError> {
        let r = self
            .read_cmd(&format!("UID SEARCH X-GM-MSGID {msgid}"))
            .await?;
        if !r.tagged.ok {
            super::errors::map_response("UID SEARCH", &r.tagged.text)?;
        }
        let mut out = vec![];
        for u in r.untagged {
            if let Response::Untagged(Untagged::Search(uids)) = u {
                out.extend(uids);
            }
        }
        out.sort_unstable();
        Ok(out)
    }

    /// Gmail web-syntax search (X-GM-RAW), newest UID order not guaranteed.
    pub async fn uid_search_raw(&mut self, query: &str) -> Result<Vec<u32>, SiftError> {
        // Quote-escape the query; Gmail syntax travels verbatim otherwise.
        let q = query.replace('\\', "\\\\").replace('"', "\\\"");
        let r = self
            .read_cmd(&format!("UID SEARCH X-GM-RAW \"{q}\""))
            .await?;
        if !r.tagged.ok {
            super::errors::map_response("UID SEARCH", &r.tagged.text)?;
        }
        let mut out = vec![];
        for u in r.untagged {
            if let Response::Untagged(Untagged::Search(uids)) = u {
                out.extend(uids);
            }
        }
        Ok(out)
    }

    /// STORE ±FLAGS.SILENT / ±X-GM-LABELS over a UID set.
    pub async fn uid_store(
        &mut self,
        uid_set: &str,
        mode: &str,
        values: &[String],
    ) -> Result<(), SiftError> {
        // mode: "+FLAGS" / "-FLAGS" / "+X-GM-LABELS" / "-X-GM-LABELS"
        let list = values
            .iter()
            .map(|v| {
                if v.starts_with('\\') || !v.contains(&[' ', '"'][..]) {
                    v.clone()
                } else {
                    format!("\"{v}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let r = self
            .cmd(&format!("UID STORE {uid_set} {mode}.SILENT ({list})"))
            .await?;
        if !r.tagged.ok {
            super::errors::map_response("UID STORE", &r.tagged.text)?;
        }
        Ok(())
    }

    /// MOVE to another folder (COPY + \Deleted + EXPUNGE fallback in ops).
    pub async fn uid_move(&mut self, uid_set: &str, dest: &str) -> Result<(), SiftError> {
        let r = self
            .cmd(&format!("UID MOVE {uid_set} {}", quote_folder(dest)))
            .await?;
        if !r.tagged.ok {
            super::errors::map_response("UID MOVE", &r.tagged.text)?;
        }
        Ok(())
    }

    pub async fn uid_copy(&mut self, uid_set: &str, dest: &str) -> Result<(), SiftError> {
        let r = self
            .cmd(&format!("UID COPY {uid_set} {}", quote_folder(dest)))
            .await?;
        if !r.tagged.ok {
            super::errors::map_response("UID COPY", &r.tagged.text)?;
        }
        Ok(())
    }

    /// EXPUNGE messages flagged \Deleted (optionally restricted to a set).
    pub async fn uid_expunge(&mut self, uid_set: Option<&str>) -> Result<(), SiftError> {
        let cmd = match uid_set {
            Some(s) => format!("UID EXPUNGE {s}"),
            None => "EXPUNGE".to_string(),
        };
        let r = self.cmd(&cmd).await?;
        if !r.tagged.ok {
            super::errors::map_response("EXPUNGE", &r.tagged.text)?;
        }
        Ok(())
    }

    /// CREATE a folder (user labels). Name travels as given (already UTF-7).
    pub async fn create(&mut self, folder: &str) -> Result<(), SiftError> {
        let r = self
            .cmd(&format!("CREATE {}", quote_folder(folder)))
            .await?;
        if !r.tagged.ok {
            super::errors::map_response("CREATE", &r.tagged.text)?;
        }
        Ok(())
    }

    /// APPEND a message; returns (uidvalidity, assigned uid) from APPENDUID.
    pub async fn append(
        &mut self,
        folder: &str,
        flags: &[String],
        bytes: &[u8],
    ) -> Result<(u32, u32), SiftError> {
        let guard = PoisonOnDrop::new(self.poison_flag.clone());
        let out = self.append_inner(folder, flags, bytes).await;
        guard.disarm();
        out
    }

    async fn append_inner(
        &mut self,
        folder: &str,
        flags: &[String],
        bytes: &[u8],
    ) -> Result<(u32, u32), SiftError> {
        if self.poison_flag.swap(false, Ordering::SeqCst) {
            self.invalidate();
        }
        if self.poisoned {
            return Err(SiftError::app(
                "imap_transient",
                "Stale connection; reconnecting.",
                true,
            ));
        }
        let flag_str = if flags.is_empty() {
            String::new()
        } else {
            format!(" ({})", flags.join(" "))
        };
        // Tagged LITERAL+ form: no continuation wait (Gmail advertises LITERAL+).
        let tag = self.next_tag();
        let head = format!(
            "{tag} APPEND {}{flag_str} {{{}+}}\r\n",
            quote_folder(folder),
            bytes.len()
        );
        self.write_raw(head.as_bytes()).await?;
        self.write_raw(bytes).await?;
        // Read until our tagged completion (ignore other untagged).
        loop {
            match self.read_response().await? {
                Response::TaggedOk { tag: t, code, .. } if t == tag => {
                    if let Some(ResponseCode::AppendUid { uidvalidity, uid }) = code {
                        return Ok((uidvalidity, uid));
                    }
                    return Err(SiftError::app(
                        "imap_protocol",
                        "APPEND succeeded without APPENDUID",
                        true,
                    ));
                }
                Response::TaggedNo { tag: t, text, .. } if t == tag => {
                    super::errors::map_response("APPEND", &text)?;
                    unreachable!("map_response always errors")
                }
                Response::TaggedBad { tag: t, text, .. } if t == tag => {
                    super::errors::map_response("APPEND", &text)?;
                    unreachable!("map_response always errors")
                }
                Response::Untagged(_) | Response::Cont(_) => {}
                // Tagged for another tag (should not happen; only APPEND in flight).
                _ => {}
            }
        }
    }

    pub async fn status(&mut self, folder: &str) -> Result<HashMap<String, u64>, SiftError> {
        let r = self
            .read_cmd(&format!(
                "STATUS {} (MESSAGES UIDNEXT UIDVALIDITY HIGHESTMODSEQ)",
                quote_folder(folder)
            ))
            .await?;
        if !r.tagged.ok {
            errors::map_response("STATUS", &r.tagged.text)?;
        }
        let mut out = HashMap::new();
        for u in r.untagged {
            if let Response::Untagged(Untagged::Status { values, .. }) = u {
                for (k, v) in values {
                    out.insert(k, v);
                }
            }
        }
        Ok(out)
    }
}

/// Encode a folder name for the wire (modified UTF-7) and quote it.
/// ASCII names (INBOX, All Mail) pass through byte-identical.
pub fn quote_folder(name: &str) -> String {
    imap_quote(&super::proto::encode_utf7_mailbox(name))
}

/// Quote a mailbox name / credential for the wire.
pub fn imap_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Trailing `{n}` / `{n+}` on a response line → literal length to splice.
/// A syntactically valid but unrepresentable length is [`Trailing::Overflow`]
/// so it becomes a typed protocol error instead of a silent "incomplete".
#[derive(Debug, PartialEq, Eq)]
enum Trailing {
    None,
    Len(usize),
    Overflow,
}

fn trailing_literal(line: &[u8]) -> Trailing {
    let line = line.strip_suffix(b"\r\n").unwrap_or(line);
    let Some(open) = line.iter().rposition(|&b| b == b'{') else {
        return Trailing::None;
    };
    let Ok(inner) = std::str::from_utf8(&line[open + 1..]) else {
        return Trailing::None;
    };
    let Some(inner) = inner.strip_suffix('}') else {
        return Trailing::None;
    };
    let num = inner.strip_suffix('+').unwrap_or(inner);
    // A bare number; anything else (e.g. command echoes) is not a literal.
    if num.bytes().all(|b| b.is_ascii_digit()) && !num.is_empty() {
        match num.parse::<usize>() {
            Ok(n) => Trailing::Len(n),
            Err(_) => Trailing::Overflow,
        }
    } else {
        Trailing::None
    }
}

/// Cap for a literal when no partial request is outstanding: header/field
/// literals stay within the metadata cap; a whole-message `BODY[]` uses the
/// message cap.
fn literal_cap_for(line: &[u8]) -> usize {
    let up = line.to_ascii_uppercase();
    if up.windows(6).any(|w| w == b"HEADER") {
        MAX_FRAME_BYTES
    } else {
        MAX_WHOLE_MESSAGE_BYTES
    }
}

/// Typed protocol error: never a panic, never a silent truncation (P2.4).
fn protocol_error(msg: &str) -> SiftError {
    SiftError::app("imap_protocol", msg.to_string(), true)
}

fn is_conn_error(e: &SiftError) -> bool {
    matches!(e, SiftError::Io(_))
        || matches!(e, SiftError::App { code, .. } if code == "offline" || code == "imap_transient" || code == "imap_protocol")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p11_conn_literal_framing_shapes() {
        assert_eq!(
            trailing_literal(b"* 1 FETCH (UID 1 BODY[] {12}\r\n"),
            Trailing::Len(12)
        );
        assert_eq!(
            trailing_literal(b"* 1 FETCH (UID 1 BODY[] {12+}\r\n"),
            Trailing::Len(12)
        );
        assert_eq!(trailing_literal(b"a001 OK done\r\n"), Trailing::None);
        assert_eq!(trailing_literal(b"* OK hello\r\n"), Trailing::None);
        // A declared length that cannot be represented is a typed error, not
        // a silent "incomplete".
        assert_eq!(
            trailing_literal(b"* 1 FETCH (UID 1 BODY[] {99999999999999999999999}\r\n"),
            Trailing::Overflow
        );
        // Header literals are capped at the metadata cap; whole-message at
        // the message cap.
        assert_eq!(
            literal_cap_for(b"* 1 FETCH (UID 1 BODY[HEADER.FIELDS (FROM)] {5}\r\n"),
            MAX_FRAME_BYTES
        );
        assert_eq!(
            literal_cap_for(b"* 1 FETCH (UID 1 BODY[] {5}\r\n"),
            MAX_WHOLE_MESSAGE_BYTES
        );
    }

    #[test]
    fn p11_conn_quote() {
        assert_eq!(imap_quote("INBOX"), "\"INBOX\"");
        assert_eq!(
            imap_quote("[Gmail]/Alle Nachrichten"),
            "\"[Gmail]/Alle Nachrichten\""
        );
        assert_eq!(imap_quote("a\"b\\c"), "\"a\\\"b\\\\c\"");
    }
}
