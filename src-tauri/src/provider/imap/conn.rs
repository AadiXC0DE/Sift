//! IMAP connection management (Phase 11 task 4).
//!
//! Exactly two connections per account: `worker` (all sync/fetch/op traffic)
//! and the idle slot owned by `idle.rs`. TLS via the platform verifier
//! (macOS keychain); plaintext TCP to loopback is test-only.
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
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{Mutex as TokioMutex, RwLock};

const BACKOFFS: [u64; 5] = [1, 2, 5, 15, 60];
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const KEEPALIVE_IDLE: Duration = Duration::from_secs(5 * 60);

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

#[derive(Debug)]
pub struct SelectInfo {
    pub exists: u32,
    pub uidvalidity: u32,
    pub uidnext: u32,
    pub highestmodseq: Option<u64>,
    pub readonly: bool,
}

pub struct TaggedOutcome {
    pub ok: bool,
    pub code: Option<ResponseCode>,
    pub text: String,
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

pub struct Conn {
    io: BufReader<Box<dyn ImapStream>>,
    inner: Arc<PoolInner>,
    tag: u32,
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
    /// as needed. The lock is never held across network I/O here.
    pub async fn worker(&self) -> Result<WorkerGuard, SiftError> {
        loop {
            let mut guard = self.worker.clone().lock_owned().await;
            let have_conn = guard.is_some();
            if have_conn {
                let idle_for = self.inner.last_used.lock().unwrap().elapsed();
                if idle_for <= KEEPALIVE_IDLE {
                    return Ok(WorkerGuard { guard });
                }
                // NOOP keep-alive when the connection sat idle (spec task 4).
                // A failed keepalive drops the connection; loop reconnects.
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
            drop(guard);
            let conn = self.connect_loop().await?;
            *self.worker.lock().await = Some(conn);
        }
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
        }
    }

    fn plain(stream: tokio::net::TcpStream, inner: Arc<PoolInner>) -> Self {
        Self {
            io: BufReader::new(Box::new(stream)),
            inner,
            tag: 0,
        }
    }

    fn next_tag(&mut self) -> String {
        self.tag += 1;
        format!("s{:04}", self.tag)
    }

