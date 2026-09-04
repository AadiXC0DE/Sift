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
}

#[derive(Debug, Default)]
pub struct State {
    pub msgs: HashMap<u64, FMsg>,
    pub next_uid: HashMap<String, u32>,
    pub uidvalidity: HashMap<String, u32>,
    pub modseq: u64,
    pub folders: HashMap<String, String>,
    pub behavior: Behavior,
    pub commands_seen: Vec<String>,
    pub idle_txs: Vec<mpsc::UnboundedSender<String>>,
    pub cmd_count: usize,
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

fn parse_uid_set(set: &str, max: u32) -> Vec<u32> {
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
            st.commands_seen.push(format!("{verb} {rest}"));
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
                w.write_all(out.as_bytes()).await.unwrap();
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
                match subverb.as_str() {
                    "SEARCH" => {
                        let sel = selected.clone().unwrap_or_else(|| "all".into());
                        let uids = uid_search(&state, &sel, &subrest);
                        let list = uids
                            .iter()
                            .map(|u| u.to_string())
                            .collect::<Vec<_>>()
                            .join(" ");
                        w.write_all(
                            format!(
                                "* SEARCH{sep}{list}\r\n{tag} OK done\r\n",
                                sep = if list.is_empty() { "" } else { " " },
                                list = list
                            )
                            .as_bytes(),
                        )
                        .await
                        .unwrap();
                    }
                    "FETCH" => {
                        // <set> (<items>) [CHANGEDSINCE n]
                        let (set, items, since) = parse_fetch_args(&subrest);
                        let sel = selected.clone().unwrap_or_else(|| "all".into());
                        let lines = uid_fetch(&state, &sel, &set, &items, since);
                        let mut out = lines.join("");
                        out.push_str(&format!("{tag} OK done\r\n"));
                        w.write_all(out.as_bytes()).await.unwrap();
                    }
                    "STORE" => {
                        // <set> +/-FLAGS.SILENT (...) / +/-X-GM-LABELS (...)
                        let (n, changed) = uid_store(
                            &state,
                            &selected.clone().unwrap_or_else(|| "all".into()),
                            &subrest,
                        );
                        let _ = (n, changed);
                        w.write_all(format!("{tag} OK done\r\n").as_bytes())
                            .await
                            .unwrap();
                    }
                    "MOVE" => {
                        // <set> <dest>
                        let mut it = subrest.splitn(2, ' ');
                        let set = it.next().unwrap_or("");
                        let dest = unquote(it.next().unwrap_or("").trim());
                        let dest_role = role_of(&state, &dest);
                        let sel = selected.clone().unwrap_or_else(|| "all".into());
                        uid_move(&state, &sel, &dest_role, set);
                        w.write_all(format!("{tag} OK done\r\n").as_bytes())
                            .await
                            .unwrap();
                    }
                    "COPY" => {
                        let mut it = subrest.splitn(2, ' ');
                        let set = it.next().unwrap_or("");
                        let dest = unquote(it.next().unwrap_or("").trim());
                        let dest_role = role_of(&state, &dest);
                        let sel = selected.clone().unwrap_or_else(|| "all".into());
                        uid_copy(&state, &sel, &dest_role, set);
                        w.write_all(format!("{tag} OK done\r\n").as_bytes())
                            .await
                            .unwrap();
                    }
                    "EXPUNGE" => {
                        let sel = selected.clone().unwrap_or_else(|| "all".into());
                        if subrest.trim().is_empty() {
                            uid_expunge_deleted(&state, &sel);
                        } else {
                            uid_expunge_set(&state, &sel, subrest.trim());
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
        let end = inner.find('"')? + 1;
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

/// Parse `UID FETCH <set> (<items>) [CHANGEDSINCE n]`.
fn parse_fetch_args(s: &str) -> (String, String, Option<u64>) {
    let since = s
        .to_ascii_uppercase()
        .find("CHANGEDSINCE")
        .and_then(|i| s[i + 11..].split_whitespace().next())
        .and_then(|n| n.parse().ok());
    let head = match s.to_ascii_uppercase().find("CHANGEDSINCE") {
        Some(i) => s[..i].trim(),
        None => s.trim(),
    };
    // head: `<set> (<items>)` or a bare item (e.g. BODYSTRUCTURE)
    let (set, items) = match head.find('(') {
        Some(depth) => (
            head[..depth].trim().to_string(),
            head[depth..].trim().to_string(),
        ),
        None => {
            let mut it = head.splitn(2, ' ');
            (
                it.next().unwrap_or("").to_string(),
                it.next().unwrap_or("").to_string(),
            )
        }
    };
    (set, items, since)
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
    // ALL (ignore other criteria for the fake)
    let mut v: Vec<u32> = st
        .msgs
        .values()
        .filter_map(|m| m.folders.get(folder).copied())
        .collect();
    v.sort_unstable();
    v
}

/// Render one FETCH response for the requested items.
fn uid_fetch(
    state: &Arc<Mutex<State>>,
    folder: &str,
    set: &str,
    items: &str,
    since: Option<u64>,
) -> Vec<String> {
    let st = state.lock().unwrap();
    let max_uid = st
        .msgs
        .values()
        .filter_map(|m| m.folders.get(folder).copied())
        .max()
        .unwrap_or(1);
    let want: HashSet<u32> = parse_uid_set(set, max_uid).into_iter().collect();
    let mut seq = 0u32;
    let mut ordered: Vec<(&FMsg, u32)> = vec![];
    let mut all: Vec<(&FMsg, u32)> = st
        .msgs
        .values()
        .filter_map(|m| m.folders.get(folder).map(|u| (m, *u)))
        .collect();
    all.sort_by_key(|(_, u)| *u);
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
        ordered.push((m, seq));
    }
    let items_up = items.to_ascii_uppercase();
    let mut out = vec![];
    for (m, s) in ordered {
        let mut parts: Vec<String> = vec![];
        // UID always included for sanity (callers key on it).
        let u = m.folders.get(folder).copied().unwrap_or(0);
        parts.push(format!("UID {u}"));
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
            // Like real Gmail, folder membership implies system labels.
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
            parts.push(format!("BODYSTRUCTURE {}", bodystructure(m)));
        }
        // BODY.PEEK[...] items (header fields, sections, partials).
        for spec in body_specs(items) {
            parts.push(render_body_part(m, &spec));
        }
        // Every response is CRLF-terminated (literals embed their own).
        out.push(format!("* {s} FETCH ({})\r\n", parts.join(" ")));
    }
    out
}

/// Extract `BODY.PEEK[...]...` specs from the item list.
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

fn render_body_part(m: &FMsg, spec: &str) -> String {
    // spec like `[HEADER.FIELDS (FROM ...)]`, `[1]<0.2048>`, `[]`
    let inner = spec.trim_start_matches('[');
    let (section, partial) = match inner.find("]<") {
        Some(i) => (&inner[..i], Some(&inner[i + 2..inner.len() - 1])),
        None => (inner.strip_suffix(']').unwrap_or(inner), None),
    };
    let up = section.to_ascii_uppercase();
    if up.starts_with("HEADER") {
        let h = header_block(m);
        return format!("BODY[{section}] {{{}}}\r\n{h}", h.len());
    }
    if section.is_empty() {
        // full body: realistic MIME (alternative + optional attachment +
        // inline image + quote), deterministic per message for snapshots.
        let full = full_mime(m);
        return format!("BODY[] {{{}}}\r\n{full}", full.len());
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
    let text = text;
    let (origin, len) = partial
        .map(|p| {
            let (o, l) = p.split_once('.').unwrap_or((p, ""));
            (
                o.parse::<usize>().unwrap_or(0),
                if l.is_empty() {
                    usize::MAX
                } else {
                    l.parse().unwrap_or(usize::MAX)
                },
            )
        })
        .unwrap_or((0, usize::MAX));
    let slice = &text.as_bytes()[origin.min(text.len())..(origin + len).min(text.len())];
    let s = String::from_utf8_lossy(slice).into_owned();
    format!("BODY[{section}]<{origin}> {{{}}}\r\n{s}", s.len())
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
    let max_uid = st
        .msgs
        .values()
        .filter_map(|m| m.folders.get(folder).copied())
        .max()
        .unwrap_or(1);
    let want = parse_uid_set(set, max_uid);
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
                    if op > 0 {
                        if !m.labels.iter().any(|l| l == v) {
                            m.labels.push(v.clone());
                        }
                    } else {
                        m.labels.retain(|l| l != v);
                    }
                }
            }
        }
    }
    (n, changed)
}

fn uid_move(state: &Arc<Mutex<State>>, src: &str, dest_role: &str, set: &str) {
    let mut st = state.lock().unwrap();
    let max_uid = st
        .msgs
        .values()
        .filter_map(|m| m.folders.get(src).copied())
        .max()
        .unwrap_or(1);
    let want = parse_uid_set(set, max_uid);
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
    let max_uid = st
        .msgs
        .values()
        .filter_map(|m| m.folders.get(src).copied())
        .max()
        .unwrap_or(1);
    let want = parse_uid_set(set, max_uid);
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
    let max_uid = st
        .msgs
        .values()
        .filter_map(|m| m.folders.get(folder).copied())
        .max()
        .unwrap_or(1);
    let want = parse_uid_set(set, max_uid);
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
