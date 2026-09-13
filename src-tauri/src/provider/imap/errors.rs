//! IMAP/SMTP error mapping (Phase 11 task 4).
//!
//! Every server string in the task-4 table maps to a `SiftError{code}` that
//! drives the guided fix in the setup wizard (Step D) and the re-auth banner.
//! `code` values are part of the IPC contract - do not rename without
//! updating the wizard copy table.
use crate::errors::SiftError;
use super::proto::ResponseCode;

fn app(code: &str, message: impl Into<String>, retryable: bool) -> SiftError {
    SiftError::app(code, message, retryable)
}

/// Tagged completion kind of a response (P1.6). Kept separate from
/// `TaggedOutcome` so error mapping and the fetch path can both use it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Completion {
    Ok,
    No,
    Bad,
}

impl Completion {
    pub fn label(self) -> &'static str {
        match self {
            Completion::Ok => "OK",
            Completion::No => "NO",
            Completion::Bad => "BAD",
        }
    }

    /// Syntax/usage failures are terminal: retrying cannot fix them.
    pub fn is_terminal(self) -> bool {
        self == Completion::Bad
    }
}

/// Public attachment error codes (P1.6). Part of the IPC contract - do not
/// rename without updating the UI copy table.
pub mod codes {
    pub const LOCATOR_INVALID: &str = "attachment_locator_invalid";
    pub const MESSAGE_MISSING: &str = "attachment_message_missing";
    pub const PART_MISSING: &str = "attachment_part_missing";
    pub const DECODE_FAILED: &str = "attachment_decode_failed";
    pub const OFFLINE: &str = "attachment_offline";
    pub const TIMEOUT: &str = "attachment_timeout";
    pub const WRITE_FAILED: &str = "attachment_write_failed";
    pub const CANCELLED: &str = "attachment_cancelled";
    /// Reconnect landed on a different UIDVALIDITY epoch (P1.3).
    pub const UIDVALIDITY_CHANGED: &str = "imap_uidvalidity_changed";
}

/// Codes that must pass through the attachment wrapper untouched: repairing
/// the account (or backing off) is the only correct reaction.
pub fn is_passthrough(code: &str) -> bool {
    matches!(
        code,
        "imap_bad_password"
            | "imap_needs_app_password"
            | "imap_web_login_required"
            | "imap_disabled_by_admin"
            | "imap_too_many_connections"
            | "imap_tls"
            | "imap_all_mail_hidden"
            | "reauth"
            | codes::UIDVALIDITY_CHANGED
    )
}

/// Machine response code as a short, content-free token (never the server's
/// human text, which can quote mail).
pub fn code_token(code: Option<&ResponseCode>) -> String {
    let raw = match code {
        None => return "-".into(),
        Some(ResponseCode::AppendUid { .. }) => "APPENDUID".to_string(),
        Some(ResponseCode::CopyUid { .. }) => "COPYUID".to_string(),
        Some(ResponseCode::UidValidity(_)) => "UIDVALIDITY".to_string(),
        Some(ResponseCode::UidNext(_)) => "UIDNEXT".to_string(),
        Some(ResponseCode::HighestModSeq(_)) => "HIGHESTMODSEQ".to_string(),
        Some(ResponseCode::ReadWrite) => "READ-WRITE".to_string(),
        Some(ResponseCode::ReadOnly) => "READ-ONLY".to_string(),
        Some(ResponseCode::Alert(_)) => "ALERT".to_string(),
        Some(ResponseCode::Capabilities(_)) => "CAPABILITY".to_string(),
        Some(ResponseCode::PermanentFlags(_)) => "PERMANENTFLAGS".to_string(),
        Some(ResponseCode::Other { kind, .. }) => kind.clone(),
    };
    let mut out: String = raw
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
        .take(24)
        .collect();
    if out.is_empty() {
        out.push_str("code");
    }
    out
}

fn token(s: &str, max: usize) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_'))
        .take(max)
        .collect()
}

/// Caller-supplied, content-free context for an operation's error detail.
pub struct OpCtx<'a> {
    /// Command shape only, e.g. `FETCH`, `SEARCH`, `EXAMINE`.
    pub command: &'a str,
    /// Operation stage, e.g. `locator-read`, `section-read`, `resolve`.
    pub stage: &'a str,
    /// Sanitized correlation id from [`correlation_id`].
    pub correlation: &'a str,
}

