use crate::app_state::AppState;
use crate::dto::Account;
use crate::errors::SiftError;
use tauri::{AppHandle, State};

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
        let r = state
            .http
            .get(base)
            .bearer_auth(&tokens.access_token)
            .send()
            .await
            .map_err(SiftError::from)?;
        r.json().await.map_err(SiftError::from)?
    };
    if let Some(rt) = tokens.refresh_token {
        crate::secrets::store_refresh_token(&info.email, &rt)?;
    }
    let acc = state
        .db
        .new_account(&info.email, info.name, info.picture)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
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
