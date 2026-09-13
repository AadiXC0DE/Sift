//! SMTP send via Gmail (Phase 11 task 11, hardened in P5.3).
//!
//! `lettre` AsyncSmtpTransport to `smtp.gmail.com:465` (implicit TLS; 587
//! STARTTLS fallback when 465 is blocked), AUTH PLAIN with the app password,
//! EHLO `sift.local`, 30 s timeout, connection reused 5 min then dropped.
//!
//! The envelope is explicit and comes from the prepared draft. It used to be
//! scraped out of the raw headers with a comma split, and when that found
//! nothing it **sent the message to the sender** — silently mailing the user
//! their own unsent mail. Now a message whose recipients cannot be determined
//! fails; the envelope is never defaulted to the sender.
//!
//! The DATA bytes never contain a `Bcc` header (recipient privacy): the
//! envelope carries those recipients and [`strip_bcc`] removes the header,
//! with the removal count checked against the prepared message.
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
            "This message is over Sift's current 25 MB send limit.",
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

/// Envelope derived from a raw message, for messages queued before the
/// envelope was stored with them.
///
/// Addresses are parsed with `mail-parser` (never comma-split) and validated.
/// A message with no usable recipient is an error; there is deliberately no
/// fallback to the sender.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SendEnvelope {
    pub from: String,
    pub recipients: Vec<String>,
    pub bcc_count: usize,
}

pub fn envelope_from_raw(raw: &[u8], fallback_from: &str) -> Result<SendEnvelope, SiftError> {
    let parsed = mail_parser::MessageParser::default()
        .parse(raw)
        .ok_or_else(|| SiftError::app("protocol", "the message could not be parsed", false))?;
    let from = parsed
        .from()
        .and_then(|a| a.first())
        .and_then(|a| a.address())
        .map(str::to_string)
        .or_else(|| {
            parsed
                .header("Sender")
                .and_then(|h| h.as_text())
                .map(|s| s.trim().to_string())
        })
        .filter(|f| f.contains('@'))
        .unwrap_or_else(|| fallback_from.to_string());

    let mut recipients: Vec<String> = Vec::new();
    let mut bcc_count = 0usize;
    for (list, is_bcc) in [
        (parsed.to(), false),
        (parsed.cc(), false),
        (parsed.bcc(), true),
    ] {
        let Some(list) = list else { continue };
        for a in list.iter() {
            if let Some(addr) = a.address().map(str::trim).filter(|a| a.contains('@')) {
                let addr = addr.to_string();
                if is_bcc && !recipients.iter().any(|r| r.eq_ignore_ascii_case(&addr)) {
                    bcc_count += 1;
                }
                if !recipients.iter().any(|r| r.eq_ignore_ascii_case(&addr)) {
                    recipients.push(addr);
                }
            }
        }
    }
    if recipients.is_empty() {
        return Err(SiftError::app(
            "bad_recipient",
            "the message has no deliverable recipient",
            false,
        ));
    }
    Ok(SendEnvelope {
        from,
        recipients,
        bcc_count,
    })
}

/// The bytes to send over SMTP: the message without its `Bcc` header.
///
/// `expected_bcc` is the number of `Bcc` headers the prepared message should
/// carry. When the message claims Bcc recipients but none can be removed, that
/// is a hard failure — sending it anyway would leak the blind recipients.
pub fn smtp_data(raw: &[u8], expected_bcc: usize) -> Result<Vec<u8>, SiftError> {
    let (data, removed) = crate::outgoing::strip_bcc(raw);
    if removed < expected_bcc {
        return Err(SiftError::app(
            "bcc_strip_failed",
            "refusing to send: the Bcc header could not be removed",
            false,
        ));
    }
    if String::from_utf8_lossy(&data).to_ascii_lowercase().contains("\nbcc:") {
        return Err(SiftError::app(
            "bcc_strip_failed",
            "refusing to send: a Bcc header survived",
            false,
        ));
    }
    Ok(data)
}

/// Send via Gmail production endpoints (465 implicit TLS, 587 fallback).
pub async fn send_gmail(
    email: &str,
    app_password: &str,
    req: &crate::provider::SendRequest,
) -> Result<(), SiftError> {
    let data = smtp_data(&req.raw, req.bcc_count)?;
    let recipients = recipients_or_error(&req.recipients)?;
    // Try 465 first, then 587 STARTTLS when 465 is blocked.
    match send_one(
        GMAIL_SMTP_HOST,
        465,
        false,
        email,
        app_password,
        &req.from,
        &recipients,
        &data,
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
                    &req.from,
                    &recipients,
                    &data,
                )
                .await
            } else {
                Err(e)
            }
        }
    }
}

fn recipients_or_error(recipients: &[String]) -> Result<Vec<lettre::Address>, SiftError> {
    let mut out = Vec::with_capacity(recipients.len());
    for r in recipients {
        let addr: lettre::Address = r.parse().map_err(|_| {
            SiftError::app("bad_recipient", format!("invalid recipient: {r}"), false)
        })?;
        out.push(addr);
    }
    if out.is_empty() {
        return Err(SiftError::app(
            "bad_recipient",
            "the message has no deliverable recipient",
            false,
        ));
    }
    Ok(out)
}

