//! Stateful fake Gmail IMAP server (Phase 11 task 15, test-only).
//!
//! Plaintext TCP on 127.0.0.1:0. Implements exactly the Appendix H subset:
//! CAPABILITY ID LOGIN ENABLE LIST(SELECT-USE) SELECT EXAMINE STATUS
//! UID SEARCH (ALL, X-GM-MSGID, X-GM-RAW-substring) UID FETCH (all task-6
//! items incl. CHANGEDSINCE + partials) UID STORE UID MOVE UID COPY
//! UID EXPUNGE CREATE APPEND(+APPENDUID) IDLE/DONE NOOP LOGOUT.
//! Seeded from `fixtures/mailbox-imap-2k.json`; failures scripted by
//! password value or explicit flags (see [`Behavior`]).
use sift::provider::imap::proto::decode_utf7_mailbox;
use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedReadHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

/// One message, present in one or more folders (Trash/Junk copies share msgid).
#[derive(Debug, Clone)]
pub struct FMsg {
    pub msgid: u64,
    pub thrid: u64,
    pub folders: HashMap<String, u32>,
    pub flags: Vec<String>,
    pub labels: Vec<String>,
    pub modseq: u64,
    pub from_name: String,
    pub from_email: String,
    pub subject: String,
    pub date_offset_h: i64,
    pub size: u32,
    pub has_att: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Behavior {
    /// First N post-login commands fail with [UNAVAILABLE].
    pub transient_first_n: usize,
    /// Close the TCP connection after N commands (reconnect test).
    pub drop_after_n: Option<usize>,
    /// Next SELECT reports uidvalidity+1 (NeedsFull test).
    pub uidvalidity_bump: bool,
    /// Omit CONDSTORE (fallback-scan test).
    pub no_condstore: bool,
    /// Omit IDLE from caps.
    pub no_idle: bool,
    /// Omit UIDPLUS from caps: a server that cannot do a targeted EXPUNGE.
    pub no_uidplus: bool,

    // -- P1.2 strictness and failure injection -------------------------------
    /// Split every response into this many TCP writes (literal fragmentation).
    pub fragmented_literals: usize,
    /// Delay the tagged completion by this many milliseconds.
    pub completion_delay_ms: u64,
    /// FETCH answers with a row for a DIFFERENT uid than the requested one.
    pub wrong_uid_response: bool,
    /// A section FETCH answers with a zero-length literal (`{0}`).
    pub empty_section: bool,
    /// A section FETCH answers with the row but WITHOUT the BODY[...] data.
    pub omit_section_data: bool,
    /// The next FETCH fails with a tagged BAD (syntax), never retried.
    pub fetch_bad: bool,
    /// The next FETCH fails with a transient tagged NO.
    pub fetch_no: bool,
    /// Close the socket immediately after a successful SELECT/EXAMINE.
    pub disconnect_after_select: bool,
    /// Apply the next mutation and close WITHOUT its tagged completion.
    pub disconnect_after_mutation: bool,
    /// A partial section FETCH declares a literal far larger than requested
    /// (and sends no bytes): the client must refuse the declaration (P2.4).
    pub oversized_partial_literal: bool,
    /// A partial section FETCH sends only the literal head, then the socket
    /// closes: the client must surface a typed error, never truncated bytes.
    pub drop_in_section_literal: bool,
    /// Serve a whole-message `BODY[]` of at least this many bytes (proves the
    /// 8 MiB framing cap does not bound a legitimate message literal).
    pub large_full_message_bytes: Option<usize>,
}

#[derive(Debug, Default)]
pub struct State {
    pub msgs: HashMap<u64, FMsg>,
    pub next_uid: HashMap<String, u32>,
    pub uidvalidity: HashMap<String, u32>,
    pub modseq: u64,
    pub folders: HashMap<String, String>,
    pub behavior: Behavior,
    /// Sanitized command structures (`VERB rest`); credentials never appear.
    pub commands_seen: Vec<String>,
    /// Every UID FETCH in arrival order as `(selected mailbox, uid set)`, so a
    /// test can prove each command ran against its intended folder (P4.3).
    pub fetches: Vec<(String, String)>,
    /// Post-SELECT pause barrier (P4.3): the handler parks on `pause_rx` after
    /// answering a SELECT, and notifies `paused`.
    pub pause_tx: Option<tokio::sync::oneshot::Sender<()>>,
    pub pause_rx: Option<tokio::sync::oneshot::Receiver<()>>,
    pub paused: Arc<tokio::sync::Notify>,
    pub idle_txs: Vec<mpsc::UnboundedSender<String>>,
    pub cmd_count: usize,
    /// Byte-exact section payloads for attachment protocol tests, keyed by
    /// (msgid, section). When present they replace the generated payload and
    /// are served unmodified (binary-safe).
    pub section_bytes: HashMap<(u64, String), Vec<u8>>,
    /// Explicit BODYSTRUCTURE text per msgid (P2.7 single-part/binary cases).
    pub bodystructure_overrides: HashMap<u64, String>,
    /// Number of TCP connections accepted so far (connection-cap tests).
    pub connections: usize,
}

pub struct FakeGmail {
    pub addr: SocketAddr,
    pub state: Arc<Mutex<State>>,
    _task: tokio::task::JoinHandle<()>,
}

impl FakeGmail {
    pub async fn start() -> Self {
        Self::start_with(State::default()).await
    }