/// Redacted developer detail, e.g. `FETCH BAD at section-read (code=ALERT
/// ref att-1a2b3c4d)`. Never contains credentials, subject, recipient,
/// folder name, query or body bytes.
pub fn detail(
    ctx: &OpCtx<'_>,
    completion: Completion,
    code: Option<&ResponseCode>,
    _text: &str,
) -> String {
    format!(
        "{} {} at {} (code={} ref {})",
        token(ctx.command, 32).trim(),
        completion.label(),
        token(ctx.stage, 32).trim(),
        code_token(code),
        token(ctx.correlation, 24).trim(),
    )
}

/// Task-4 table mapping that also preserves completion kind, machine
/// response code, stage and correlation id. Use for any tagged failure whose
/// cause the user should be able to act on.
pub fn map_tagged(
    ctx: &OpCtx<'_>,
    completion: Completion,
    code: Option<&ResponseCode>,
    text: &str,
) -> SiftError {
    let base = match map_response(ctx.command, text) {
        Err(e) => e,
        Ok(()) => app("imap_error", "Gmail rejected the command.", false),
    };
    let d = detail(ctx, completion, code, text);
    match base {
        SiftError::App {
            code,
            message,
            retryable,
        } => app(&code, format!("{message} [{d}]"), retryable),
        other => other,
    }
}

/// Opaque per-operation correlation id derived from account/message/part.
/// Stable for the same operation, reveals no mail content, and is safe to
/// include in diagnostics.
pub fn correlation_id(account_id: &str, message_id: &str, part: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    account_id.hash(&mut h);
    message_id.hash(&mut h);
    part.hash(&mut h);
    format!("att-{:08x}", h.finish() as u32)
}

// -- public attachment codes (P1.6) -----------------------------------------

pub fn attachment_locator_invalid(d: &str) -> SiftError {
    app(
        codes::LOCATOR_INVALID,
        format!("Sift couldn't request this attachment. Update Sift or export diagnostics. [{d}]"),
        false,
    )
}

pub fn attachment_message_missing(d: &str) -> SiftError {
    app(
        codes::MESSAGE_MISSING,
        format!("This message isn't in Gmail any more. [{d}]"),
        false,
    )
}

pub fn attachment_part_missing(d: &str) -> SiftError {
    app(
        codes::PART_MISSING,
        format!("This attachment is no longer part of the message. [{d}]"),
        false,
    )
}

pub fn attachment_decode_failed(d: &str) -> SiftError {
    app(
        codes::DECODE_FAILED,
        format!("Sift couldn't decode this attachment. [{d}]"),
        false,
    )
}

pub fn attachment_offline(d: &str) -> SiftError {
    app(
        codes::OFFLINE,
        format!("No connection to Gmail. [{d}]"),
        true,
    )
}

pub fn attachment_timeout(d: &str) -> SiftError {
    app(
        codes::TIMEOUT,
        format!("This attachment is taking too long to download. Check your connection and try again. [{d}]"),
        true,
    )
}

pub fn attachment_write_failed(d: &str) -> SiftError {
    app(
        codes::WRITE_FAILED,
        format!("Sift couldn't save this attachment. [{d}]"),
        false,
    )
}

pub fn attachment_cancelled(d: &str) -> SiftError {
    app(
        codes::CANCELLED,
        format!("Attachment download cancelled. [{d}]"),
        false,
    )
}

pub fn uidvalidity_changed(d: &str) -> SiftError {
    app(
        codes::UIDVALIDITY_CHANGED,
        format!("Gmail reset this mailbox while Sift was reading it. [{d}]"),
        false,
    )
}

/// Attachment-layer mapping of a tagged NO/BAD. Preserves reauth/throttle
/// codes verbatim, is terminal for BAD, and otherwise reports the missing
/// part. Transient NO stays retryable via [`attachment_timeout`].
pub fn attachment_from_tagged(
    ctx: &OpCtx<'_>,
    completion: Completion,
    code: Option<&ResponseCode>,
    text: &str,
) -> SiftError {
    let mapped = map_tagged(ctx, completion, code, text);
    if let SiftError::App {
        code: c,
        message,
        retryable,
    } = mapped
    {
        let d = detail(ctx, completion, code, text);
        if is_passthrough(&c) {
            return app(&c, message, retryable);
        }
        if c == "imap_transient" {
            return attachment_timeout(&d);
        }
        return match completion {
            Completion::Bad => attachment_locator_invalid(&d),
            _ if looks_nonexistent(text) => attachment_message_missing(&d),
            _ => attachment_part_missing(&d),
        };
    }
    mapped
}

/// Attachment-layer mapping of a transport failure.
pub fn attachment_from_conn(e: SiftError) -> SiftError {
    if let SiftError::App { code, message, .. } = &e {
        if is_passthrough(code) {
            return e;
        }
        if code == "offline" || code == "imap_transient" || code == "imap_protocol" {
            return attachment_offline(message);
        }
    }
    e
}