    /// Rebuild the transport + re-login, preserving the tag counter.
    async fn reconnect(&mut self) -> Result<(), SiftError> {
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
        loop {
            // Read one line (or fail on timeout/close).
            let mut line = vec![];
            let read_line = async {
                loop {
                    let b = self.io.read_u8().await?;
                    line.push(b);
                    if line.ends_with(b"\r\n") {
                        break;
                    }
                    if line.len() > 8 * 1024 * 1024 {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            "line too long",
                        ));
                    }
                }
                Ok::<(), std::io::Error>(())
            };
            tokio::time::timeout(CONNECT_TIMEOUT, read_line)
                .await
                .map_err(|_| SiftError::app("offline", "No connection to Gmail.", true))?
                .map_err(errors::io_err)?;
            buf.extend_from_slice(&line);
            // Splice `{n}` / `{n+}` literals inline so the parser sees whole responses.
            if let Some(n) = trailing_literal_len(&line) {
                // NOTE: no CRLF follows literal bytes on the wire — the
                // enclosing `)` (or the next response) comes immediately.
                let mut data = vec![0u8; n];
                tokio::time::timeout(CONNECT_TIMEOUT, self.io.read_exact(&mut data))
                    .await
                    .map_err(|_| SiftError::app("offline", "No connection to Gmail.", true))?
                    .map_err(errors::io_err)?;
                buf.extend_from_slice(&data);
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
                    return Err(SiftError::app(
                        "imap_protocol",
                        format!("Could not parse Gmail response: {msg}"),
                        true,
                    ));
                }
            }
        }
    }

    /// Send one command; gather untagged responses until the tagged completion.
    /// On connection loss: rebuild once and retry the command once.
    pub async fn cmd(&mut self, body: &str) -> Result<CommandResult, SiftError> {
        match self.cmd_once(body).await {
            Err(e) if is_conn_error(&e) => {
                log::debug!(target: "sift::imap", "connection lost mid-command; reconnecting once");
                // Boxed: reconnect() can reach connect_loop(), which reaches
                // back here — boxing breaks the async type recursion.
                Box::pin(self.reconnect()).await?;
                self.cmd_once(body).await
            }
            other => other,
        }
    }

    async fn cmd_once(&mut self, body: &str) -> Result<CommandResult, SiftError> {
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
        let r = self.cmd("NOOP").await?;
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
        let verb = if readonly { "EXAMINE" } else { "SELECT" };
        let r = self
            .cmd(&format!("{verb} {}", quote_folder(folder)))
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
        Ok(info)
    }

    /// LIST with SPECIAL-USE return options. Returns (attributes,
    /// delimiter, raw server name) per folder; see folders::map_folders.
    pub async fn list_special(
        &mut self,
    ) -> Result<Vec<(Vec<String>, Option<String>, String)>, SiftError> {
        let r = self.cmd("LIST \"\" \"*\" RETURN (SPECIAL-USE)").await?;
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

    /// Raw UID FETCH for Gmail items; callers decode via proto. Returns
    /// (sequence, attributes) in server order.
    pub async fn uid_fetch(
        &mut self,
        uid_set: &str,
        items: &str,
    ) -> Result<Vec<(u32, Vec<super::proto::FetchAttr>)>, SiftError> {
        let r = self.cmd(&format!("UID FETCH {uid_set} {items}")).await?;
        if !r.tagged.ok {
            super::errors::map_response("UID FETCH", &r.tagged.text)?;
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
        let r = self.cmd("UID SEARCH ALL").await?;
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

    /// Flag/label delta since a modseq (CONDSTORE). Empty vec when the
    /// server reports nothing newer.
    pub async fn uid_fetch_changed(
        &mut self,
        modseq: u64,
    ) -> Result<Vec<(u32, Vec<super::proto::FetchAttr>)>, SiftError> {
        let r = self
            .cmd(&format!(
                "UID FETCH 1:* (UID FLAGS X-GM-LABELS X-GM-THRID MODSEQ) (CHANGEDSINCE {modseq})"
            ))
            .await?;
        if !r.tagged.ok {
            super::errors::map_response("UID FETCH", &r.tagged.text)?;
        }
        let mut out = vec![];
        for u in r.untagged {
            if let Response::Untagged(Untagged::Fetch { seq, attrs }) = u {
                out.push((seq, attrs));
            }
        }
        Ok(out)
    }

    /// UIDs in the selected folder with a given Gmail message id.
    pub async fn uid_search_gmmsgid(&mut self, msgid: u64) -> Result<Vec<u32>, SiftError> {
        let r = self.cmd(&format!("UID SEARCH X-GM-MSGID {msgid}")).await?;
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
        let r = self.cmd(&format!("UID SEARCH X-GM-RAW \"{q}\"")).await?;
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
            .cmd(&format!(
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
fn trailing_literal_len(line: &[u8]) -> Option<usize> {
    let line = line.strip_suffix(b"\r\n").unwrap_or(line);
    let open = line.iter().rposition(|&b| b == b'{')?;
    let inner = std::str::from_utf8(&line[open + 1..]).ok()?;
    let inner = inner.strip_suffix('}')?;
    let num = inner.strip_suffix('+').unwrap_or(inner);
    // A bare number; anything else (e.g. command echoes) is not a literal.
    if num.bytes().all(|b| b.is_ascii_digit()) && !num.is_empty() {
        num.parse().ok()
    } else {
        None
    }
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
            trailing_literal_len(b"* 1 FETCH (UID 1 BODY[] {12}\r\n"),
            Some(12)
        );
        assert_eq!(
            trailing_literal_len(b"* 1 FETCH (UID 1 BODY[] {12+}\r\n"),
            Some(12)
        );
        assert_eq!(trailing_literal_len(b"a001 OK done\r\n"), None);
        assert_eq!(trailing_literal_len(b"* OK hello\r\n"), None);
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