    pub async fn start_with(mut init: State) -> Self {
        Self::seed(&mut init);
        let state = Arc::new(Mutex::new(init));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let s2 = state.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    break;
                };
                s2.lock().unwrap().connections += 1;
                let st = s2.clone();
                tokio::spawn(async move { handle(sock, st).await });
            }
        });
        Self {
            addr,
            state,
            _task: task,
        }
    }

    fn seed(st: &mut State) {
        let data = std::fs::read_to_string(format!(
            "{}/../fixtures/mailbox-imap-2k.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let v: serde_json::Value = serde_json::from_str(&data).unwrap();
        let folders = v["folders"].as_object().unwrap();
        for (role, name) in folders {
            st.folders
                .insert(role.clone(), name.as_str().unwrap().to_string());
            st.next_uid.insert(role.clone(), 1);
            st.uidvalidity.insert(role.clone(), 987654);
        }
        st.modseq = 89123;
        for m in v["messages"].as_array().unwrap() {
            let msgid = m["msgid"].as_u64().unwrap();
            let folder = m["folder"].as_str().unwrap().to_string();
            let uid = *st.next_uid.get(&folder).unwrap();
            st.next_uid.insert(folder.clone(), uid + 1);
            let mut folders = HashMap::new();
            folders.insert(folder, uid);
            st.modseq += 1;
            let ms = st.modseq;
            st.msgs.insert(
                msgid,
                FMsg {
                    msgid,
                    thrid: m["thrid"].as_u64().unwrap(),
                    folders,
                    flags: m["flags"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| x.as_str().unwrap().to_string())
                        .collect(),
                    labels: m["labels"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|x| x.as_str().unwrap().to_string())
                        .collect(),
                    modseq: ms,
                    from_name: m["from_name"].as_str().unwrap().to_string(),
                    from_email: m["from_email"].as_str().unwrap().to_string(),
                    subject: m["subject"].as_str().unwrap().to_string(),
                    date_offset_h: m["date_offset_h"].as_i64().unwrap(),
                    size: m["size"].as_u64().unwrap() as u32,
                    has_att: m["has_att"].as_bool().unwrap(),
                },
            );
        }
    }

    /// Serve exact bytes for one `(msgid, section)` on section FETCHes.
    ///
    /// Used by the attachment protocol tests to pin zero-byte payloads,
    /// embedded NULs and bytes above 0x7f without any String round-trip.
    pub fn set_section_bytes(&self, msgid: u64, section: &str, bytes: Vec<u8>) {
        let mut st = self.state.lock().unwrap();
        st.section_bytes.insert((msgid, section.to_string()), bytes);
    }

    /// Serve an explicit BODYSTRUCTURE for one message (P2.7).
    pub fn set_bodystructure(&self, msgid: u64, text: &str) {
        let mut st = self.state.lock().unwrap();
        st.bodystructure_overrides.insert(msgid, text.to_string());
    }

    /// Sanitized command log (credentials are never recorded).
    pub fn commands(&self) -> Vec<String> {
        self.state.lock().unwrap().commands_seen.clone()
    }

    /// `(selected mailbox, uid set)` for every UID FETCH so far (P4.3).
    pub fn fetches(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().fetches.clone()
    }

    /// Suspend the connection right after it answers a SELECT/EXAMINE, until
    /// [`FakeGmail::release_paused`] runs: a deterministic barrier for the
    /// P4.3 interleave test.
    pub fn pause_after_select(&self) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut st = self.state.lock().unwrap();
        st.pause_tx = Some(tx);
        st.pause_rx = Some(rx);
    }

    /// Wait until the paused SELECT has been answered and the server parked.
    pub async fn wait_paused(&self) {
        let notify = self.state.lock().unwrap().paused.clone();
        let _ = tokio::time::timeout(std::time::Duration::from_secs(10), notify.notified()).await;
    }

    /// Answer a SELECT paused by [`FakeGmail::pause_after_select`].
    pub fn release_paused(&self) {
        let tx = self.state.lock().unwrap().pause_tx.take();
        if let Some(tx) = tx {
            let _ = tx.send(());
        }
    }

    /// Commands matching a prefix, in arrival order.
    pub fn commands_with_prefix(&self, prefix: &str) -> Vec<String> {
        self.commands()
            .into_iter()
            .filter(|c| c.starts_with(prefix))
            .collect()
    }

    /// Deliver a brand-new message to a folder (IDLE/poll tests).
    pub fn deliver(
        &self,
        folder: &str,
        from: &str,
        subject: &str,
        labels: Vec<String>,
        unread: bool,
    ) -> u64 {
        let mut st = self.state.lock().unwrap();
        let msgid = st.msgs.keys().max().copied().unwrap_or(0) + 7919;
        st.modseq += 1;
        let ms_seed = st.modseq;
        let uid = *st.next_uid.get(folder).unwrap_or(&1);
        st.next_uid.insert(folder.into(), uid + 1);
        let mut flags = vec![];
        if !unread {
            flags.push("\\Seen".to_string());
        }
        let mut folders = HashMap::new();
        folders.insert(folder.into(), uid);
        st.msgs.insert(
            msgid,
            FMsg {
                msgid,
                thrid: msgid,
                folders,
                flags,
                labels,
                modseq: ms_seed,
                from_name: from.into(),
                from_email: format!("{}@example.com", from.to_lowercase()),
                subject: subject.into(),
                date_offset_h: 0,
                size: 4096,
                has_att: false,
            },
        );
        let exists = st
            .msgs
            .values()
            .filter(|m| m.folders.contains_key(folder))
            .count();
        let line = format!("* {exists} EXISTS");
        st.idle_txs.retain(|tx| tx.send(line.clone()).is_ok());
        msgid
    }

    pub fn set_flag(&self, msgid: u64, flag: &str, on: bool) {
        let mut st = self.state.lock().unwrap();
        st.modseq += 1;
        let ms = st.modseq;
        if let Some(m) = st.msgs.get_mut(&msgid) {
            if on {
                if !m.flags.iter().any(|f| f == flag) {
                    m.flags.push(flag.into());
                }
            } else {
                m.flags.retain(|f| f != flag);
            }
            m.modseq = ms;
        }
    }

    pub fn uids_in(&self, folder: &str) -> Vec<u32> {
        let st = self.state.lock().unwrap();
        let mut v: Vec<u32> = st
            .msgs
            .values()
            .filter_map(|m| m.folders.get(folder).copied())
            .collect();
        v.sort_unstable();
        v
    }
}

fn internaldate(offset_h: i64) -> String {
    // Fixed reference clock (deterministic across runs).
    let base: i64 = 1786526400; // 2026-08-12T12:00:00Z
    let ts = base - offset_h * 3600;
    let days = ts.div_euclid(86400);
    // 2026-08-12 is a Wednesday; compute weekday by offset from known date.
    let dow = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][(days % 7 + 7) as usize % 7];
    // Convert days-since-epoch to civil date (Howard Hinnant algorithm).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let mon = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ][(m - 1) as usize];
    let sod = (ts.rem_euclid(86400)) as u32;
    format!(
        "{dow}, {d:02} {mon} {y} {:02}:{:02}:{:02} +0000",
        sod / 3600,
        sod / 60 % 60,
        sod % 60
    )
}

/// Recorded command structure. Credentials never appear (P1.2).
fn sanitize_command(verb: &str, rest: &str) -> String {
    if verb.eq_ignore_ascii_case("LOGIN") || verb.eq_ignore_ascii_case("AUTHENTICATE") {
        return format!("{verb} <redacted>");
    }
    format!("{verb} {rest}")
}

fn quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn gm_label_list(labels: &[String]) -> String {
    let parts: Vec<String> = labels
        .iter()
        .map(|l| {
            if l.starts_with('\\') {
                l.clone()
            } else {
                quote(l)
            }
        })
        .collect();
    format!("({})", parts.join(" "))
}

fn bodystructure(m: &FMsg) -> String {
    let text = |sub: &str, size: u32, lines: u32| {
        format!("(\"text\" \"{sub}\" (\"charset\" \"utf-8\") NIL NIL \"quoted-printable\" {size} {lines} NIL NIL NIL NIL)")
    };
    if !m.has_att {
        return format!(
            "({} {} \"alternative\" (\"boundary\" \"bnd\") NIL NIL NIL)",
            text("plain", 300, 10),
            text("html", 1200, 30)
        );
    }
    if m.msgid % 3 == 0 {
        format!(
            "({} (\"application\" \"pdf\" (\"name\" \"doc.pdf\") NIL NIL \"base64\" 2100 NIL (\"attachment\" (\"filename\" \"doc.pdf\")) NIL NIL) \"mixed\" (\"boundary\" \"bnd\") NIL NIL NIL)",
            text("html", 900, 22)
        )
    } else {
        format!(
            "({} (\"image\" \"png\" (\"name\" \"img.png\") \"<ii_{}>\" NIL \"base64\" 4520 NIL NIL NIL NIL) \"mixed\" (\"boundary\" \"bnd\") NIL NIL NIL)",
            text("html", 900, 22),
            m.msgid % 1000
        )
    }
}

fn header_block(m: &FMsg) -> String {
    let mut h = format!(
        "From: {} <{}>\r\nTo: me@example.com\r\nSubject: {}\r\nDate: {}\r\nMessage-ID: <{}@example.com>\r\n",
        m.from_name, m.from_email, m.subject,
        internaldate(m.date_offset_h),
        m.msgid
    );
    if m.msgid % 7 == 0 {
        h.push_str("List-Unsubscribe: <https://example.com/unsub>, <mailto:leave@example.com>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n");
    }
    h.push_str("\r\n");
    h
}