/// Map a tagged `NO`/`BAD` response to the task-4 table. `command` names the
/// IMAP command for the fallback message. Returns `None` when the text looks
/// like a vanished-UID no-op (caller maps to `AlreadyApplied`).
pub fn map_response(command: &str, text: &str) -> Result<(), SiftError> {
    let t = text;
    if t.contains("Application-specific password required") {
        return Err(app(
            "imap_needs_app_password",
            "Google needs an app password here, not your normal password.",
            false,
        ));
    }
    if t.contains("[AUTHENTICATIONFAILED]") || t.contains("Invalid credentials") {
        return Err(app(
            "imap_bad_password",
            "That app password didn't work. Create a fresh one and paste it again.",
            false,
        ));
    }
    if t.contains("Web login required") || t.contains("log in via your web browser") {
        return Err(app(
            "imap_web_login_required",
            "Google wants you to sign in on the web once. Open Gmail in your browser, then try again.",
            false,
        ));
    }
    if t.contains("IMAP access is disabled") {
        return Err(app(
            "imap_disabled_by_admin",
            "IMAP is turned off for this account by your Google Workspace admin. Ask them to enable IMAP, or use Sign in with Google if available.",
            false,
        ));
    }
    if t.contains("Too many simultaneous connections") {
        return Err(app(
            "imap_too_many_connections",
            "Gmail says too many apps are connected. Quit other mail apps and retry.",
            true,
        ));
    }
    if t.contains("[UNAVAILABLE]")
        || t.contains("Temporary System Problem")
        || t.contains("[SERVERBUG]")
    {
        return Err(app("imap_transient", "Gmail hiccuped; retrying.", true));
    }
    Err(app(
        "imap_error",
        format!("Gmail said no to {command}: {t}"),
        false,
    ))
}

/// Does this NO/BAD text mean "already applied" (vanished UID, duplicate
/// label op)? Callers map it to `Ok(AlreadyApplied)` instead of an error.
pub fn looks_nonexistent(text: &str) -> bool {
    let t = text.to_uppercase();
    t.contains("NONEXISTENT")
        || t.contains("DOES NOT EXIST")
        || t.contains("NO SUCH MESSAGE")
        || t.contains("COULD NOT BE COMPLETED")
        || t.contains("INVALID MESSAGE")
}

/// IO failure on a live connection: timeouts/refusals are `offline` (existing
/// offline bar), resets and truncated reads are transient (reconnect path).
pub fn io_err(e: std::io::Error) -> SiftError {
    use std::io::ErrorKind as K;
    match e.kind() {
        K::TimedOut | K::ConnectionRefused | K::ConnectionAborted | K::NotConnected => {
            app("offline", "No connection to Gmail.", true)
        }
        _ => app("imap_transient", format!("Connection trouble: {e}"), true),
    }
}

/// TLS failure: proxy/firewall interference, never retried blindly.
pub fn tls_err(_e: impl ToString) -> SiftError {
    app(
        "imap_tls",
        "The connection to Gmail isn't secure (a proxy or firewall may be interfering).",
        false,
    )
}

