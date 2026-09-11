//! IMAP/SMTP error mapping (Phase 11 task 4).
//!
//! Every server string in the task-4 table maps to a `SiftError{code}` that
//! drives the guided fix in the setup wizard (Step D) and the re-auth banner.
//! `code` values are part of the IPC contract - do not rename without
//! updating the wizard copy table.
use crate::errors::SiftError;

fn app(code: &str, message: impl Into<String>, retryable: bool) -> SiftError {
    SiftError::app(code, message, retryable)
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
    fn p11_errors_nonexistent_shapes() {
        assert!(looks_nonexistent("NO [NONEXISTENT] Hmm"));
        assert!(looks_nonexistent("NO Some messages could not be completed"));
        assert!(!looks_nonexistent(
            "[AUTHENTICATIONFAILED] Invalid credentials"
        ));
    }
}
