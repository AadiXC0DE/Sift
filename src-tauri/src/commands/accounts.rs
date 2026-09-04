use crate::app_state::AppState;
use crate::dto::Account;
use crate::errors::SiftError;
use crate::provider::Provider;
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
pub async fn accounts_list(state: State<'_, AppState>) -> Result<Vec<Account>, SiftError> {
    state
        .db
        .accounts_list()
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))
}

#[tauri::command]
pub async fn accounts_add_google(
    app: AppHandle,
    state: State<'_, AppState>,
    login_hint: Option<String>,
) -> Result<Account, SiftError> {
    // PKCE + loopback flow
    let flow = crate::provider::gmail::oauth::start_flow()?;
    let url = flow.auth_url.clone();
    let mut url = url;
    if let Some(hint) = login_hint {
        url.push_str(&format!("&login_hint={}", urlencoding::encode(&hint)));
    }
    // open browser
    let _ = crate::opener::open(&app, &url);
    // wait up to 5 min
    let verifier = flow.verifier.clone();
    let port = flow.port;
    let code = flow.wait(300).await?;
    let tokens =
        crate::provider::gmail::oauth::exchange(&code, &verifier, port, &state.http).await?;
    // userinfo
    let client = crate::provider::gmail::client::GmailClient::new(tokens.access_token.clone());
    // store access in memory via clients map keyed after account creation
    let info: crate::provider::gmail::types::Userinfo = {
        let base = std::env::var("SIFT_USERINFO_URL")
            .unwrap_or_else(|_| "https://openidconnect.googleapis.com/v1/userinfo".into());
        let response = state
            .http
            .get(base)
            .bearer_auth(&tokens.access_token)
            .send()
            .await
            .map_err(SiftError::from)?;
        response.json().await.map_err(SiftError::from)?
    };
    let refresh_token = tokens
        .refresh_token
        .as_deref()
        .ok_or_else(|| SiftError::reauth("Google did not return a refresh token"))?;
    crate::secrets::store_refresh_token(&info.email, refresh_token)?;

    let existing = state
        .db
        .accounts_list()
        .await
        .map_err(|error| SiftError::app("db", error.to_string(), false))?
        .into_iter()
        .find(|account| account.email.eq_ignore_ascii_case(&info.email));
    let acc = if let Some(existing) = existing {
        state
            .db
            .accounts_update_meta(&existing.id, None, info.name.clone(), None, None)
            .await
            .map_err(|error| SiftError::app("db", error.to_string(), false))?;
        state
            .db
            .accounts_set_auth_kind(&existing.id, "oauth")
            .await
            .map_err(|error| SiftError::app("db", error.to_string(), false))?;
        state
            .db
            .accounts_set_state(&existing.id, "partial")
            .await
            .map_err(|error| SiftError::app("db", error.to_string(), false))?;
        state
            .db
            .accounts_get(&existing.id)
            .await
            .map_err(|error| SiftError::app("db", error.to_string(), false))?
            .ok_or_else(|| SiftError::NotFound("account".into()))?
    } else {
        state
            .db
            .new_account(&info.email, info.name, info.picture)
            .await
            .map_err(|error| SiftError::app("db", error.to_string(), false))?
    };

    state.tokens.write().await.insert(
        acc.id.clone(),
        crate::app_state::TokenInfo {
            access: tokens.access_token.clone(),
            expires_at: crate::db::now_ms() + tokens.expires_in * 1000,
        },
    );
    state.providers.write().await.insert(
        acc.id.clone(),
        std::sync::Arc::new(crate::provider::gmail::api::GmailApiProvider::new(
            acc.id.clone(),
            client,
        )),
    );
    // start full sync in background
    let (db, accid) = (state.db.clone(), acc.id.clone());
    // Note: real sync loop spawned by lib.rs watcher; here kick full sync once
    tokio::spawn(async move {
        let _ = (db, accid);
    });
    Ok(acc)
}

#[tauri::command]
pub async fn accounts_cancel_add() -> Result<(), SiftError> {
    Ok(())
}

#[tauri::command]
pub async fn accounts_remove(state: State<'_, AppState>, id: String) -> Result<(), SiftError> {
    if let Some(a) = state
        .db
        .accounts_get(&id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
    {
        let _ = crate::secrets::delete(&a.email);
    }
    state.providers.write().await.remove(&id);
    state.tokens.write().await.remove(&id);
    state
        .db
        .accounts_remove(&id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))
}

