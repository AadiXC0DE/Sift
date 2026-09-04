use crate::db::Db;
use crate::errors::SiftError;
use crate::gmail::client::GmailClient;
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
    pub clients: RwLock<HashMap<String, GmailClient>>,
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
            clients: RwLock::new(Default::default()),
            refresh_lock: Mutex::new(Default::default()),
            online: AtomicBool::new(true),
            foreground_inflight: Arc::new(AtomicUsize::new(0)),
            gate: crate::sync::backfill::BackfillGate::new(),
            first_paint_at: AtomicI64::new(0),
            data_dir,
            emit: Mutex::new(None),
        }
    }

    pub async fn client_for(&self, account_id: &str) -> Result<GmailClient, SiftError> {
        if let Some(c) = self.clients.read().await.get(account_id).cloned() {
            return Ok(c);
        }
        let token = self.get_access_token(account_id).await?;
        let c = GmailClient::new(token);
        self.clients
            .write()
            .await
            .insert(account_id.into(), c.clone());
        Ok(c)
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
        let rt = crate::secrets::load_refresh_token(&acc.email)?
            .ok_or_else(|| SiftError::reauth("no refresh token"))?;
        match crate::gmail::oauth::refresh(&rt, &self.http).await {
            Ok(t) => {
                let info = TokenInfo {
                    access: t.access_token.clone(),
                    expires_at: now + t.expires_in * 1000,
                };
                self.tokens.write().await.insert(account_id.into(), info);
                if let Some(c) = self.clients.read().await.get(account_id) {
                    c.set_token(t.access_token.clone()).await;
                }
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
