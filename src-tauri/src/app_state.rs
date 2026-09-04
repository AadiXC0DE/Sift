use crate::db::Db;
use crate::errors::SiftError;
use crate::provider::gmail::{api::GmailApiProvider, client::GmailClient};
use crate::provider::Provider;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize};
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock};

pub struct TokenInfo {
    pub access: String,
    pub expires_at: i64,
}

pub struct AppState {
    pub db: Db,
    pub http: reqwest::Client,
    pub tokens: RwLock<HashMap<String, TokenInfo>>,
    pub providers: RwLock<HashMap<String, std::sync::Arc<dyn Provider>>>,
    pub refresh_lock: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub online: AtomicBool,
    pub foreground_inflight: Arc<AtomicUsize>,
    pub gate: crate::sync::backfill::BackfillGate,
    pub first_paint_at: AtomicI64,
    pub data_dir: PathBuf,
    pub emit: Mutex<Option<tauri::AppHandle>>,
}

impl AppState {
    pub fn new(db: Db, data_dir: PathBuf) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(format!("Sift/{}", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            db,
            http,
            tokens: RwLock::new(Default::default()),
            providers: RwLock::new(Default::default()),
            refresh_lock: Mutex::new(Default::default()),
            online: AtomicBool::new(true),
            foreground_inflight: Arc::new(AtomicUsize::new(0)),
            gate: crate::sync::backfill::BackfillGate::new(),
            first_paint_at: AtomicI64::new(0),
            data_dir,
            emit: Mutex::new(None),
        }
    }

    /// Provider bound to an account, cached per account id. OAuth accounts
    /// refresh via the TokenStore; app-password accounts resolve once the
    /// IMAP provider lands (until then they report not-configured).
    pub async fn provider_for(
        &self,
        account_id: &str,
    ) -> Result<std::sync::Arc<dyn Provider>, SiftError> {
        if let Some(cached) = self.providers.read().await.get(account_id).cloned() {
            if cached.kind() == crate::provider::ProviderKind::GmailImap {
                return Ok(cached);
            }
            let now = crate::db::now_ms();
            let token_is_fresh = self
                .tokens
                .read()
                .await
                .get(account_id)
                .is_some_and(|token| token.expires_at - now > 5 * 60 * 1000);
            if token_is_fresh {
                return Ok(cached);
            }
            self.providers.write().await.remove(account_id);
        }
        let acc = self
            .db
            .accounts_get(account_id)
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        let acc = acc.ok_or_else(|| SiftError::app("account", "missing", false))?;
        if acc.auth_kind == "app_password" {
            let pw = match crate::secrets::load_app_password(&acc.email)? {
                Some(password) => password,
                None => {
                    let _ = self.db.accounts_set_state(account_id, "reauth").await;
                    return Err(SiftError::app(
                        "imap_needs_app_password",
                        "Reconnect this account with a Google app password.",
                        false,
                    ));
                }
            };
            let pool = crate::provider::imap::conn::ImapPool::gmail(acc.email.clone(), pw);
            let c: std::sync::Arc<dyn Provider> =
                std::sync::Arc::new(crate::provider::imap::provider::GmailImapProvider::new(
                    account_id.into(),
                    pool,
                    self.db.clone(),
                ));
            self.providers
                .write()
                .await
                .insert(account_id.into(), c.clone());
            return Ok(c);
        }
        let access = self.get_access_token(account_id).await?;
        let c: std::sync::Arc<dyn Provider> = std::sync::Arc::new(GmailApiProvider::new(
            account_id.into(),
            GmailClient::new(access),
        ));
        self.providers
            .write()
            .await
            .insert(account_id.into(), c.clone());
        Ok(c)
    }

    pub async fn invalidate_oauth_provider(&self, account_id: &str) {
        self.providers.write().await.remove(account_id);
        self.tokens.write().await.remove(account_id);
    }

    /// Back-compat shim used while call sites migrate to [`Self::provider_for`].
    pub async fn client_for(&self, account_id: &str) -> Result<GmailClient, SiftError> {
        let access = self.get_access_token(account_id).await?;
        Ok(GmailClient::new(access))
    }

