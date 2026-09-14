//! Native connectivity state (P4.6).
//!
//! One state per account: `online`, `offline`, `degraded`, `reauth_required`.
//! The browser's `navigator.onLine` hint is only a *hint*; the evidence is the
//! outcome of real provider operations. A cached read never consults this -
//! it answers from local rows and blocks on nothing.
//!
//! Rules:
//! * the network hint going down marks every account offline at once;
//! * a provider operation that succeeds clears THAT account's error only;
//! * a connectivity failure (offline/timeout/transient) marks the account
//!   offline, which pauses further background network work until the hint or a
//!   user-initiated retry says otherwise;
//! * an authentication failure marks the account `reauth_required` and stays
//!   until a later operation succeeds.
use crate::errors::SiftError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

pub const ONLINE: &str = "online";
pub const OFFLINE: &str = "offline";
pub const DEGRADED: &str = "degraded";
pub const REAUTH_REQUIRED: &str = "reauth_required";

/// Codes that mean "the account must be reconnected", not "the network is
/// down": these are sticky until the user repairs the account.
const AUTH_CODES: [&str; 5] = [
    "reauth",
    "imap_needs_app_password",
    "imap_bad_password",
    "imap_web_login_required",
    "imap_disabled_by_admin",
];

/// Codes that are not evidence about connectivity at all (a local refusal, a
/// budget pause, a cancellation) and must never change the reported state.
const IGNORED_CODES: &[&str] = &[
    "cancelled",
    "account_cancelled",
    "backfill_budget",
    "not_in_trash",
    "attachment_locator_invalid",
    "no_recipients",
    "bad_recipient",
    "attachment_missing",
    "too_large",
    "storage",
    "db",
    "payload_invalid",
    "unsupported_operation",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectivityState {
    #[serde(rename = "accountId")]
    pub account_id: String,
    /// online | offline | degraded | reauth_required
    pub state: String,
    #[serde(rename = "lastOkAt")]
    pub last_ok_at: Option<i64>,
    #[serde(rename = "lastError")]
    pub last_error: Option<String>,
    /// Unix ms when the account entered this state.
    pub since: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FailureKind {
    Connectivity,
    Auth,
    Other,
}

#[derive(Default)]
struct Account {
    last_ok_at: Option<i64>,
    last_error: Option<String>,
    failure: Option<FailureKind>,
    since: i64,
}

pub struct Connectivity {
    /// Network reachability hint from the host (frontend `online`/`offline`).
    network: AtomicBool,
    accounts: Mutex<HashMap<String, Account>>,
}

impl Default for Connectivity {
    fn default() -> Self {
        Self::new()
    }
}

impl Connectivity {
    pub fn new() -> Self {
        Self {
            network: AtomicBool::new(true),
            accounts: Mutex::new(HashMap::new()),
        }
    }

    /// Record the host's reachability hint. Returns true when it changed.
    pub fn set_network_reachable(&self, reachable: bool) -> bool {
        self.network.swap(reachable, Ordering::SeqCst) != reachable
    }

    pub fn network_reachable(&self) -> bool {
        self.network.load(Ordering::SeqCst)
    }

    /// A provider operation succeeded: clears THIS account's error only.
    /// Returns true when the reported state changed.
    pub fn record_ok(&self, account_id: &str, at: i64) -> bool {
        let before = self.state_for(account_id, at);
        {
            let mut map = self.accounts.lock().unwrap();
            let entry = map.entry(account_id.to_string()).or_default();
            entry.last_ok_at = Some(at);
            entry.last_error = None;
            entry.failure = None;
            if entry.since == 0 {
                entry.since = at;
            }
        }
        self.state_for(account_id, at) != before
    }

    /// A provider operation failed. Returns true when the state changed.
    pub fn record_failure(&self, account_id: &str, error: &SiftError, at: i64) -> bool {
        let code = error_code(error);
        if IGNORED_CODES.contains(&code.as_str()) {
            return false;
        }
        let kind = if AUTH_CODES.contains(&code.as_str()) || error.is_reauth() {
            FailureKind::Auth
        } else if is_connectivity_error(error) {
            FailureKind::Connectivity
        } else {
            FailureKind::Other
        };
        let before = self.state_for(account_id, at);
        {
            let mut map = self.accounts.lock().unwrap();
            let entry = map.entry(account_id.to_string()).or_default();
            // A sticky auth failure outranks a later transient blip: only a
            // successful operation (or reauth) clears it.
            if !(entry.failure == Some(FailureKind::Auth) && kind != FailureKind::Auth) {
                entry.failure = Some(kind);
                entry.last_error = Some(error.to_string());
            }
            if entry.since == 0 {
                entry.since = at;
            }
        }
        self.state_for(account_id, at) != before
    }

    /// Mark an account as needing the user's repair (reauth), independent of a
    /// specific provider call.
    pub fn require_reauth(&self, account_id: &str, at: i64) -> bool {
        let before = self.state_for(account_id, at);
        {
            let mut map = self.accounts.lock().unwrap();
            let entry = map.entry(account_id.to_string()).or_default();
            entry.failure = Some(FailureKind::Auth);
            entry.last_error = Some("Reconnect this account to continue syncing.".to_string());
            if entry.since == 0 {
                entry.since = at;
            }
        }
        self.state_for(account_id, at) != before
    }

    /// Forget an account (removal), so no stale error survives it.
    pub fn forget(&self, account_id: &str) {
        self.accounts.lock().unwrap().remove(account_id);
    }

    /// The current state of one account.
    pub fn state_for(&self, account_id: &str, at: i64) -> ConnectivityState {
        let entry = self.accounts.lock().unwrap();
        let account = entry.get(account_id);
        let state = resolve(self.network.load(Ordering::SeqCst), account);
        ConnectivityState {
            account_id: account_id.to_string(),
            state: state.to_string(),
            last_ok_at: account.and_then(|a| a.last_ok_at),
            last_error: account.and_then(|a| a.last_error.clone()),
            since: account.map(|a| a.since).unwrap_or(at),
        }
    }

    pub fn snapshot(&self, account_ids: &[String], at: i64) -> Vec<ConnectivityState> {
        account_ids.iter().map(|a| self.state_for(a, at)).collect()
    }

    /// Whether background network work should pause for this account: we are
    /// offline, or the account needs the user's attention. A user-initiated
    /// action may still try (and will fail with a clear error).
    pub fn background_paused(&self, account_id: &str) -> bool {
        matches!(
            self.state_for(account_id, 0).state.as_str(),
            OFFLINE | REAUTH_REQUIRED
        )
    }
}

fn resolve(network: bool, account: Option<&Account>) -> &'static str {
    if !network {
        return OFFLINE;
    }
    match account.and_then(|a| a.failure) {
        Some(FailureKind::Auth) => REAUTH_REQUIRED,
        Some(FailureKind::Connectivity) => OFFLINE,
        Some(FailureKind::Other) => DEGRADED,
        None => ONLINE,
    }
}

fn error_code(error: &SiftError) -> String {
    match error {
        SiftError::App { code, .. } | SiftError::Typed { code, .. } => code.clone(),
        SiftError::Http(_) => "http".into(),
        SiftError::Io(_) => "io".into(),
        SiftError::Db(_) => "db".into(),
        SiftError::Json(_) => "json".into(),
        SiftError::Keyring(_) => "keyring".into(),
        SiftError::Oauth(_) => "oauth".into(),
        SiftError::NotFound(_) => "not_found".into(),
    }
}

/// A failure that says something about reaching the provider at all.
fn is_connectivity_error(error: &SiftError) -> bool {
    match error {
        SiftError::App { code, .. } | SiftError::Typed { code, .. } => matches!(
            code.as_str(),
            "offline"
                | "imap_transient"
                | "imap_protocol"
                | "attachment_offline"
                | "attachment_timeout"
                | "oauth_timeout"
        ),
        SiftError::Http(e) => {
            e.is_timeout()
                || e.is_connect()
                || e.status()
                    .map(|s| s.as_u16() == 429 || s.as_u16() >= 500)
                    .unwrap_or(true)
        }
        SiftError::Io(_) => true,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at() -> i64 {
        1_700_000_000_000
    }

    fn err(code: &str, retryable: bool) -> SiftError {
        SiftError::app(code, format!("{code} failed"), retryable)
    }

    #[test]
    fn p46_states_and_recovery_are_per_account() {
        let c = Connectivity::new();
        assert_eq!(c.state_for("a", at()).state, ONLINE);

        // One bad account beside a healthy one: only the bad one degrades.
        assert!(c.record_failure("a", &err("imap_transient", true), at()));
        assert!(c.record_ok("b", at()));
        assert_eq!(c.state_for("a", at()).state, OFFLINE);
        assert_eq!(c.state_for("b", at()).state, ONLINE);
        assert!(c.background_paused("a"));
        assert!(!c.background_paused("b"));

        // Recovery clears only the affected account's error.
        assert!(c.record_ok("a", at() + 1));
        assert_eq!(c.state_for("a", at()).state, ONLINE);
        assert!(c.state_for("a", at()).last_error.is_none());
        assert_eq!(c.state_for("a", at()).last_ok_at, Some(at() + 1));
    }

    #[test]
    fn p46_network_hint_pauses_everything_and_auth_is_sticky() {
        let c = Connectivity::new();
        c.record_failure("a", &err("attachment_timeout", true), at());
        assert_eq!(c.state_for("a", at()).state, OFFLINE);
        // The host says the network is gone: everyone is offline.
        assert!(c.set_network_reachable(false));
        assert_eq!(c.state_for("b", at()).state, OFFLINE);
        assert!(!c.set_network_reachable(false), "no change, no re-emit");
        assert!(c.set_network_reachable(true));
        assert_eq!(c.state_for("a", at()).state, OFFLINE, "evidence stays");
        assert_eq!(c.state_for("b", at()).state, ONLINE);

        // Auth failure is sticky: a later transient blip must not downgrade it
        // to a plain connectivity error.
        assert!(c.record_failure("b", &err("reauth", false), at()));
        assert_eq!(c.state_for("b", at()).state, REAUTH_REQUIRED);
        c.record_failure("b", &err("imap_transient", true), at() + 1);
        assert_eq!(c.state_for("b", at()).state, REAUTH_REQUIRED);
        assert!(c.record_ok("b", at() + 2));
        assert_eq!(c.state_for("b", at()).state, ONLINE);

        // A non-network failure is degraded, not offline: work may continue.
        assert!(c.record_failure("a", &err("imap_error", false), at() + 3));
        assert_eq!(c.state_for("a", at()).state, DEGRADED);
        assert!(!c.background_paused("a"));

        assert!(!c.record_failure("a", &err("no_recipients", false), at() + 4));
        assert_eq!(c.state_for("a", at()).state, DEGRADED);

        // Local refusals and cancellations are not connectivity evidence.
        assert!(!c.record_failure("a", &err("cancelled", true), at() + 4));
        assert_eq!(c.state_for("a", at()).state, DEGRADED);
        assert!(!c.record_failure("a", &err("backfill_budget", false), at() + 5));
        assert_eq!(c.state_for("a", at()).state, DEGRADED);

        c.forget("a");
        assert_eq!(c.state_for("a", at()).state, ONLINE);
    }
}
