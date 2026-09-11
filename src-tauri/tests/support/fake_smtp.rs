//! Minimal SMTP sink for send tests (Phase 11 task 15, test-only).
//! Records accepted messages; scripted 535/552/421 replies by content flags.
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

#[derive(Debug, Clone)]
pub struct Accepted {
    pub from: String,
    pub to: Vec<String>,
    pub raw: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct SmtpBehavior {
    /// AUTH always fails 535 (bad password test).
    pub fail_auth: bool,
    /// DATA always fails 552 (over quota/size test).
    pub fail_data_552: bool,
    /// First DATA attempt fails 421, then succeeds (retry test).
    pub fail_data_421_once: bool,
}

pub struct FakeSmtp {
    pub addr: SocketAddr,
    pub accepted: Arc<Mutex<Vec<Accepted>>>,
    pub behavior: Arc<Mutex<SmtpBehavior>>,
    _task: tokio::task::JoinHandle<()>,
}

impl FakeSmtp {
    pub async fn start() -> Self {
        Self::start_with(SmtpBehavior::default()).await
    }

    pub async fn start_with(behavior: SmtpBehavior) -> Self {
        let accepted = Arc::new(Mutex::new(vec![]));
        let behavior = Arc::new(Mutex::new(behavior));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (a2, b2) = (accepted.clone(), behavior.clone());
        let task = tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else {
                    break;
                };
                let (a3, b3) = (a2.clone(), b2.clone());
                tokio::spawn(async move { handle(sock, a3, b3).await });
            }
        });
        Self {
            addr,
            accepted,
            behavior,
            _task: task,
        }
    }
}

async fn write(w: &mut tokio::net::tcp::OwnedWriteHalf, s: &str) {
    let _ = w.write_all(s.as_bytes()).await;
}

async fn handle(
    sock: tokio::net::TcpStream,
    accepted: Arc<Mutex<Vec<Accepted>>>,
    behavior: Arc<Mutex<SmtpBehavior>>,
) {
    let (rh, mut wh) = sock.into_split();
    let mut r = BufReader::new(rh);
    let mut line = String::new();
    write(&mut wh, "220 fake-smtp ready\r\n").await;
    let mut from = String::new();
    let mut to: Vec<String> = vec![];
    let mut authed = false;
    loop {
        line.clear();
        if r.read_line(&mut line).await.unwrap_or(0) == 0 {
            return;
        }
        let upper = line.to_ascii_uppercase();
        if upper.starts_with("EHLO") || upper.starts_with("HELO") {
            write(&mut wh, "250-HELLO\r\n250 AUTH PLAIN LOGIN\r\n").await;
        } else if upper.starts_with("AUTH PLAIN") {
            // AUTH PLAIN [b64] - credentials may arrive inline or challenged.
            let mut b64 = line["AUTH PLAIN".len()..].trim().to_string();
            if b64.is_empty() {
                write(&mut wh, "334 \r\n").await;
                line.clear();
                if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                b64 = line.trim().to_string();
            }
            let bad_pw = behavior.lock().unwrap().fail_auth || b64.contains("YmFk"); // "\0u\0bad..." base64 always contains YmFk
            if bad_pw {
                write(&mut wh, "535 5.7.8 Username and Password not accepted.\r\n").await;
            } else {
                authed = true;
                write(&mut wh, "235 accepted\r\n").await;
            }
        } else if upper.starts_with("AUTH ") {
            write(&mut wh, "504 unsupported\r\n").await;
        } else if upper.starts_with("MAIL FROM:") {
            from = line[10..].trim().trim_matches(&['<', '>'][..]).to_string();
            write(&mut wh, "250 ok\r\n").await;
        } else if upper.starts_with("RCPT TO:") {
            to.push(line[8..].trim().trim_matches(&['<', '>'][..]).to_string());
            write(&mut wh, "250 ok\r\n").await;
        } else if upper.starts_with("DATA") {
            if !authed {
                write(&mut wh, "530 auth first\r\n").await;
                continue;
            }
            write(&mut wh, "354 end with .\r\n").await;
            let mut raw = vec![];
            loop {
                line.clear();
                if r.read_line(&mut line).await.unwrap_or(0) == 0 {
                    return;
                }
                if line == ".\r\n" || line == ".\n" {
                    break;
                }
                // dot-unstuff
                let l = line.strip_prefix("..").unwrap_or(&line);
                raw.extend_from_slice(l.as_bytes());
            }
            let (f552, f421) = {
                let b = behavior.lock().unwrap();
                (b.fail_data_552, b.fail_data_421_once)
            };
            if f421 {
                behavior.lock().unwrap().fail_data_421_once = false;
            }
            if f552 {
                write(&mut wh, "552 5.3.4 Message too large.\r\n").await;
            } else if f421 {
                write(&mut wh, "421 4.7.0 Temporary failure, try again.\r\n").await;
            } else {
                accepted.lock().unwrap().push(Accepted {
                    from: from.clone(),
                    to: to.clone(),
                    raw,
                });
                write(&mut wh, "250 queued\r\n").await;
            }
        } else if upper.starts_with("RSET") {
            from.clear();
            to.clear();
            write(&mut wh, "250 ok\r\n").await;
        } else if upper.starts_with("QUIT") {
            write(&mut wh, "221 bye\r\n").await;
            return;
        } else if upper.starts_with("NOOP") {
            write(&mut wh, "250 ok\r\n").await;
        } else {
            write(&mut wh, "502 unimplemented\r\n").await;
        }
    }
}