/// Resolve a UID set against the folder's ACTUAL UIDs. Ranges are matched by
/// membership, never expanded: `1:*` against a UIDNEXT of a billion must not
/// materialize a billion entries (the P4.5 sparse-scan test depends on the
/// fake behaving like a real server here).
fn parse_uid_set(set: &str, uids: &[u32]) -> Vec<u32> {
    let mut out: Vec<u32> = uids
        .iter()
        .copied()
        .filter(|u| uid_in_set(*u, set))
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

#[allow(dead_code)]
fn parse_uid_set_unbounded(set: &str, max: u32) -> Vec<u32> {
    let mut out = HashSet::new();
    for part in set.split(',') {
        let part = part.trim();
        if let Some((a, b)) = part.split_once(':') {
            let (a, b): (u32, u32) = (
                a.parse().unwrap_or(1),
                if b == "*" {
                    max
                } else {
                    b.parse().unwrap_or(max)
                },
            );
            let (lo, hi) = (a.min(b), a.max(b));
            for u in lo..=hi.min(max.max(1)) {
                out.insert(u);
            }
        } else if let Ok(u) = part.parse::<u32>() {
            out.insert(u);
        }
    }
    let mut v: Vec<u32> = out.into_iter().collect();
    v.sort_unstable();
    v
}

async fn read_line(r: &mut BufReader<OwnedReadHalf>, buf: &mut Vec<u8>) -> std::io::Result<usize> {
    buf.clear();
    r.read_until(b'\n', buf).await
}

async fn handle(sock: TcpStream, state: Arc<Mutex<State>>) {
    let (rh, wh) = sock.into_split();
    let mut r = BufReader::new(rh);
    let mut w = wh;
    let mut selected: Option<String> = None;
    let mut logged_in = false;
    let mut buf = vec![];

    // Greeting.
    w.write_all(b"* OK fake-gmail ready\r\n").await.unwrap();

    loop {
        if read_line(&mut r, &mut buf).await.unwrap_or(0) == 0 {
            return;
        }
        let line = String::from_utf8_lossy(&buf).trim_end().to_string();
        if line.is_empty() {
            continue;
        }
        // IDLE data phase is handled inline below; DONE arrives as a line.
        let mut parts = line.splitn(3, ' ');
        let tag = parts.next().unwrap_or("").to_string();
        let verb = parts.next().unwrap_or("").to_ascii_uppercase();
        let rest = parts.next().unwrap_or("").to_string();
        enum Pre {
            Transient,
            Drop,
            Proceed,
        }
        let pre = {
            let mut st = state.lock().unwrap();
            // Sanitized structure only: LOGIN credentials and literal bodies
            // must never enter the recorded log (P1.2).
            st.commands_seen.push(sanitize_command(&verb, &rest));
            st.cmd_count += 1;
            if st.behavior.transient_first_n > 0 {
                st.behavior.transient_first_n -= 1;
                Pre::Transient
            } else if st
                .behavior
                .drop_after_n
                .map(|n| st.cmd_count >= n)
                .unwrap_or(false)
            {
                // One-shot: a single dropped connection models a transient
                // blip; the client must reconnect and resume IDLE.
                st.behavior.drop_after_n = None;
                Pre::Drop
            } else {
                Pre::Proceed
            }
        };
        match pre {
            Pre::Transient => {
                let _ = w
                    .write_all(
                        format!("{tag} NO [UNAVAILABLE] Temporary System Problem. Please try again later.\r\n")
                            .as_bytes(),
                    )
                    .await;
                continue;
            }
            Pre::Drop => return,
            Pre::Proceed => {}
        }
        macro_rules! no {
            ($t:expr) => {
                w.write_all(format!("{tag} NO {}\r\n", $t).as_bytes())
                    .await
                    .unwrap();
                continue;
            };
        }
        match verb.as_str() {
            "CAPABILITY" => {
                let caps = {
                    let st = state.lock().unwrap();
                    let mut caps =
                        "IMAP4rev1 UNSELECT IDLE NAMESPACE QUOTA ID XLIST CHILDREN X-GM-EXT-1 UIDPLUS MOVE IDLE SPECIAL-USE ESEARCH UTF8=ACCEPT LIST-EXTENDED LIST-STATUS LITERAL+ APPENDLIMIT=35651584".to_string();
                    if !st.behavior.no_condstore {
                        caps.push_str(" CONDSTORE ENABLE");
                    }
                    if st.behavior.no_idle {
                        caps = caps.replace(" IDLE", "");
                    }
                    if st.behavior.no_uidplus {
                        caps = caps.replace(" UIDPLUS", "");
                    }
                    caps
                };
                w.write_all(format!("* CAPABILITY {caps}\r\n{tag} OK done\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            "ID" => {
                w.write_all(
                    format!("* ID (\"name\" \"fake\" \"version\" \"1\")\r\n{tag} OK done\r\n")
                        .as_bytes(),
                )
                .await
                .unwrap();
            }
            "LOGIN" => {
                // LOGIN <email> <password>; password may be quoted.
                let args = split_args(&rest);
                let pw = args.get(1).cloned().unwrap_or_default();
                let resp: Option<String> = match pw.as_str() {
                    "badpassword000000" => Some("[AUTHENTICATIONFAILED] Invalid credentials (Failure)".into()),
                    "needapppassword0" => Some("[ALERT] Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)".into()),
                    "weblogin00000000" => Some("[ALERT] Please log in via your web browser: https://support.google.com/mail/accounts/answer/78754 (Failure)".into()),
                    "disabled00000000" => Some("[ALERT] IMAP access is disabled for your domain. Please contact your domain administrator. (Failure)".into()),
                    "manyconns0000000" => Some("Too many simultaneous connections. (Failure)".into()),
                    _ => None,
                };
                if let Some(text) = resp {
                    w.write_all(format!("{tag} NO {text}\r\n").as_bytes())
                        .await
                        .unwrap();
                } else {
                    logged_in = true;
                    w.write_all(format!("{tag} OK done\r\n").as_bytes())
                        .await
                        .unwrap();
                }
            }
            "ENABLE" => {
                w.write_all(format!("* ENABLED CONDSTORE\r\n{tag} OK done\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            "LIST" => {
                if !logged_in {
                    no!("LOGIN first");
                }
                let out = {
                    let st = state.lock().unwrap();
                    let roles = ["inbox", "all", "trash", "junk", "drafts", "sent"];
                    let mut out = String::new();
                    out.push_str("* LIST (\\HasNoChildren) \"/\" \"INBOX\"\r\n");
                    for role in roles.iter().skip(1) {
                        let name = st.folders.get(*role).cloned().unwrap_or_default();
                        let attr = match *role {
                            "all" => "\\All",
                            "trash" => "\\Trash",
                            "junk" => "\\Junk",
                            "drafts" => "\\Drafts",
                            "sent" => "\\Sent",
                            _ => "",
                        };
                        out.push_str(&format!(
                            "* LIST (\\HasNoChildren {attr}) \"/\" {}\r\n",
                            quote(&name)
                        ));
                    }
                    // [Gmail] container (noselect) + a user label folder
                    out.push_str("* LIST (\\HasChildren \\Noselect) \"/\" \"[Gmail]\"\r\n");
                    out.push_str("* LIST (\\HasNoChildren) \"/\" \"Receipts\"\r\n");
                    out.push_str(&format!("{tag} OK done\r\n"));
                    out
                };
                w.write_all(out.as_bytes()).await.unwrap();
            }
            "SELECT" | "EXAMINE" => {
                if !logged_in {
                    no!("LOGIN first");
                }
                let folder = unquote(rest.trim());
                let role = role_of(&state, &folder);
                let (uids, max_uid, uidv, cond, ms) = {
                    let mut st = state.lock().unwrap();
                    let uids: Vec<u32> = st
                        .msgs
                        .values()
                        .filter_map(|m| m.folders.get(&role).copied())
                        .collect();
                    let max_uid = uids.iter().max().copied().unwrap_or(0);
                    let mut uidv = *st.uidvalidity.get(&role).unwrap_or(&987654);
                    if st.behavior.uidvalidity_bump {
                        uidv += 1;
                        st.behavior.uidvalidity_bump = false;
                    }
                    let cond = !st.behavior.no_condstore;
                    let ms = st.modseq;
                    (uids, max_uid, uidv, cond, ms)
                };
                selected = Some(role);
                let mut out = format!(
                    "* FLAGS (\\Answered \\Flagged \\Draft \\Deleted \\Seen)\r\n* OK [PERMANENTFLAGS (\\Answered \\Flagged \\Draft \\Deleted \\Seen \\*)] Flags permitted.\r\n* OK [UIDVALIDITY {uidv}] UIDs valid.\r\n* OK [UIDNEXT {}] Predicted next UID.\r\n",
                    max_uid + 1
                );
                if cond {
                    out.push_str(&format!("* OK [HIGHESTMODSEQ {ms}] Highest.\r\n"));
                }
                out.push_str(&format!(
                    "* {} EXISTS\r\n* 0 RECENT\r\n* OK [READ-{}] Select completed.\r\n{tag} OK done\r\n",
                    uids.len(),
                    if verb == "SELECT" { "WRITE" } else { "ONLY" }
                ));
                let drop_now = {
                    let mut st = state.lock().unwrap();
                    let d = st.behavior.disconnect_after_select;
                    st.behavior.disconnect_after_select = false;
                    d
                };
                write_out(&mut w, out.as_bytes(), &state).await;
                // Deterministic barrier (P4.3): the SELECT is answered and the
                // connection parks before it reads the next command.
                let rx = state.lock().unwrap().pause_rx.take();
                if let Some(rx) = rx {
                    state.lock().unwrap().paused.notify_one();
                    let _ = tokio::time::timeout(std::time::Duration::from_secs(30), rx).await;
                }
                if drop_now {
                    // Selected state is established, then the socket dies
                    // before the next command (P1.3 recovery test).
                    return;
                }
            }
            "STATUS" => {
                // STATUS <folder> (...)
                let folder = unquote(rest.split_whitespace().next().unwrap_or(""));
                let role = role_of(&state, &folder);
                let resp = {
                    let st = state.lock().unwrap();
                    let uids: Vec<u32> = st
                        .msgs
                        .values()
                        .filter_map(|m| m.folders.get(&role).copied())
                        .collect();
                    let max_uid = uids.iter().max().copied().unwrap_or(0);
                    let uidv = *st.uidvalidity.get(&role).unwrap_or(&987654);
                    let ms = st.modseq;
                    let cond = if st.behavior.no_condstore {
                        String::new()
                    } else {
                        format!(" HIGHESTMODSEQ {ms}")
                    };
                    format!(
                        "* STATUS {} (MESSAGES {} UIDNEXT {} UIDVALIDITY {uidv}{cond})\r\n{tag} OK done\r\n",
                        quote(&folder),
                        uids.len(),
                        max_uid + 1
                    )
                };
                w.write_all(resp.as_bytes()).await.unwrap();
            }
            "UID" => {
                if !logged_in {
                    no!("LOGIN first");
                }
                let mut sub = rest.splitn(2, ' ');
                let subverb = sub.next().unwrap_or("").to_ascii_uppercase();
                let subrest = sub.next().unwrap_or("").to_string();
                // P1.2: selected-state operations before SELECT/EXAMINE are a
                // protocol error. There is no lenient mode any more: a test
                // that forgets to select must fail here (P4.3 migrated the
                // last callers).
                let sel = match selected.clone() {
                    Some(s) => s,
                    None => {
                        write_out(
                            &mut w,
                            format!("{tag} BAD no mailbox selected\r\n").as_bytes(),
                            &state,
                        )
                        .await;
                        continue;
                    }
                };
                match subverb.as_str() {
                    "SEARCH" => {
                        let uids = uid_search(&state, &sel, &subrest);
                        let list = uids
                            .iter()
                            .map(|u| u.to_string())
                            .collect::<Vec<_>>()
                            .join(" ");
                        write_out(
                            &mut w,
                            format!(
                                "* SEARCH{sep}{list}\r\n{tag} OK done\r\n",
                                sep = if list.is_empty() { "" } else { " " },
                                list = list
                            )
                            .as_bytes(),
                            &state,
                        )
                        .await;
                    }
                    "FETCH" => {
                        // <set> <items> [CHANGEDSINCE n]; strict validation.
                        let (set, items, since) = match parse_fetch_args(&subrest) {
                            Ok(v) => v,
                            Err(e) => {
                                write_out(&mut w, format!("{tag} BAD {e}\r\n").as_bytes(), &state)
                                    .await;
                                continue;
                            }
                        };
                        // Which mailbox this FETCH actually ran against
                        // (P4.3 interleave tests).
                        state
                            .lock()
                            .unwrap()
                            .fetches
                            .push((sel.clone(), set.clone()));
                        let (bad, transient) = {
                            let mut st = state.lock().unwrap();
                            let bad = st.behavior.fetch_bad;
                            st.behavior.fetch_bad = false;
                            let t = st.behavior.fetch_no;
                            st.behavior.fetch_no = false;
                            (bad, t)
                        };
                        if bad {
                            write_out(
                                &mut w,
                                format!("{tag} BAD syntax error at FETCH\r\n").as_bytes(),
                                &state,
                            )
                            .await;
                            continue;
                        }
                        if transient {
                            write_out(
                                &mut w,
                                format!("{tag} NO [UNAVAILABLE] Temporary System Problem.\r\n")
                                    .as_bytes(),
                                &state,
                            )
                            .await;
                            continue;
                        }
                        let mut out = uid_fetch(&state, &sel, &set, &items, since);
                        // Mid-stream drop (P2.4): send the literal head, then
                        // close without the bytes or completion.
                        let drop_now = {
                            let st = state.lock().unwrap();
                            st.behavior.drop_in_section_literal
                                && items.to_ascii_uppercase().contains("BODY.PEEK[")
                                && items.contains('<')
                        };
                        if drop_now {
                            write_out(&mut w, &out, &state).await;
                            return;
                        }
                        out.extend_from_slice(format!("{tag} OK done\r\n").as_bytes());
                        write_out(&mut w, &out, &state).await;
                    }
                    "STORE" => {
                        // <set> +/-FLAGS.SILENT (...) / +/-X-GM-LABELS (...)
                        let dropped = {
                            let mut st = state.lock().unwrap();
                            let d = st.behavior.disconnect_after_mutation;
                            st.behavior.disconnect_after_mutation = false;
                            d
                        };
                        let (n, changed) = uid_store(&state, &sel, &subrest);
                        let _ = (n, changed);
                        if dropped {
                            // Applied, but the completion never arrives: the
                            // client must not replay the mutation (P1.3).
                            return;
                        }
                        write_out(&mut w, format!("{tag} OK done\r\n").as_bytes(), &state).await;
                    }
                    "MOVE" => {
                        // <set> <dest>
                        let mut it = subrest.splitn(2, ' ');
                        let set = it.next().unwrap_or("");
                        let dest = unquote(it.next().unwrap_or("").trim());
                        let dest_role = role_of(&state, &dest);
                        let dropped = {
                            let mut st = state.lock().unwrap();
                            let d = st.behavior.disconnect_after_mutation;
                            st.behavior.disconnect_after_mutation = false;
                            d
                        };
                        uid_move(&state, &sel, &dest_role, set);
                        if dropped {
                            // The mutation may have been accepted; the client
                            // must not blindly replay it (P1.3).
                            return;
                        }
                        w.write_all(format!("{tag} OK done\r\n").as_bytes())
                            .await
                            .unwrap();
                    }
                    "COPY" => {
                        let mut it = subrest.splitn(2, ' ');
                        let set = it.next().unwrap_or("");
                        let dest = unquote(it.next().unwrap_or("").trim());
                        let dest_role = role_of(&state, &dest);
                        let dropped = {
                            let mut st = state.lock().unwrap();
                            let d = st.behavior.disconnect_after_mutation;
                            st.behavior.disconnect_after_mutation = false;
                            d
                        };
                        uid_copy(&state, &sel, &dest_role, set);
                        if dropped {
                            return;
                        }
                        w.write_all(format!("{tag} OK done\r\n").as_bytes())
                            .await
                            .unwrap();
                    }
                    "EXPUNGE" => {
                        let dropped = {
                            let mut st = state.lock().unwrap();
                            let d = st.behavior.disconnect_after_mutation;
                            st.behavior.disconnect_after_mutation = false;
                            d
                        };
                        if subrest.trim().is_empty() {
                            uid_expunge_deleted(&state, &sel);
                        } else {
                            uid_expunge_set(&state, &sel, subrest.trim());
                        }
                        if dropped {
                            return;
                        }
                        w.write_all(format!("{tag} OK done\r\n").as_bytes())
                            .await
                            .unwrap();
                    }
                    _ => {
                        w.write_all(format!("{tag} BAD unknown\r\n").as_bytes())
                            .await
                            .unwrap();
                    }
                }
            }
            "CREATE" => {
                w.write_all(format!("{tag} OK done\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            "APPEND" => {
                // APPEND <folder> [flags] {n}  (or {n+}): read literal, store.
                let (folder_part, litlen) = match parse_append_prelude(&rest) {
                    Some(v) => v,
                    None => {
                        w.write_all(format!("{tag} BAD append\r\n").as_bytes())
                            .await
                            .unwrap();
                        continue;
                    }
                };
                let folder = unquote(&folder_part);
                let role = role_of(&state, &folder);
                // LITERAL+ form needs no continuation; sync form gets one.
                let sync = !rest.contains("{") || {
                    let b = rest.as_bytes();
                    let o = b.iter().rposition(|&c| c == b'{').unwrap();
                    !rest[o..].contains('+')
                };
                if sync {
                    w.write_all(b"+ go ahead\r\n").await.unwrap();
                }
                let mut data = vec![0u8; litlen];
                let mut got = 0;
                while got < litlen {
                    let n = match tokio::io::AsyncReadExt::read(&mut r, &mut data[got..]).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    got += n;
                }
                // NOTE: no CRLF follows APPEND literal bytes on the wire.
                let raw = String::from_utf8_lossy(&data).to_string();
                let subject = raw
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("subject:"))
                    .map(|l| l[8..].trim().to_string())
                    .unwrap_or_default();
                let (uidv, uid) = {
                    let mut st = state.lock().unwrap();
                    let msgid = st.msgs.keys().max().copied().unwrap_or(0) + 7919;
                    st.modseq += 1;
                    let ms = st.modseq;
                    let uid = *st.next_uid.get(&role).unwrap_or(&1);
                    st.next_uid.insert(role.clone(), uid + 1);
                    let mut folders = HashMap::new();
                    folders.insert(role.clone(), uid);
                    let is_draft = rest.contains("\\Draft");
                    let mut labels = vec![];
                    if is_draft {
                        labels.push("DRAFT".to_string());
                    }
                    st.msgs.insert(
                        msgid,
                        FMsg {
                            msgid,
                            thrid: msgid,
                            folders,
                            flags: if is_draft {
                                vec!["\\Draft".into(), "\\Seen".into()]
                            } else {
                                vec![]
                            },
                            labels,
                            modseq: ms,
                            from_name: "me".into(),
                            from_email: "me@example.com".into(),
                            subject,
                            date_offset_h: 0,
                            size: data.len() as u32,
                            has_att: false,
                        },
                    );
                    (st.uidvalidity.get(&role).copied().unwrap_or(987654), uid)
                };
                w.write_all(
                    format!("{tag} OK [APPENDUID {uidv} {uid}] APPEND completed.\r\n").as_bytes(),
                )
                .await
                .unwrap();
            }
            "IDLE" => {
                // Register for server-pushed events; DONE ends idle.
                let (tx, mut rx) = mpsc::unbounded_channel::<String>();
                {
                    state.lock().unwrap().idle_txs.push(tx);
                }
                w.write_all(b"+ idling\r\n").await.unwrap();
                loop {
                    tokio::select! {
                        n = read_line(&mut r, &mut buf) => {
                            if n.unwrap_or(0) == 0 { return; }
                            let l = String::from_utf8_lossy(&buf).trim().to_string();
                            if l.eq_ignore_ascii_case("DONE") {
                                w.write_all(format!("{tag} OK done\r\n").as_bytes()).await.unwrap();
                                break;
                            }
                        }
                        evt = rx.recv() => {
                            if let Some(e) = evt {
                                w.write_all(format!("{e}\r\n").as_bytes()).await.unwrap();
                            } else {
                                break;
                            }
                        }
                    }
                }
            }
            "NOOP" => {
                w.write_all(format!("{tag} OK done\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            "LOGOUT" => {
                w.write_all(b"* BYE bye\r\n").await.unwrap();
                w.write_all(format!("{tag} OK done\r\n").as_bytes())
                    .await
                    .unwrap();
                return;
            }
            "DONE" => {
                // stray DONE (no IDLE active)
                w.write_all(format!("{tag} BAD no idle\r\n").as_bytes())
                    .await
                    .unwrap();
            }
            _ => {
                w.write_all(format!("{tag} BAD unknown command\r\n").as_bytes())
                    .await
                    .unwrap();
            }
        }
    }
}

/// Split `APPEND <folder> [flags] {n[+]}` into (folder, literal_len).
fn parse_append_prelude(s: &str) -> Option<(String, usize)> {
    let s = s.trim();
    let (folder, rest) = if let Some(inner) = s.strip_prefix('"') {
        let end = inner.find('"')?;
        (
            inner[..end].to_string(),
            inner[end + 1..].trim().to_string(),
        )
    } else {
        let mut it = s.splitn(2, ' ');
        (it.next()?.to_string(), it.next().unwrap_or("").to_string())
    };
    // literal size is the last {n[+]} token
    let open = rest.rfind('{')?;
    let inner = rest[open + 1..].strip_suffix('}')?;
    let num: usize = inner.strip_suffix('+').unwrap_or(inner).parse().ok()?;
    Some((folder, num))
}

fn split_args(s: &str) -> Vec<String> {
    // LOGIN args: email + password (password may be quoted).
    let mut out = vec![];
    let mut cur = String::new();
    let mut in_q = false;
    for ch in s.chars() {
        match ch {
            '"' => {
                in_q = !in_q;
            }
            ' ' if !in_q => {
                if !cur.is_empty() {
                    out.push(cur.clone());
                    cur.clear();
                }
            }
            _ => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

fn role_of(state: &Arc<Mutex<State>>, folder: &str) -> String {
    let st = state.lock().unwrap();
    for (role, name) in &st.folders {
        if name == folder {
            return role.clone();
        }
    }
    // Unknown folder (e.g. freshly CREATEed label): user folder keyed by
    // decoded name, matching the provider's imap:<name> convention.
    decode_utf7_mailbox(folder)
}

/// Split a FETCH argument tail at TOP-LEVEL whitespace, honoring `[...]`
/// (which may contain `(...)` field lists with spaces) and quoted strings.
fn split_items(s: &str) -> Result<Vec<String>, String> {
    let b = s.as_bytes();
    let mut out: Vec<String> = vec![];
    let mut cur = String::new();
    let (mut depth_paren, mut depth_bracket) = (0usize, 0usize);
    let mut in_q = false;
    for &c in b {
        match c {
            b'(' if !in_q => {
                depth_paren += 1;
                cur.push('(');
            }
            b')' if !in_q => {
                if depth_paren == 0 {
                    return Err("unbalanced )".into());
                }
                depth_paren -= 1;
                cur.push(')');
            }
            b'[' if !in_q => {
                depth_bracket += 1;
                cur.push('[');
            }
            b']' if !in_q => {
                if depth_bracket == 0 {
                    return Err("unbalanced ]".into());
                }
                depth_bracket -= 1;
                cur.push(']');
            }
            b'"' => {
                in_q = !in_q;
                cur.push('"');
            }
            b' ' | b'\t' if !in_q && depth_paren == 0 && depth_bracket == 0 => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c as char),
        }
    }
    if in_q {
        return Err("unterminated quote".into());
    }
    if depth_paren > 0 || depth_bracket > 0 {
        return Err("unbalanced item list".into());
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    Ok(out)
}

/// A UID set: digits/`*`, optional `:<end>`, comma-separated entries.
fn valid_uid_set(set: &str) -> bool {
    if set.is_empty() {
        return false;
    }
    let num = |t: &str| t == "*" || (!t.is_empty() && t.bytes().all(|c| c.is_ascii_digit()));
    set.split(',').all(|part| match part.split_once(':') {
        Some((a, b)) => num(a) && num(b),
        None => num(part),
    })
}

/// One legal FETCH data item (no embedded top-level spaces).
fn validate_item(tok: &str) -> Result<(), String> {
    if tok.starts_with('(') {
        return Err("nested list is not a single item".into());
    }
    let up = tok.to_ascii_uppercase();
    if matches!(
        up.as_str(),
        "UID"
            | "FLAGS"
            | "INTERNALDATE"
            | "RFC822"
            | "RFC822.HEADER"
            | "RFC822.SIZE"
            | "ENVELOPE"
            | "MODSEQ"
            | "BODYSTRUCTURE"
            | "X-GM-MSGID"
            | "X-GM-THRID"
            | "X-GM-LABELS"
    ) {
        return Ok(());
    }
    if let Some(rest) = up
        .strip_prefix("BODY.PEEK[")
        .or_else(|| up.strip_prefix("BODY["))
    {
        // `...]` plus an optional `<origin[.len]>` partial suffix.
        let Some(close) = rest.find(']') else {
            return Err("unterminated section".into());
        };
        let tail = rest[close + 1..].trim();
        if tail.is_empty() {
            return Ok(());
        }
        let inner = tail
            .strip_prefix('<')
            .and_then(|t| t.strip_suffix('>'))
            .ok_or_else(|| "bad partial suffix".to_string())?;
        let ok = inner.split_once('.').map_or_else(
            || inner.bytes().all(|c| c.is_ascii_digit()) && !inner.is_empty(),
            |(o, l)| {
                !o.is_empty()
                    && o.bytes().all(|c| c.is_ascii_digit())
                    && !l.is_empty()
                    && l.bytes().all(|c| c.is_ascii_digit())
            },
        );
        return if ok {
            Ok(())
        } else {
            Err("bad partial suffix".into())
        };
    }
    Err(format!("unknown fetch item {tok}"))
}

/// Validate a fetched item list: either ONE legal bare item or a balanced
/// parenthesized list of legal items. Two bare items are rejected (P1.1).
fn validate_items(items: &str) -> Result<(), String> {
    let t = items.trim();
    if t.is_empty() {
        return Err("empty item list".into());
    }
    if t.starts_with('(') {
        if !t.ends_with(')') {
            return Err("unbalanced item list".into());
        }
        let inner = &t[1..t.len() - 1];
        let toks = split_items(inner)?;
        if toks.is_empty() {
            return Err("empty item list".into());
        }
        for tok in toks {
            validate_item(&tok)?;
        }
        Ok(())
    } else {
        let toks = split_items(t)?;
        if toks.len() != 1 {
            return Err("multiple bare FETCH items must be parenthesized".into());
        }
        validate_item(&toks[0])
    }
}

/// Test-only entry point to the strict FETCH argument validator (P1.2).
pub fn parse_fetch_args_for_test(s: &str) -> Result<(String, String, Option<u64>), String> {
    parse_fetch_args(s)
}

/// Parse `UID FETCH <set> <items> [CHANGEDSINCE n]`, returning
/// `(set, items, changedsince)`. Malformed input is a tagged BAD, not a
/// silently-accepted default (P1.2).
fn parse_fetch_args(s: &str) -> Result<(String, String, Option<u64>), String> {
    const MOD: &str = "CHANGEDSINCE";
    let found = s.to_ascii_uppercase().find(MOD);
    let since = found
        .and_then(|i| {
            s.get(i + MOD.len()..)?
                .split(|c: char| !c.is_ascii_digit())
                .next()
        })
        .and_then(|n| n.parse().ok());
    // The modifier travels as `(CHANGEDSINCE n)`; drop the whole modifier
    // including its opening paren, not just the keyword.
    let head = match found {
        Some(i) => s[..i].trim().trim_end_matches('(').trim_end(),
        None => s.trim(),
    };
    let toks = split_items(head)?;
    if toks.len() < 2 {
        return Err("missing uid set or items".into());
    }
    if !valid_uid_set(&toks[0]) {
        return Err(format!("bad uid set {}", toks[0]));
    }
    let set = toks[0].clone();
    let items = toks[1..].join(" ");
    validate_items(&items)?;
    Ok((set, items, since))
}

fn uid_search(state: &Arc<Mutex<State>>, folder: &str, query: &str) -> Vec<u32> {
    let st = state.lock().unwrap();
    let q = query.trim();
    if let Some(rest) = q.strip_prefix("X-GM-MSGID ") {
        if let Ok(dec) = rest.trim().parse::<u64>() {
            return st
                .msgs
                .values()
                .filter(|m| m.msgid == dec)
                .filter_map(|m| m.folders.get(folder).copied())
                .collect();
        }
        return vec![];
    }
    if let Some(rest) = q.strip_prefix("X-GM-RAW ") {
        let needle = rest.trim().trim_matches('"').to_lowercase();
        let mut v: Vec<u32> = st
            .msgs
            .values()
            .filter(|m| m.folders.contains_key(folder))
            .filter(|m| m.subject.to_lowercase().contains(&needle))
            .filter_map(|m| m.folders.get(folder).copied())
            .collect();
        v.sort_unstable();
        return v;
    }
    // `UID <set>`: the actual UIDs inside the requested interval (P4.5). The
    // bounded partial-sync scan depends on this returning only real UIDs.
    if let Some(rest) = q.strip_prefix("UID ") {
        let mut v: Vec<u32> = st
            .msgs
            .values()
            .filter(|m| m.folders.contains_key(folder))
            .filter_map(|m| m.folders.get(folder).copied())
            .filter(|u| uid_in_set(*u, rest.trim()))
            .collect();
        v.sort_unstable();
        v.dedup();
        return v;
    }
    // ALL (ignore other criteria for the fake)
    let mut v: Vec<u32> = st
        .msgs
        .values()
        .filter_map(|m| m.folders.get(folder).copied())
        .collect();
    v.sort_unstable();
    v
}

/// `UID 12:34,56,*`-style set membership for the fake server.
fn uid_in_set(uid: u32, set: &str) -> bool {
    for part in set.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.split_once(':') {
            Some((lo, hi)) => {
                let lo = lo.trim().parse::<u32>().unwrap_or(1);
                let hi = if hi.trim() == "*" {
                    u32::MAX
                } else {
                    hi.trim().parse::<u32>().unwrap_or(u32::MAX)
                };
                if uid >= lo && uid <= hi {
                    return true;
                }
            }
            None => {
                if part == "*" {
                    return true;
                }
                if part.trim().parse::<u32>() == Ok(uid) {
                    return true;
                }
            }
        }
    }
    false
}

/// Write a response, optionally fragmented across several TCP writes and/or
/// delayed (P1.2 failure injection).
async fn write_out<W: tokio::io::AsyncWrite + Unpin>(
    w: &mut W,
    bytes: &[u8],
    state: &Arc<Mutex<State>>,
) {
    let (frags, delay_ms) = {
        let st = state.lock().unwrap();
        (
            st.behavior.fragmented_literals,
            st.behavior.completion_delay_ms,
        )
    };
    if delay_ms > 0 {
        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
    }
    if frags <= 1 || bytes.len() < 2 {
        let _ = w.write_all(bytes).await;
        let _ = w.flush().await;
        return;
    }
    let chunk = bytes.len().div_ceil(frags).max(1);
    for c in bytes.chunks(chunk) {
        let _ = w.write_all(c).await;
        let _ = w.flush().await;
        tokio::task::yield_now().await;
    }
}

/// Extract `BODY.PEEK[...]` / `BODY[...]` specs (with any `<origin.len>`
/// suffix) from a validated item list.
fn body_specs(items: &str) -> Vec<String> {
    let mut out = vec![];
    let upper = items.to_ascii_uppercase();
    let mut i = 0;
    let bytes = upper.as_bytes();
    while i < bytes.len() {
        if upper[i..].starts_with("BODY.PEEK[") || upper[i..].starts_with("BODY[") {
            let start = if upper[i..].starts_with("BODY.PEEK[") {
                i + "BODY.PEEK".len()
            } else {
                i + "BODY".len()
            };
            // find matching ] honoring (...) inside
            let mut depth = 0;
            let mut j = start;
            while j < bytes.len() {
                match bytes[j] {
                    b'[' => depth += 1,
                    b']' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            // include optional <origin[.len]> suffix
            let mut k = j + 1;
            if bytes.get(k) == Some(&b'<') {
                while k < bytes.len() && bytes[k] != b'>' {
                    k += 1;
                }
                k += 1;
            }
            // map back to the ORIGINAL-case substring
            out.push(items[start..k].to_string());
            i = k;
        } else {
            i += 1;
        }
    }
    out
}

fn literal_head(label: &str, section: &str, origin: u32, len: usize) -> Vec<u8> {
    if section.is_empty() {
        format!("{label}[]<{origin}> {{{len}}}\r\n").into_bytes()
    } else {
        format!("{label}[{section}]<{origin}> {{{len}}}\r\n").into_bytes()
    }
}

fn slice_partial(b: &[u8], partial: Option<(usize, usize)>) -> &[u8] {
    match partial {
        None => b,
        Some((o, l)) => {
            let o = o.min(b.len());
            &b[o..(o + l).min(b.len())]
        }
    }
}

/// Render one FETCH response for the requested items. Byte-exact: overridden
/// section payloads and binary literals are never routed through a String.
fn uid_fetch(
    state: &Arc<Mutex<State>>,
    folder: &str,
    set: &str,
    items: &str,
    since: Option<u64>,
) -> Vec<u8> {
    let st = state.lock().unwrap();
    let folder_uids: Vec<u32> = st
        .msgs
        .values()
        .filter_map(|m| m.folders.get(folder).copied())
        .collect();
    let want: HashSet<u32> = parse_uid_set(set, &folder_uids).into_iter().collect();
    let wrong = st.behavior.wrong_uid_response;
    let omit = st.behavior.omit_section_data;
    let mut seq = 0u32;
    let mut ordered: Vec<(&FMsg, u32, u32)> = vec![];
    let mut all: Vec<(&FMsg, u32)> = st
        .msgs
        .values()
        .filter_map(|m| m.folders.get(folder).map(|u| (m, *u)))
        .collect();
    all.sort_by_key(|(_, u)| *u);
    let folder_uids: Vec<u32> = all.iter().map(|(_, u)| *u).collect();
    for (m, u) in all {
        seq += 1;
        if !want.contains(&u) {
            continue;
        }
        if let Some(s) = since {
            if m.modseq <= s {
                continue;
            }
        }
        ordered.push((m, u, seq));
    }
    let items_up = items.to_ascii_uppercase();
    let specs = body_specs(items);
    let mut out: Vec<u8> = vec![];
    for (m, u, s) in ordered {
        // A deliberately wrong UID models an unsolicited/stale FETCH row the
        // client must filter out (P1.2).
        let advertised = if wrong {
            folder_uids
                .iter()
                .copied()
                .filter(|v| !want.contains(v))
                .min()
                .unwrap_or(u + 1000)
        } else {
            u
        };
        let mut parts: Vec<String> = vec![format!("UID {advertised}")];
        if items_up.contains("FLAGS") {
            parts.push(format!("FLAGS ({})", m.flags.join(" ")));
        }
        if items_up.contains("INTERNALDATE") {
            parts.push(format!(
                "INTERNALDATE \"{}\"",
                internaldate(m.date_offset_h)
            ));
        }
        if items_up.contains("RFC822.SIZE") {
            parts.push(format!("RFC822.SIZE {}", m.size));
        }
        if items_up.contains("MODSEQ") {
            parts.push(format!("MODSEQ ({})", m.modseq));
        }
        if items_up.contains("X-GM-MSGID") {
            parts.push(format!("X-GM-MSGID {}", m.msgid));
        }
        if items_up.contains("X-GM-THRID") {
            parts.push(format!("X-GM-THRID {}", m.thrid));
        }
        if items_up.contains("X-GM-LABELS") {
            let mut labels = m.labels.clone();
            let implied = match folder {
                "trash" => Some("\\Trash"),
                "junk" => Some("\\Spam"),
                _ => None,
            };
            if let Some(imp) = implied {
                if !labels.iter().any(|l| l == imp) {
                    labels.push(imp.into());
                }
            }
            parts.push(format!("X-GM-LABELS {}", gm_label_list(&labels)));
        }
        if items_up.contains("BODYSTRUCTURE") {
            let bs = st
                .bodystructure_overrides
                .get(&m.msgid)
                .cloned()
                .unwrap_or_else(|| bodystructure(m));
            parts.push(format!("BODYSTRUCTURE {bs}"));
        }
        let mut body: Vec<Vec<u8>> = vec![];
        if !omit {
            for spec in &specs {
                body.push(render_body_part(m, spec, &st));
            }
        }
        out.extend_from_slice(format!("* {s} FETCH (").as_bytes());
        out.extend_from_slice(parts.join(" ").as_bytes());
        for b in body {
            out.extend_from_slice(b" ");
            out.extend_from_slice(&b);
        }
        out.extend_from_slice(b")\r\n");
    }
    out
}

fn render_body_part(m: &FMsg, spec: &str, st: &State) -> Vec<u8> {
    // spec like `[HEADER.FIELDS (FROM ...)]`, `[1]<0.2048>`, `[]`
    let inner = spec.trim_start_matches('[');
    let (section, partial) = match inner.find("]<") {
        Some(i) => (
            &inner[..i],
            Some(&inner[i + 2..inner.len().saturating_sub(1)]),
        ),
        None => (inner.strip_suffix(']').unwrap_or(inner), None),
    };
    let partial = partial.map(|p| {
        let (o, l) = p.split_once('.').unwrap_or((p, ""));
        (
            o.parse::<usize>().unwrap_or(0),
            if l.is_empty() {
                usize::MAX
            } else {
                l.parse().unwrap_or(usize::MAX)
            },
        )
    });
    let origin = partial.map(|(o, _)| o as u32).unwrap_or(0);
    let up = section.to_ascii_uppercase();

    // Cap-boundary cases (P2.4): a partial section literal that is either
    // over-declared or cut off by a dropped socket.
    if let Some((_, requested)) = partial {
        if requested != usize::MAX && !section.is_empty() {
            if st.behavior.oversized_partial_literal {
                return literal_head("BODY", section, origin, requested.saturating_add(1_000_000));
            }
            if st.behavior.drop_in_section_literal {
                // Head only; the handler closes the socket before any bytes.
                return literal_head("BODY", section, origin, requested);
            }
        }
    }

    // Zero-length or byte-exact payloads for protocol tests (P1.2).
    if st.behavior.empty_section && !section.is_empty() && !up.starts_with("HEADER") {
        let mut out = literal_head("BODY", section, origin, 0);
        out.extend_from_slice(b"\r\n");
        return out;
    }
    if !section.is_empty() {
        if let Some(bytes) = st.section_bytes.get(&(m.msgid, section.to_string())) {
            let bytes = slice_partial(bytes, partial);
            let mut out = literal_head("BODY", section, origin, bytes.len());
            out.extend_from_slice(bytes);
            return out;
        }
    }

    if up.starts_with("HEADER") {
        let h = header_block(m);
        let mut out = literal_head("BODY", section, origin, h.len());
        out.extend_from_slice(h.as_bytes());
        return out;
    }
    if section.is_empty() {
        if let Some(n) = st.behavior.large_full_message_bytes {
            let mut full = header_block(m).into_bytes();
            full.reserve(n);
            full.extend(std::iter::repeat(b'x').take(n));
            let bytes = slice_partial(&full, partial);
            let mut out = literal_head("BODY", section, origin, bytes.len());
            out.extend_from_slice(bytes);
            return out;
        }
        let full = full_mime(m);
        let mut out = literal_head("BODY", section, origin, full.len());
        out.extend_from_slice(full.as_bytes());
        return out;
    }
    // section fetch: content honors the encoding the BODYSTRUCTURE
    // advertises (base64 parts serve base64, like real Gmail).
    let (payload, _enc) = section_payload(m, section);
    let mut text = payload;
    while text.len() < 3000 && text.len() < 6000 {
        let t = text.clone();
        text.push_str(&t);
    }
    // harnesses that need byte-exactness fetch small windows; cap there.
    let slice = slice_partial(text.as_bytes(), partial);
    let mut out = literal_head("BODY", section, origin, slice.len());
    out.extend_from_slice(slice);
    out
}

/// Apply STORE edits; returns (matched uids, changed msgids).
/// Deterministic full MIME for BODY[] fetches: text+html alternative,
// PDF attachment for has_att, inline PNG for every 5th, quote block for
// every 3rd, List-Unsubscribe for every 7th.
fn full_mime(m: &FMsg) -> String {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    let mut h = format!(
        "From: {} <{}>\r\nTo: me@example.com\r\nSubject: {}\r\nDate: {}\r\nMessage-ID: <{}@example.com>\r\n",
        m.from_name, m.from_email, m.subject,
        internaldate(m.date_offset_h),
        m.msgid
    );
    if m.msgid % 7 == 0 {
        h.push_str("List-Unsubscribe: <https://example.com/unsub>, <mailto:leave@example.com>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click\r\n");
    }
    let quote = if m.msgid % 3 == 0 {
        "\r\nOn Monday, someone wrote:\r\n> quoted line here\r\n"
    } else {
        ""
    };
    let text = format!("Hello {}, this is the plain version.{quote}", m.from_name);
    let html = format!(
        "<html><body><p>Hello {}, this is the <b>html</b> version.</p></body></html>",
        m.from_name
    );
    if !m.has_att {
        return format!(
            "{h}Content-Type: multipart/alternative; boundary=\"ALT\"\r\n\r\n--ALT\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n--ALT\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n--ALT--\r\n"
        );
    }
    let pdf_raw = format!("PDFDATA-{}", m.msgid);
    let pdf = b64.encode(pdf_raw.as_bytes());
    let mut parts = format!(
        "{h}Content-Type: multipart/mixed; boundary=\"MIX\"\r\n\r\n--MIX\r\nContent-Type: multipart/alternative; boundary=\"ALT\"\r\n\r\n--ALT\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n--ALT\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{html}\r\n--ALT--\r\n--MIX\r\nContent-Type: application/pdf; name=\"doc-{}.pdf\"\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"doc-{}.pdf\"\r\n\r\n{pdf}\r\n",
        m.msgid % 1000,
        m.msgid % 1000
    );
    if m.msgid % 5 == 0 {
        // 1x1 transparent PNG, byte-exact for decode tests.
        let png_b64 = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
        parts.push_str(&format!(
            "--MIX\r\nContent-Type: image/png; name=\"img.png\"\r\nContent-Transfer-Encoding: base64\r\nContent-ID: <ii_{}>\r\n\r\n{png_b64}\r\n",
            m.msgid % 1000
        ));
    }
    parts.push_str("--MIX--\r\n");
    parts
}

/// Section payload honoring the advertised transfer encoding, mirroring
/// bodystructure(): pdf/png parts are base64, text parts are raw.
fn section_payload(m: &FMsg, section: &str) -> (String, &'static str) {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD;
    if m.has_att && section == "2" {
        let raw = format!("PDFDATA-{}", m.msgid);
        return (b64.encode(raw.as_bytes()), "base64");
    }
    if m.has_att && section == "3" && m.msgid % 5 == 0 {
        return (
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg=="
                .to_string(),
            "base64",
        );
    }
    (
        format!(
            "Hello, this is section {section} of {}. {} ",
            m.subject, m.subject
        ),
        "7bit",
    )
}

fn uid_store(state: &Arc<Mutex<State>>, folder: &str, args: &str) -> (usize, Vec<u64>) {
    // `<set> +/-FLAGS.SILENT (...)` or `<set> +/-X-GM-LABELS (...)`
    let mut it = args.splitn(2, ' ');
    let set = it.next().unwrap_or("");
    let rest = it.next().unwrap_or("").to_string();
    let up = rest.to_ascii_uppercase();
    let (op, kind) = if up.starts_with("+FLAGS") {
        (1i8, "flags")
    } else if up.starts_with("-FLAGS") {
        (-1, "flags")
    } else if up.starts_with("+X-GM-LABELS") {
        (1, "labels")
    } else if up.starts_with("-X-GM-LABELS") {
        (-1, "labels")
    } else {
        return (0, vec![]);
    };
    // value list in parens
    let vals: Vec<String> = rest
        .find('(')
        .and_then(|a| rest.rfind(')').map(|b| rest[a + 1..b].to_string()))
        .map(|inner| {
            inner
                .split_whitespace()
                .map(|s| s.trim_matches('"').to_string())
                .collect()
        })
        .unwrap_or_default();
    let mut st = state.lock().unwrap();
    let folder_uids: Vec<u32> = st
        .msgs
        .values()
        .filter_map(|m| m.folders.get(folder).copied())
        .collect();
    let want = parse_uid_set(set, &folder_uids);
    st.modseq += 1;
    let ms = st.modseq;
    let mut n = 0;
    let mut changed = vec![];
    let ids: Vec<u64> = st
        .msgs
        .iter()
        .filter(|(_, m)| {
            m.folders
                .get(folder)
                .map(|u| want.contains(u))
                .unwrap_or(false)
        })
        .map(|(id, _)| *id)
        .collect();
    for id in ids {
        if let Some(m) = st.msgs.get_mut(&id) {
            n += 1;
            changed.push(id);
            m.modseq = ms;
            if kind == "flags" {
                for v in &vals {
                    if op > 0 {
                        if !m.flags.iter().any(|f| f.eq_ignore_ascii_case(v)) {
                            m.flags.push(v.clone());
                        }
                    } else {
                        m.flags.retain(|f| !f.eq_ignore_ascii_case(v));
                    }
                }
            } else {
                for v in &vals {
                    let norm_in = v.strip_prefix('\\').unwrap_or(v);
                    let is_system = [
                        "inbox",
                        "sent",
                        "draft",
                        "starred",
                        "important",
                        "trash",
                        "spam",
                        "unread",
                    ]
                    .contains(&norm_in.to_ascii_lowercase().as_str());
                    if op > 0 {
                        let exists = m.labels.iter().any(|l| {
                            let ns = l.strip_prefix('\\').unwrap_or(l);
                            if is_system {
                                ns.eq_ignore_ascii_case(norm_in)
                            } else {
                                ns == norm_in
                            }
                        });
                        if !exists {
                            let stored = if is_system {
                                norm_in.to_ascii_uppercase()
                            } else {
                                v.clone()
                            };
                            m.labels.push(stored);
                        }
                    } else {
                        m.labels.retain(|l| {
                            let ns = l.strip_prefix('\\').unwrap_or(l);
                            if is_system {
                                !ns.eq_ignore_ascii_case(norm_in)
                            } else {
                                ns != norm_in
                            }
                        });
                    }
                }
            }
        }
    }
    (n, changed)
}

fn uid_move(state: &Arc<Mutex<State>>, src: &str, dest_role: &str, set: &str) {
    let mut st = state.lock().unwrap();
    let folder_uids: Vec<u32> = st
        .msgs
        .values()
        .filter_map(|m| m.folders.get(src).copied())
        .collect();
    let want = parse_uid_set(set, &folder_uids);
    st.modseq += 1;
    let ms = st.modseq;
    // resolve dest folder name presence (CREATE may have added user folders)
    let next = *st.next_uid.get(dest_role).unwrap_or(&1);
    let mut nu = next;
    let ids: Vec<u64> = st
        .msgs
        .iter()
        .filter(|(_, m)| {
            m.folders
                .get(src)
                .map(|u| want.contains(u))
                .unwrap_or(false)
        })
        .map(|(id, _)| *id)
        .collect();
    for id in ids {
        if let Some(m) = st.msgs.get_mut(&id) {
            m.folders.remove(src);
            // real MOVE assigns a fresh UID in dest
            m.folders.insert(dest_role.into(), nu);
            nu += 1;
            m.modseq = ms;
        }
    }
    st.next_uid.insert(dest_role.into(), nu);
}

fn uid_copy(state: &Arc<Mutex<State>>, src: &str, dest_role: &str, set: &str) {
    let mut st = state.lock().unwrap();
    let folder_uids: Vec<u32> = st
        .msgs
        .values()
        .filter_map(|m| m.folders.get(src).copied())
        .collect();
    let want = parse_uid_set(set, &folder_uids);
    st.modseq += 1;
    let ms = st.modseq;
    let next = *st.next_uid.get(dest_role).unwrap_or(&1);
    let mut nu = next;
    let ids: Vec<u64> = st
        .msgs
        .iter()
        .filter(|(_, m)| {
            m.folders
                .get(src)
                .map(|u| want.contains(u))
                .unwrap_or(false)
        })
        .map(|(id, _)| *id)
        .collect();
    for id in ids {
        if let Some(m) = st.msgs.get_mut(&id) {
            m.folders.insert(dest_role.into(), nu);
            nu += 1;
            m.modseq = ms;
        }
    }
    st.next_uid.insert(dest_role.into(), nu);
}

fn uid_expunge_deleted(state: &Arc<Mutex<State>>, folder: &str) {
    let mut st = state.lock().unwrap();
    let dead: Vec<u64> = st
        .msgs
        .iter()
        .filter(|(_, m)| {
            m.folders.contains_key(folder)
                && m.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Deleted"))
        })
        .map(|(id, _)| *id)
        .collect();
    for id in dead {
        if let Some(m) = st.msgs.get_mut(&id) {
            m.folders.remove(folder);
            if m.folders.is_empty() {
                st.msgs.remove(&id);
            }
        }
    }
}

fn uid_expunge_set(state: &Arc<Mutex<State>>, folder: &str, set: &str) {
    let mut st = state.lock().unwrap();
    let folder_uids: Vec<u32> = st
        .msgs
        .values()
        .filter_map(|m| m.folders.get(folder).copied())
        .collect();
    let want = parse_uid_set(set, &folder_uids);
    let dead: Vec<u64> = st
        .msgs
        .iter()
        .filter(|(_, m)| {
            m.folders
                .get(folder)
                .map(|u| want.contains(u))
                .unwrap_or(false)
        })
        .map(|(id, _)| *id)
        .collect();
    for id in dead {
        if let Some(m) = st.msgs.get_mut(&id) {
            m.folders.remove(folder);
            if m.folders.is_empty() {
                st.msgs.remove(&id);
            }
        }
    }
}
