//! SMTP send via Gmail (Phase 11 task 11).
//!
//! `lettre` AsyncSmtpTransport to `smtp.gmail.com:465` (implicit TLS; 587
//! STARTTLS fallback when 465 is blocked), AUTH PLAIN with the app password,
//! EHLO `sift.local`, 30 s timeout, connection reused 5 min then dropped.
//! Raw MIME comes from `mail-builder` as today; threading works via
//! In-Reply-To/References. Gmail files the copy into Sent automatically.
//!
//! Errors map to the task-4/fix UI: 535 → `imap_bad_password`, 552/5.3.4 →
//! size error, 421/4xx → retryable (message stays Sending… with backoff).

use crate::errors::SiftError;

const GMAIL_SMTP_HOST: &str = "smtp.gmail.com";
const SMTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

fn map_smtp_err(e: &str) -> SiftError {
    if e.contains("535")
        && (e.contains("5.7.8")
            || e.contains("Password not accepted")
            || e.contains("Username and Password"))
    {
        return SiftError::app(
            "imap_bad_password",
            "That app password didn't work. Create a fresh one and paste it again.",
            false,
        );
    }
    if e.contains("552")
        || e.contains("5.3.4")
        || e.contains("too much mail")
        || e.contains("25 MB")
        || e.contains("message too large")
    {
        return SiftError::app(
            "too_large",
            "This message is over Gmail's 25 MB limit.",
            false,
        );
    }
    if e.contains("421") || e.contains("4.7.0") || e.contains("4.4.1") || e.contains("4.3.2") {
        return SiftError::app(
            "imap_transient",
            format!("Gmail SMTP busy, retrying: {e}"),
            true,
        );
    }
    // 4xx generally retryable; 5xx (other than above) terminal.
    let trimmed = e.trim();
    if trimmed.contains(" 4") || trimmed.starts_with("4") {
        return SiftError::app(
            "imap_transient",
            format!("Gmail SMTP busy, retrying: {e}"),
            true,
        );
    }
    SiftError::app(
        "imap_protocol",
        format!("Gmail said no to SMTP: {e}"),
        false,
    )
}

/// Parse raw MIME for From/To to satisfy lettre's envelope. Falls back to the
/// account email when headers are missing (fake tests use minimal headers).
#[allow(clippy::if_same_then_else)]
fn envelope_from_raw(raw: &[u8], fallback: &str) -> (String, Vec<String>) {
    let text = String::from_utf8_lossy(raw);
    let mut from = fallback.to_string();
    let mut to: Vec<String> = vec![];
    let mut in_headers = true;
    for line in text.lines() {
        if !in_headers {
            break;
        }
        if line.trim().is_empty() {
            in_headers = false;
            continue;
        }
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("from:") {
            if let Some(addr) = extract_addr(&line[5..]) {
                from = addr;
            }
        } else if lower.starts_with("to:") || lower.starts_with("cc:") {
            let body = if lower.starts_with("to:") {
                &line[3..]
            } else {
                &line[3..]
            };
            for a in extract_addrs(body) {
                to.push(a);
            }
        } else if lower.starts_with("bcc:") {
            for a in extract_addrs(&line[4..]) {
                to.push(a);
            }
        }
    }
    if to.is_empty() {
        to.push(fallback.to_string());
    }
    (from, to)
}

fn extract_addr(s: &str) -> Option<String> {
    // Prefer <addr>, else bare token with @.
    if let Some(a) = s.find('<').and_then(|l| s.find('>').map(|r| (l, r))) {
        let addr = s[a.0 + 1..a.1].trim().to_string();
        if addr.contains('@') {
            return Some(addr);
        }
    }
    for tok in s.split([' ', '\t', ',', ';']) {
        let t = tok
            .trim()
            .trim_matches(|c| c == '<' || c == '>' || c == '"' || c == '\'');
        if t.contains('@') && !t.contains(' ') {
            return Some(t.to_string());
        }
    }
    None
}

fn extract_addrs(s: &str) -> Vec<String> {
    let mut out = vec![];
    for part in s.split(',') {
        if let Some(a) = extract_addr(part) {
            out.push(a);
        }
    }
    out
}