/// Single-attempt send to an explicit host/port. `starttls` selects explicit
/// STARTTLS (port 587); otherwise implicit TLS. Test-only callers use
/// `plaintext=true` via [`send_test`].
#[allow(clippy::too_many_arguments)]
async fn send_one(
    host: &str,
    port: u16,
    starttls: bool,
    email: &str,
    app_password: &str,
    from: &str,
    to: &[lettre::Address],
    raw: &[u8],
) -> Result<(), SiftError> {
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
    crate::install_crypto_provider();

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
    let envelope = lettre::address::Envelope::new(Some(from_addr), to.to_vec())
        .map_err(|_| SiftError::app("bad_id", "no recipients", false))?;
    transport
        .send_raw(&envelope, raw)
        .await
        .map_err(|e| map_smtp_err(&e.to_string()))?;
    Ok(())
}

/// Test-only send to a plaintext fake (no TLS). Mirrors production envelope
/// handling so tests assert the same bytes Gmail would receive.
#[allow(clippy::too_many_arguments)] // host/port/credentials/raw/envelope: the production shape
pub async fn send_test(
    host: &str,
    port: u16,
    email: &str,
    app_password: &str,
    raw: &[u8],
    from: &str,
    recipients: &[String],
    bcc_count: usize,
) -> Result<(), SiftError> {
    use lettre::transport::smtp::authentication::Credentials;
    use lettre::{AsyncSmtpTransport, AsyncTransport, Tokio1Executor};
    crate::install_crypto_provider();

    let data = smtp_data(raw, bcc_count)?;
    let to = recipients_or_error(recipients)?;
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
    let from_addr: lettre::Address = from
        .parse()
        .map_err(|_| SiftError::app("bad_id", "bad From", false))?;
    let envelope = lettre::address::Envelope::new(Some(from_addr), to)
        .map_err(|_| SiftError::app("bad_id", "bad envelope", false))?;
    transport
        .send_raw(&envelope, &data)
        .await
        .map_err(|e| map_smtp_err(&e.to_string()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_parses_addrs_without_comma_splitting() {
        let raw = b"From: Alice <alice@example.com>\r\nTo: \"Doe, Jane\" <jane@example.com>, carol@example.com\r\nSubject: hi\r\n\r\nbody";
        let e = envelope_from_raw(raw, "fallback@example.com").unwrap();
        assert_eq!(e.from, "alice@example.com");
        assert_eq!(
            e.recipients,
            vec!["jane@example.com".to_string(), "carol@example.com".to_string()]
        );
        assert_eq!(e.bcc_count, 0);
    }

    #[test]
    fn envelope_never_falls_back_to_the_sender() {
        // No recipient at all: a hard failure, not "send it to myself".
        let raw = b"From: Alice <alice@example.com>\r\nSubject: hi\r\n\r\nbody";
        let err = envelope_from_raw(raw, "alice@example.com").unwrap_err();
        assert_eq!(serde_json::to_value(&err).unwrap()["code"], "bad_recipient");
        // A header that exists but holds no usable address is the same.
        let raw = b"From: Alice <alice@example.com>\r\nTo: undeliverable\r\n\r\nbody";
        let err = envelope_from_raw(raw, "alice@example.com").unwrap_err();
        assert_eq!(serde_json::to_value(&err).unwrap()["code"], "bad_recipient");
    }

    #[test]
    fn envelope_keeps_bcc_recipients_and_counts_their_header() {
        let raw = b"From: a@example.com\r\nTo: b@example.com\r\nBcc: c@example.com, b@example.com\r\n\r\nbody";
        let e = envelope_from_raw(raw, "x@example.com").unwrap();
        assert_eq!(
            e.recipients,
            vec!["b@example.com".to_string(), "c@example.com".to_string()]
        );
        assert_eq!(e.bcc_count, 1, "only the address unique to Bcc counts");
    }

    #[test]
    fn smtp_data_drops_bcc_and_refuses_when_it_cannot() {
        let raw = b"From: a@example.com\r\nTo: b@example.com\r\nBcc: c@example.com\r\n\r\nbody";
        let data = smtp_data(raw, 1).unwrap();
        assert!(!String::from_utf8_lossy(&data).contains("Bcc"));
        assert!(String::from_utf8_lossy(&data).contains("body"));
        // Claims a Bcc header that is not there: refuse rather than leak.
        let raw = b"From: a@example.com\r\nTo: b@example.com\r\n\r\nbody";
        let err = smtp_data(raw, 1).unwrap_err();
        assert_eq!(
            serde_json::to_value(&err).unwrap()["code"],
            "bcc_strip_failed"
        );
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
        assert_eq!(serde_json::to_value(&e).unwrap()["code"], "imap_transient");
        assert!(e.is_retryable());
    }
}
