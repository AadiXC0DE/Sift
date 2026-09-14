//! Remote-content privacy state (P9.1).
//!
//! The policy is explicit and versioned:
//!
//! * `block` — nothing external ever loads.
//! * `ask` — the default for a newly configured installation. A read blocks
//!   until the user grants "load once" for that message (a session grant) or
//!   "always for sender" (a persisted, account-scoped preference).
//! * `allow` — loads the HTTPS images a message needs without asking.
//!
//! A database upgraded from a build that treated a legacy ambiguous `ask` as
//! `allow` keeps `allow` **behaviour** and records that the one-time compact
//! privacy choice is still pending, so nothing changes silently on upgrade.
//!
//! Every preference change bumps a per-account *permission generation*. The
//! generation is what a body cache must key on, because a cached rendering
//! was produced under the old permission.

use super::Db;
use crate::dto::RemoteContentMode;
use anyhow::Result;

/// Settings-doc keys. They live in the typed settings document so a settings
/// round-trip cannot drop them.
pub(crate) const MODE_KEY: &str = "remoteContentMode";
pub(crate) const PENDING_KEY: &str = "remoteContentChoicePending";

#[derive(Debug, Clone, PartialEq)]
pub struct PrivacyState {
    /// What the user chose (or the default).
    pub stored_mode: RemoteContentMode,
    /// True while the one-time upgrade choice is unanswered.
    pub choice_pending: bool,
}

impl PrivacyState {
    /// The policy actually used to render: an unanswered upgrade choice keeps
    /// the behaviour the user already had.
    pub fn effective(&self) -> RemoteContentMode {
        if self.choice_pending {
            RemoteContentMode::Allow
        } else {
            self.stored_mode.clone()
        }
    }

    /// Whether a read may load external resources without a decision.
    pub fn loads_without_asking(&self) -> bool {
        self.effective().loads_without_asking()
    }
}

fn generation_key(account_id: &str) -> String {
    format!("privacy_gen:{account_id}")
}

impl Db {
    pub async fn privacy_state(&self) -> Result<PrivacyState> {
        let settings = self.settings_get().await?;
        Ok(PrivacyState {
            stored_mode: RemoteContentMode::parse(&settings.remote_content_mode)
                .unwrap_or(RemoteContentMode::Ask),
            choice_pending: settings.remote_content_choice_pending,
        })
    }

    /// Store a deliberate choice. `settings_set` clears the pending flag and
    /// moves every account's permission generation in the same operation,
    /// because the policy a cached rendering was produced under changed.
    pub async fn privacy_set_mode(&self, mode: RemoteContentMode) -> Result<()> {
        self.settings_set(serde_json::json!({
            MODE_KEY: mode.as_str(),
        }))
        .await?;
        Ok(())
    }

    pub async fn privacy_generation(&self, account_id: &str) -> Result<i64> {
        Ok(self
            .setting_get_raw(&generation_key(account_id))
            .await?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(0))
    }

    /// Bump the permission generation. `None` bumps every account (a global
    /// policy change); `Some(ids)` bumps only the accounts whose own
    /// preference changed.
    pub async fn privacy_bump_generations(&self, account_ids: Option<&[String]>) -> Result<()> {
        let targets: Vec<String> = match account_ids {
            Some(ids) => ids.to_vec(),
            None => self
                .read(|c| {
                    let mut statement = c.prepare("SELECT id FROM accounts")?;
                    let rows: Vec<String> = statement
                        .query_map([], |r| r.get(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    Ok(rows)
                })
                .await?,
        };
        for account in targets {
            let next = self.privacy_generation(&account).await? + 1;
            self.setting_set_raw(&generation_key(&account), &next.to_string())
                .await?;
        }
        Ok(())
    }

    /// Is this sender permanently allowed **for this account**? A sender
    /// preference never leaks between accounts.
    pub async fn sender_allowed(&self, account_id: &str, email: &str) -> Result<bool> {
        let (account, sender) = (
            account_id.to_string(),
            email.trim().to_ascii_lowercase(),
        );
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT allow_remote_images FROM sender_prefs WHERE account_id=? AND email=?",
                rusqlite::params![account, sender],
                |r| r.get::<_, i64>(0),
            )
            .unwrap_or(0)
                != 0)
        })
        .await
    }

    pub async fn sender_allow(&self, account_id: &str, email: &str) -> Result<()> {
        let (account, sender) = (
            account_id.to_string(),
            email.trim().to_ascii_lowercase(),
        );
        if sender.is_empty() {
            anyhow::bail!("cannot remember a sender without an address");
        }
        self.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO sender_prefs (account_id,email,allow_remote_images) VALUES (?,?,1)",
                rusqlite::params![account, sender],
            )?;
            Ok(())
        })
        .await
    }

    /// Revoke a remembered sender. The row is deleted (not merely disabled) so
    /// the allow-list in the privacy panel reflects exactly what is allowed.
    pub async fn sender_revoke(&self, account_id: &str, email: &str) -> Result<()> {
        let (account, sender) = (
            account_id.to_string(),
            email.trim().to_ascii_lowercase(),
        );
        self.write(move |c| {
            c.execute(
                "DELETE FROM sender_prefs WHERE account_id=? AND email=?",
                rusqlite::params![account, sender],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn sender_allow_list(&self, account_id: &str) -> Result<Vec<String>> {
        let account = account_id.to_string();
        self.read(move |c| {
            let mut statement = c.prepare(
                "SELECT email FROM sender_prefs WHERE account_id=? AND allow_remote_images=1 ORDER BY email",
            )?;
            let rows: Vec<String> = statement
                .query_map(rusqlite::params![account], |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }
}