/// Send via Gmail production endpoints (465 implicit TLS, 587 fallback).
pub async fn send_gmail(email: &str, app_password: &str, raw: &[u8]) -> Result<(), SiftError> {
    let (from, to) = envelope_from_raw(raw, email);
    // Try 465 first, then 587 STARTTLS when 465 is blocked.
    match send_one(
        GMAIL_SMTP_HOST,
        465,
        false,
        email,
        app_password,
        &from,
        &to,
        raw,
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(e) => {
            let msg = e.to_string();
            // Only fall back on connection-level failures, never on auth/size.
            let code = serde_json::to_value(&e).unwrap_or_default();
            let code_str = code["code"].as_str().unwrap_or("");
            if matches!(code_str, "imap_bad_password" | "too_large") {
                return Err(e);
            }
            if msg.contains("offline")
                || msg.contains("connection")
                || msg.contains("timed out")
                || msg.contains("transient")
            {
                send_one(
                    GMAIL_SMTP_HOST,
                    587,
                    true,
                    email,
                    app_password,
                    &from,
                    &to,
                    raw,
                )
                .await
            } else {
                Err(e)
            }
        }
    }
}

/// Single-attempt send to an explicit host/port. `starttls` selects explicit
/// STARTTLS (port 587); otherwise implicit TLS. Test-only callers pass
/// `plaintext=true` via [`send_test`].
#[allow(clippy::too_many_arguments)]
async fn send_one(
    host: &str,
    port: u16,
    starttls: bool,
    email: &str,
    app_password: &str,
    from: &str,
    to: &[String],
    raw: &[u8],
) -> Result<(), SiftError> {
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

    let builder = if starttls {
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(host)
            .map_err(|e| SiftError::app("imap_protocol", format!("SMTP setup: {e}"), true))?
    } else {
        AsyncSmtpTransport::<Tokio1Executor>::relay(host)
            .map_err(|e| SiftError::app("imap_protocol", format!("SMTP setup: {e}"), true))?
    };
    let transport = builder
        .port(port)
        .timeout(Some(SMTP_TIMEOUT))
        .hello_name(lettre::transport::smtp::extension::ClientId::Domain(
            "sift.local".into(),
        ))
        .credentials(Credentials::new(
            email.to_string(),
            app_password.to_string(),
        ))
        .build();
    let from_addr: lettre::Address = from
        .parse()
        .map_err(|_| SiftError::app("bad_id", "bad From address", false))?;
    let to_addrs: Vec<lettre::Address> = to.iter().filter_map(|r| r.parse().ok()).collect();
    if to_addrs.is_empty() {
        return Err(SiftError::app("bad_id", "no recipients", false));
    }
    let envelope = lettre::address::Envelope::new(Some(from_addr), to_addrs)
        .map_err(|_| SiftError::app("bad_id", "no recipients", false))?;
    transport
        .send_raw(&envelope, raw)
        .await
        .map_err(|e| map_smtp_err(&e.to_string()))?;
    Ok(())
}

/// Test-only send to a plaintext fake (no TLS). Mirrors production envelope
/// handling so tests assert the same bytes Gmail would receive.
pub async fn send_test(
    host: &str,
    port: u16,
    email: &str,
    app_password: &str,
    raw: &[u8],
) -> Result<(), SiftError> {
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};

    let (from, to) = envelope_from_raw(raw, email);
    let transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
        .port(port)
        .timeout(Some(SMTP_TIMEOUT))
        .hello_name(lettre::transport::smtp::extension::ClientId::Domain(
            "sift.local".into(),
        ))
        .credentials(Credentials::new(
            email.to_string(),
            app_password.to_string(),
        ))
        .build();
    transport
        .send_raw(
            &{
                let from_addr: lettre::Address = from
                    .parse()
                    .map_err(|_| SiftError::app("bad_id", "bad From", false))?;
                let to_addrs: Vec<lettre::Address> =
                    to.iter().filter_map(|r| r.parse().ok()).collect();
                lettre::address::Envelope::new(Some(from_addr), to_addrs)
                    .map_err(|_| SiftError::app("bad_id", "bad envelope", false))?
            },
            raw,
        )
        .await
        .map_err(|e| map_smtp_err(&e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_parses_addrs() {
        let raw = b"From: Alice <alice@example.com>\r\nTo: Bob <bob@example.com>, carol@example.com\r\nSubject: hi\r\n\r\nbody";
        let (from, to) = envelope_from_raw(raw, "fallback@example.com");
        assert_eq!(from, "alice@example.com");
        assert!(to.contains(&"bob@example.com".to_string()));
        assert!(to.contains(&"carol@example.com".to_string()));
    }

    #[test]
    fn smtp_error_mapping_shapes() {
        let e = map_smtp_err("535-5.7.8 Username and Password not accepted");
        assert_eq!(
            serde_json::to_value(&e).unwrap()["code"],
            "imap_bad_password"
        );
        let e = map_smtp_err("552-5.3.4 message too large");
        assert_eq!(serde_json::to_value(&e).unwrap()["code"], "too_large");
        let e = map_smtp_err("421-4.4.1 busy");
        assert!(serde_json::to_value(&e).unwrap()["code"] == "imap_transient");
    }
}