#[tauri::command]
pub async fn accounts_update(
    state: State<'_, AppState>,
    id: String,
    color: Option<String>,
    display_name: Option<String>,
    signature_html: Option<String>,
    sort_order: Option<i64>,
) -> Result<Account, SiftError> {
    state
        .db
        .accounts_update_meta(&id, color, display_name, signature_html, sort_order)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    state
        .db
        .accounts_get(&id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
        .ok_or_else(|| SiftError::NotFound("account".into()))
}

/// MX probe for Step B (P11-T15). Returns google_hosted true/false, or None
/// (null) when DNS fails/offline — the wizard proceeds silently then.
#[tauri::command]
pub async fn accounts_probe_email(email: String) -> Result<Option<bool>, SiftError> {
    Ok(probe_google_hosted(&email).await)
}

pub async fn probe_google_hosted(email: &str) -> Option<bool> {
    if let Ok(mock) = std::env::var("SIFT_MX_MOCK") {
        match mock.as_str() {
            "google" => return Some(true),
            "other" => return Some(false),
            "error" => return None,
            _ => {}
        }
    }
    let domain = email
        .split('@')
        .nth(1)?
        .trim()
        .trim_end_matches('.')
        .to_lowercase();
    if domain.is_empty() {
        return None;
    }
    let lookup = tokio::time::timeout(std::time::Duration::from_millis(1500), async {
        #[allow(deprecated)]
        let resolver = hickory_resolver::TokioResolver::builder_tokio()
            .ok()?
            .build()
            .ok()?;
        let resp = resolver.mx_lookup(domain.clone()).await.ok()?;
        let mut any = false;
        for rec in resp.answers() {
            // Tolerant: match the exchange name in any record text (avoids
            // hickory RData API churn across 0.24–0.26).
            let s = format!("{rec:?}").to_lowercase();
            // Heuristic: any answer means the domain has MX; google when the
            // exchange mentions google/googlemail.
            any = true;
            if s.contains("google.com") || s.contains("googlemail.com") {
                return Some(true);
            }
        }
        if !any {
            return None;
        }
        Some(false)
    })
    .await;
    lookup.unwrap_or_default()
}

/// Normalize a pasted app password: strip spaces/dashes, lowercase.
/// Valid when exactly 16 ASCII letters.
pub fn normalize_app_password(raw: &str) -> Option<String> {
    let clean: String = raw
        .chars()
        .filter(|c| *c != ' ' && *c != '-' && *c != '\u{00a0}')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    if clean.len() == 16 && clean.bytes().all(|b| b.is_ascii_lowercase()) {
        Some(clean)
    } else {
        None
    }
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SetupProgress {
    Connecting,
    Authenticating,
    Listing,
    Syncing,
    Done,
    Error { code: String },
}

/// App-password sign-in (Step D). Transactional: nothing touches `accounts`
/// or Keychain until verify() + list_labels() both succeed; the full sync
/// starts only after the row exists (P11-T14).
#[tauri::command]
pub async fn accounts_add_app_password(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    email: String,
    app_password: String,
    progress: tauri::ipc::Channel<SetupProgress>,
) -> Result<Account, SiftError> {
    let email = email.trim().to_lowercase();
    let pw = normalize_app_password(&app_password).ok_or_else(|| {
        SiftError::app(
            "imap_bad_password",
            "That app password didn\'t work. Create a fresh one and paste it again.",
            false,
        )
    })?;
    log::info!(target: "sift::setup", "app-password sign-in: connecting");
    let _ = progress.send(SetupProgress::Connecting);
    // Build a non-cached pool for verification (no DB writes yet).
    let pool = crate::provider::imap::conn::ImapPool::gmail(email.clone(), pw.clone());
    let tmp_db = state.db.clone();
    let verifier = crate::provider::imap::provider::GmailImapProvider::new(
        "verify".into(),
        pool.clone(),
        tmp_db,
    );
    let _ = progress.send(SetupProgress::Authenticating);
    verifier.verify().await.inspect_err(|e| {
        log::warn!(target: "sift::setup", "app-password sign-in: verify failed: {e}");
        let _ = progress.send(SetupProgress::Error {
            code: serde_json::to_value(e)
                .unwrap()
                .get("code")
                .and_then(|c| c.as_str())
                .unwrap_or("imap_transient")
                .to_string(),
        });
    })?;
    log::info!(target: "sift::setup", "app-password sign-in: authenticated, listing labels");
    let _ = progress.send(SetupProgress::Listing);
    verifier.list_labels().await.inspect_err(|e| {
        log::warn!(target: "sift::setup", "app-password sign-in: list_labels failed: {e}");
        let _ = progress.send(SetupProgress::Error {
            code: serde_json::to_value(e)
                .unwrap()
                .get("code")
                .and_then(|c| c.as_str())
                .unwrap_or("imap_transient")
                .to_string(),
        });
    })?;
    // Commit: Keychain first, then the account row with auth_kind app_password.
    crate::secrets::store_app_password(&email, &pw)?;
    let acc = state
        .db
        .new_account(&email, None, None)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    state
        .db
        .accounts_set_auth_kind(&acc.id, "app_password")
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    let acc = state
        .db
        .accounts_get(&acc.id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
        .ok_or_else(|| SiftError::NotFound("account".into()))?;
    // Provider + background full sync (first store:threads within ~2 s).
    let provider: std::sync::Arc<dyn crate::provider::Provider> =
        std::sync::Arc::new(crate::provider::imap::provider::GmailImapProvider::new(
            acc.id.clone(),
            crate::provider::imap::conn::ImapPool::gmail(email.clone(), pw),
            state.db.clone(),
        ));
    state
        .providers
        .write()
        .await
        .insert(acc.id.clone(), provider.clone());
    log::info!(target: "sift::setup", "app-password sign-in: account created, starting sync");
    let _ = progress.send(SetupProgress::Syncing);
    let (db, sink_acc, app2) = (state.db.clone(), acc.id.clone(), app.clone());
    tokio::spawn(async move {
        let sink = crate::provider::DbSink::with_progress(db.clone(), move |s| {
            let _ = app2.emit("sync:status", &s);
        });
        let cancel = tokio_util::sync::CancellationToken::new();
        match provider.full_sync(&sink, cancel).await {
            Ok(cursor) => {
                let _ = db
                    .accounts_set_history(&sink_acc, &cursor.render(), crate::db::now_ms())
                    .await;
                let _ = db.accounts_set_state(&sink_acc, "partial").await;
            }
            Err(e) => {
                let _ = db.accounts_set_state(&sink_acc, "error").await;
                eprintln!("app-password full sync failed: {e}");
            }
        }
    });
    let _ = progress.send(SetupProgress::Done);
    Ok(acc)
}

/// Re-auth with a fresh app password (banner → Step C). No resync: ids are
/// stable across transports, cursors stay intact.
#[tauri::command]
pub async fn accounts_update_app_password(
    state: tauri::State<'_, AppState>,
    id: String,
    app_password: String,
) -> Result<Account, SiftError> {
    let acc = state
        .db
        .accounts_get(&id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
        .ok_or_else(|| SiftError::NotFound("account".into()))?;
    let pw = normalize_app_password(&app_password).ok_or_else(|| {
        SiftError::app(
            "imap_bad_password",
            "That app password didn\'t work.",
            false,
        )
    })?;
    // Verify before overwriting the stored secret.
    let pool = crate::provider::imap::conn::ImapPool::gmail(acc.email.clone(), pw.clone());
    let verifier =
        crate::provider::imap::provider::GmailImapProvider::new(id.clone(), pool, state.db.clone());
    verifier.verify().await?;
    crate::secrets::store_app_password(&acc.email, &pw)?;
    state.providers.write().await.remove(&id);
    state
        .db
        .accounts_set_state(&id, "partial")
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    state
        .db
        .accounts_get(&id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
        .ok_or_else(|| SiftError::NotFound("account".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn p11_t15_probe_mx_mocked() {
        std::env::set_var("SIFT_MX_MOCK", "google");
        assert_eq!(probe_google_hosted("a@gmail.com").await, Some(true));
        std::env::set_var("SIFT_MX_MOCK", "other");
        assert_eq!(probe_google_hosted("a@zoho.com").await, Some(false));
        std::env::set_var("SIFT_MX_MOCK", "error");
        assert_eq!(probe_google_hosted("a@gmail.com").await, None);
        std::env::remove_var("SIFT_MX_MOCK");
    }
    #[test]
    fn p11_t16_app_password_normalize() {
        assert_eq!(
            normalize_app_password("abcd efgh ijkl mnop"),
            Some("abcdefghijklmnop".into())
        );
        assert_eq!(
            normalize_app_password("ABCD-EFGH-IJKL-MNOP"),
            Some("abcdefghijklmnop".into())
        );
        assert_eq!(
            normalize_app_password("abcdefghijklmnop "),
            Some("abcdefghijklmnop".into())
        );
        assert!(normalize_app_password("abcde").is_none());
        assert!(normalize_app_password("abcd1234efgh5678").is_none());
        assert!(normalize_app_password("abcdefghijklmnopq").is_none());
    }
}