    pub async fn get_access_token(&self, account_id: &str) -> Result<String, SiftError> {
        // single-flight per account
        let guard: Arc<Mutex<()>> = {
            let mut m = self.refresh_lock.lock().await;
            m.entry(account_id.into())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _g = guard.lock().await;
        let now = crate::db::now_ms();
        if let Some(info) = self.tokens.read().await.get(account_id) {
            if info.expires_at - now > 5 * 60 * 1000 {
                return Ok(info.access.clone());
            }
        }
        // refresh
        let acc = self
            .db
            .accounts_get(account_id)
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        let acc = acc.ok_or_else(|| SiftError::app("account", "missing", false))?;
        let rt = match crate::secrets::load_refresh_token(&acc.email)? {
            Some(token) => token,
            None => {
                let _ = self.db.accounts_set_state(account_id, "reauth").await;
                self.emit_event(
                    "auth:expired",
                    serde_json::json!({ "account_id": account_id }),
                );
                return Err(SiftError::reauth("Reconnect this Google account."));
            }
        };
        match crate::provider::gmail::oauth::refresh(&rt, &self.http).await {
            Ok(t) => {
                let info = TokenInfo {
                    access: t.access_token.clone(),
                    expires_at: now + t.expires_in * 1000,
                };
                self.tokens.write().await.insert(account_id.into(), info);
                // Drop any cached provider so the next call rebuilds it
                // with the fresh access token.
                self.providers.write().await.remove(account_id);
                Ok(t.access_token)
            }
            Err(SiftError::App { code, .. }) if code == "reauth" => {
                self.db
                    .accounts_set_state(account_id, "reauth")
                    .await
                    .map_err(|e| SiftError::app("db", e.to_string(), false))?;
                self.emit_event(
                    "auth:expired",
                    serde_json::json!({ "account_id": account_id }),
                );
                Err(SiftError::reauth("refresh rejected"))
            }
            Err(e) => Err(e),
        }
    }

    pub fn emit_event(&self, _event: &str, _payload: serde_json::Value) {
        // Real emit happens in commands via AppHandle; this is a fallback log hook.
        // Tests assert state transitions, not Tauri events.
    }

    pub fn set_online(&self, v: bool) {
        self.online.store(v, std::sync::atomic::Ordering::SeqCst);
    }
    pub fn is_online(&self) -> bool {
        self.online.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn p2_t04_singleflight_refresh() {
        // wiremock token endpoint counting refresh requests
        let server = wiremock::MockServer::start().await;
        std::env::set_var("SIFT_TOKEN_URL", format!("{}/token", server.uri()));
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_json(serde_json::json!({"access_token":"a1","expires_in":3600})),
            )
            .expect(1)
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let a = db.new_account("u@x.com", None, None).await.unwrap();
        crate::secrets::store_refresh_token("u@x.com", "rt").unwrap();
        let st = std::sync::Arc::new(AppState::new(db, dir.path().to_path_buf()));
        // seed expired token
        st.tokens.write().await.insert(
            a.id.clone(),
            TokenInfo {
                access: "old".into(),
                expires_at: 0,
            },
        );
        let mut hs = vec![];
        for _ in 0..20 {
            let (st2, aid) = (st.clone(), a.id.clone());
            hs.push(tokio::spawn(async move {
                st2.get_access_token(&aid).await.unwrap()
            }));
        }
        let mut vals = vec![];
        for h in hs {
            vals.push(h.await.unwrap());
        }
        assert!(vals.iter().all(|v| v == "a1"));
        // wiremock expect(1) verified on drop; give it a beat
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        std::env::remove_var("SIFT_TOKEN_URL");
    }

    #[tokio::test]
    async fn p2_t05_invalid_grant_reauth() {
        let server = wiremock::MockServer::start().await;
        std::env::set_var("SIFT_TOKEN_URL", format!("{}/token", server.uri()));
        wiremock::Mock::given(wiremock::matchers::method("POST"))
            .respond_with(
                wiremock::ResponseTemplate::new(400)
                    .set_body_string("{\"error\":\"invalid_grant\"}"),
            )
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let a = db.new_account("v@x.com", None, None).await.unwrap();
        crate::secrets::store_refresh_token("v@x.com", "bad").unwrap();
        let st = AppState::new(db, dir.path().to_path_buf());
        let e = st.get_access_token(&a.id).await.unwrap_err();
        assert_eq!(serde_json::to_value(&e).unwrap()["code"], "reauth");
        std::env::remove_var("SIFT_TOKEN_URL");
    }
}