/// Redact credentials from a command line before it touches any log.
pub fn redact_cmd(line: &str) -> String {
    // `TAG LOGIN user secret` -> `TAG LOGIN <redacted>`
    let mut parts = line.splitn(3, ' ');
    let (tag, verb) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    if verb.eq_ignore_ascii_case("LOGIN") {
        format!("{tag} LOGIN <redacted>")
    } else {
        line.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p11_errors_map_task4_table() {
        let cases = [
            (
                "[AUTHENTICATIONFAILED] Invalid credentials (Failure)",
                "imap_bad_password",
                false,
            ),
            (
                "[ALERT] Application-specific password required: https://support.google.com/accounts/answer/185833 (Failure)",
                "imap_needs_app_password",
                false,
            ),
            (
                "[ALERT] Please log in via your web browser: https://support.google.com/mail/accounts/answer/78754 (Failure)",
                "imap_web_login_required",
                false,
            ),
            (
                "[ALERT] IMAP access is disabled for your domain. Please contact your domain administrator. (Failure)",
                "imap_disabled_by_admin",
                false,
            ),
            (
                "Too many simultaneous connections. (Failure)",
                "imap_too_many_connections",
                true,
            ),
            (
                "[UNAVAILABLE] Temporary System Problem. Please try again later. (Failure)",
                "imap_transient",
                true,
            ),
            (
                "[SERVERBUG] Temporary System Problem. Please try again later. (Failure)",
                "imap_transient",
                true,
            ),
        ];
        for (text, code, retryable) in cases {
            let e = map_response("LOGIN", text).unwrap_err();
            let v = serde_json::to_value(&e).unwrap();
            assert_eq!(v["code"], code, "for {text}");
            assert_eq!(v["retryable"], retryable, "for {text}");
        }
    }

    #[test]
    fn p11_errors_redact_login() {
        assert_eq!(
            redact_cmd("a001 LOGIN me@x.com hunter2"),
            "a001 LOGIN <redacted>"
        );
        assert_eq!(
            redact_cmd("a002 SELECT \"[Gmail]/Alle Nachrichten\""),
            "a002 SELECT \"[Gmail]/Alle Nachrichten\""
        );
    }

    #[test]
    fn p1_t06_attachment_codes_by_completion_and_response() {
        let ctx = OpCtx {
            command: "FETCH",
            stage: "section-read",
            correlation: "att-0badc0de",
        };
        let cases: [(&str, Completion, Option<ResponseCode>, &str, bool); 6] = [
            // A syntax (BAD) failure is terminal.
            (
                "syntax error",
                Completion::Bad,
                None,
                codes::LOCATOR_INVALID,
                false,
            ),
            // No such message / part.
            (
                "NO [NONEXISTENT] Hmm",
                Completion::No,
                None,
                codes::MESSAGE_MISSING,
                false,
            ),
            (
                "NO Some messages could not be completed",
                Completion::No,
                None,
                codes::MESSAGE_MISSING,
                false,
            ),
            // Transient server trouble is a bounded retry.
            (
                "NO [UNAVAILABLE] Temporary System Problem.",
                Completion::No,
                None,
                codes::TIMEOUT,
                true,
            ),
            // Reauth and throttle codes pass through untouched.
            (
                "[AUTHENTICATIONFAILED] Invalid credentials",
                Completion::No,
                None,
                "imap_bad_password",
                false,
            ),
            (
                "Too many simultaneous connections. (Failure)",
                Completion::No,
                None,
                "imap_too_many_connections",
                true,
            ),
        ];
        for (text, completion, code, want, retry) in cases {
            let e = attachment_from_tagged(&ctx, completion, code.as_ref(), text);
            let v = serde_json::to_value(&e).unwrap();
            assert_eq!(v["code"], want, "for {text}");
            assert_eq!(v["retryable"], retry, "for {text}");
            let msg = v["message"].as_str().unwrap();
            assert!(
                !msg.contains("Invalid credentials") && !msg.contains("Hmm"),
                "server text must not leak into {msg:?}"
            );
        }
        // A BAD carries the redacted developer detail.
        let e = attachment_from_tagged(
            &ctx,
            Completion::Bad,
            Some(&ResponseCode::Alert("x".into())),
            "syntax error",
        );
        let msg = serde_json::to_value(&e).unwrap()["message"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(msg.contains("BAD at section-read"), "{msg}");
        assert!(msg.contains("code=ALERT"), "{msg}");
        assert!(msg.contains("ref att-0badc0de"), "{msg}");
    }

    #[test]
    fn p1_t06_attachment_from_conn_maps_transport() {
        let offline = attachment_from_conn(app("offline", "No connection to Gmail.", true));
        let v = serde_json::to_value(&offline).unwrap();
        assert_eq!(v["code"], codes::OFFLINE);
        assert_eq!(v["retryable"], true);
        // Reauth is a passthrough: the account repair route must survive.
        let reauth = attachment_from_conn(SiftError::reauth("sign in again"));
        assert_eq!(serde_json::to_value(&reauth).unwrap()["code"], "reauth");
    }

    #[test]
    fn p1_t06_correlation_id_is_opaque_and_stable() {
        let a = correlation_id("acct-1", "abcdef", "2");
        assert!(a.starts_with("att-"), "{a}");
        assert_eq!(a, correlation_id("acct-1", "abcdef", "2"));
        assert_ne!(a, correlation_id("acct-1", "abcdef", "2.1"));
        assert_ne!(a, correlation_id("acct-2", "abcdef", "2"));
        for leak in ["acct-1", "abcdef"] {
            assert!(!a.contains(leak), "id must not embed {leak}: {a}");
        }
    }

    #[test]
    fn p1_t06_bad_is_terminal_no_is_not() {
        assert!(Completion::Bad.is_terminal());
        assert!(!Completion::No.is_terminal());
        assert_eq!(Completion::Bad.label(), "BAD");
    }

    #[test]
    fn p11_errors_nonexistent_shapes() {
        assert!(looks_nonexistent("NO [NONEXISTENT] Hmm"));
        assert!(looks_nonexistent("NO Some messages could not be completed"));
        assert!(!looks_nonexistent(
            "[AUTHENTICATIONFAILED] Invalid credentials"
        ));
    }
}
