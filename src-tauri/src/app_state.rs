use crate::db::Db;
use crate::errors::SiftError;
use crate::provider::gmail::{api::GmailApiProvider, client::GmailClient};
use crate::provider::Provider;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicI64, AtomicUsize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

/// How long removal/reauth waits for a cancelled generation's tasks to finish
/// before giving up on them (they re-check the generation before any write, so
/// a straggler can no longer touch the account either way).
const JOIN_TIMEOUT: Duration = Duration::from_secs(5);

pub struct TokenInfo {
    pub access: String,
    pub expires_at: i64,
}

/// Background work owned by one account at one point in time. A new generation
/// (reauth, re-add) cancels and replaces the previous one, so a removed or
/// reconnected account can never keep writing under an old credential.
pub struct AccountRuntime {
    pub cancel: CancellationToken,
    pub joins: Vec<JoinHandle<()>>,
    pub generation: u64,
}

/// The setup wizard's in-flight sign-in. A new setup supersedes the previous
/// one; a cancelled setup can never insert an account.
pub struct SetupRuntime {
    pub generation: u64,
    pub cancel: CancellationToken,
}

/// Cancel a generation and await its tasks, bounded so a stuck task cannot
/// hold up removal or reauthentication.
async fn stop_generation(runtime: AccountRuntime) {
    runtime.cancel.cancel();
    let _ = tokio::time::timeout(JOIN_TIMEOUT, async {
        for join in runtime.joins {
            let _ = join.await;
        }
    })
    .await;
}

pub struct AppState {
    pub db: Db,
    pub http: reqwest::Client,
    pub tokens: RwLock<HashMap<String, TokenInfo>>,
    pub providers: RwLock<HashMap<String, std::sync::Arc<dyn Provider>>>,
    pub refresh_lock: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    pub provider_lock: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// In-memory copy of app passwords. Avoids re-reading the Keychain (and
    /// re-prompting) when a provider is rebuilt during a session.
    pub app_passwords: RwLock<HashMap<String, String>>,
    pub foreground_inflight: Arc<AtomicUsize>,
    pub gate: crate::sync::backfill::BackfillGate,
    pub first_paint_at: AtomicI64,
    pub data_dir: PathBuf,
    pub emit: Mutex<Option<tauri::AppHandle>>,
    /// Per-account background generations (P4.4).
    pub account_runtimes: Mutex<HashMap<String, AccountRuntime>>,
    /// Per-account sync coordinator (P4.3): one tick at a time, one
    /// coalesced follow-up, cancellable with the account's generation.
    pub sync_coordinators: Mutex<HashMap<String, Arc<crate::sync::coordinator::SyncCoordinator>>>,
    /// The wizard's current sign-in, if any.
    pub setup_runtime: Mutex<Option<SetupRuntime>>,
    /// Native connectivity state per account (P4.6). The network hint is a
    /// hint; successful/failed provider operations are the evidence.
    pub connectivity: crate::connectivity::Connectivity,
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
            provider_lock: Mutex::new(Default::default()),
            app_passwords: RwLock::new(Default::default()),
            foreground_inflight: Arc::new(AtomicUsize::new(0)),
            gate: crate::sync::backfill::BackfillGate::new(),
            first_paint_at: AtomicI64::new(0),
            data_dir,
            emit: Mutex::new(None),
            account_runtimes: Mutex::new(Default::default()),
            sync_coordinators: Mutex::new(Default::default()),
            setup_runtime: Mutex::new(None),
            connectivity: crate::connectivity::Connectivity::new(),
        }
    }

    // -- per-account background generations (P4.4) --------------------------

    /// Start a new generation for an account. The previous generation is
    /// cancelled and its tasks awaited (bounded) before this returns, so the
    /// caller can safely swap credentials afterwards.
    pub async fn begin_generation(&self, account_id: &str) -> (u64, CancellationToken) {
        let cancel = CancellationToken::new();
        // A new generation invalidates the old tick coordinator: its token
        // must not stay bound to the previous credentials (P4.3/P4.4).
        self.retire_coordinator(account_id).await;
        let (generation, previous) = {
            let mut runtimes = self.account_runtimes.lock().await;
            let generation = runtimes.get(account_id).map_or(1, |rt| rt.generation + 1);
            let previous = runtimes.insert(
                account_id.to_string(),
                AccountRuntime {
                    cancel: cancel.clone(),
                    joins: Vec::new(),
                    generation,
                },
            );
            (generation, previous)
        };
        if let Some(previous) = previous {
            stop_generation(previous).await;
        }
        (generation, cancel)
    }

    /// Current generation for an account, if one was ever started.
    pub async fn generation(&self, account_id: &str) -> Option<u64> {
        self.account_runtimes
            .lock()
            .await
            .get(account_id)
            .map(|rt| rt.generation)
    }

    /// Current `(generation, token)` in one atomic read, so a caller cannot
    /// pair a token with a generation that was replaced in between.
    pub async fn runtime_state(&self, account_id: &str) -> Option<(u64, CancellationToken)> {
        self.account_runtimes
            .lock()
            .await
            .get(account_id)
            .map(|rt| (rt.generation, rt.cancel.clone()))
    }

    /// True while `generation` is the account's live generation: the account
    /// was not removed and no reauth/re-add replaced it in the meantime.
    /// Every write that follows network work is gated on this.
    pub async fn is_current(&self, account_id: &str, generation: u64) -> bool {
        self.account_runtimes
            .lock()
            .await
            .get(account_id)
            .is_some_and(|rt| rt.generation == generation && !rt.cancel.is_cancelled())
    }

    /// True once [`Self::cancel_account`] ran and nothing restarted the
    /// account's work.
    pub async fn is_cancelled(&self, account_id: &str) -> bool {
        self.account_runtimes
            .lock()
            .await
            .get(account_id)
            .is_some_and(|rt| rt.cancel.is_cancelled())
    }

    /// Attach a task to the account's current generation. A handle whose
    /// generation was already replaced (or cancelled) is aborted instead: its
    /// work belongs to a generation nobody is waiting for any more.
    pub async fn register_task(&self, account_id: &str, generation: u64, handle: JoinHandle<()>) {
        let mut runtimes = self.account_runtimes.lock().await;
        match runtimes.get_mut(account_id) {
            Some(rt) if rt.generation == generation && !rt.cancel.is_cancelled() => {
                rt.joins.push(handle);
            }
            _ => handle.abort(),
        }
    }

    /// Stop an account's work and make the account safe to delete: no new
    /// provider or token is handed out, the token is cancelled and its tasks
    /// awaited (bounded), then every cached provider, credential and lock for
    /// the account is dropped. Secrets and rows are the caller's next step.
    pub async fn cancel_account(&self, account_id: &str) {
        self.retire_coordinator(account_id).await;
        self.connectivity.forget(account_id);
        let email = self
            .db
            .accounts_get(account_id)
            .await
            .ok()
            .flatten()
            .map(|account| account.email);
        let previous = {
            let mut runtimes = self.account_runtimes.lock().await;
            runtimes.remove(account_id).unwrap_or(AccountRuntime {
                cancel: CancellationToken::new(),
                joins: Vec::new(),
                generation: 0,
            })
        };
        let cancel = previous.cancel.clone();
        cancel.cancel();
        // Tombstone before unwinding: while the joins below finish, the
        // account must already refuse new providers and tokens.
        self.account_runtimes.lock().await.insert(
            account_id.to_string(),
            AccountRuntime {
                cancel,
                joins: Vec::new(),
                generation: previous.generation.max(1),
            },
        );
        stop_generation(previous).await;
        self.providers.write().await.remove(account_id);
        self.tokens.write().await.remove(account_id);
        self.refresh_lock.lock().await.remove(account_id);
        self.provider_lock.lock().await.remove(account_id);
        if let Some(email) = email {
            self.forget_app_password(&email).await;
        }
    }

    /// Refuse work for an account whose generation was cancelled (removed, or
    /// mid-reauthentication) instead of rebuilding a provider for it.
    async fn refuse_if_cancelled(&self, account_id: &str) -> Result<(), SiftError> {
        if self.is_cancelled(account_id).await {
            return Err(SiftError::app(
                "account_cancelled",
                "This account's session was cancelled.",
                false,
            ));
        }
        Ok(())
    }

    // -- setup wizard (P4.4) -------------------------------------------------

    /// Start a setup sign-in. A setup still running is cancelled: only the
    /// newest wizard attempt may create an account.
    pub async fn begin_setup(&self) -> (u64, CancellationToken) {
        let cancel = CancellationToken::new();
        let (generation, previous) = {
            let mut setup = self.setup_runtime.lock().await;
            let generation = setup.as_ref().map_or(1, |s| s.generation + 1);
            let previous = setup.replace(SetupRuntime {
                generation,
                cancel: cancel.clone(),
            });
            (generation, previous)
        };
        if let Some(previous) = previous {
            previous.cancel.cancel();
        }
        (generation, cancel)
    }

    /// Cancel the wizard's in-flight sign-in. Harmless when none is running.
    pub async fn cancel_setup(&self) {
        if let Some(setup) = self.setup_runtime.lock().await.as_ref() {
            setup.cancel.cancel();
        }
    }

    /// Token of the current setup attempt, if one was started.
    pub async fn setup_token(&self) -> Option<CancellationToken> {
        self.setup_runtime
            .lock()
            .await
            .as_ref()
            .map(|setup| setup.cancel.clone())
    }

    /// True while `generation` is the live, uncancelled setup. Checked right
    /// before an account row is committed, so a late sign-in success cannot
    /// insert an account after the user cancelled the wizard.
    pub async fn setup_is_current(&self, generation: u64) -> bool {
        self.setup_runtime
            .lock()
            .await
            .as_ref()
            .is_some_and(|setup| setup.generation == generation && !setup.cancel.is_cancelled())
    }

    /// Provider bound to an account, cached per account id. OAuth accounts
    /// refresh via the TokenStore; app-password accounts resolve once the
    /// IMAP provider lands (until then they report not-configured).
    ///
    /// Single-flight: concurrent callers (poll loop, IDLE, foreground fetch)
    /// share one provider construction, so the Keychain is read exactly once
    /// per account instead of prompting several times at startup.
    pub async fn provider_for(
        &self,
        account_id: &str,
    ) -> Result<std::sync::Arc<dyn Provider>, SiftError> {
        self.refuse_if_cancelled(account_id).await?;
        if let Some(cached) = self.cached_provider(account_id).await {
            return Ok(cached);
        }
        let guard: Arc<Mutex<()>> = {
            let mut m = self.provider_lock.lock().await;
            m.entry(account_id.to_string())
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone()
        };
        let _g = guard.lock().await;
        // Another task may have built it while we waited.
        if let Some(cached) = self.cached_provider(account_id).await {
            return Ok(cached);
        }
        let acc = self
            .db
            .accounts_get(account_id)
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        let acc = acc.ok_or_else(|| SiftError::app("account", "missing", false))?;
        if acc.auth_kind == "app_password" {
            let pw = match self.load_app_password_cached(&acc.email).await? {
                Some(password) => password,
                None => {
                    let _ = self.db.accounts_set_state(account_id, "reauth").await;
                    let error = SiftError::app(
                        "imap_needs_app_password",
                        "Reconnect this account with a Google app password.",
                        false,
                    );
                    if self.record_provider_error(account_id, &error) {
                        self.emit_connectivity(account_id);
                    }
                    return Err(error);
                }
            };
            let pool = crate::provider::imap::conn::ImapPool::gmail(acc.email.clone(), pw);
            // Reading the password may have blocked on the Keychain; the
            // account could be gone by now.
            self.refuse_if_cancelled(account_id).await?;
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
        // Refreshing the token is network work: never hand out a provider for
        // an account that was removed or reconnected in the meantime.
        self.refuse_if_cancelled(account_id).await?;
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

    /// A cached provider that is still valid, or `None` when it must be built.
    async fn cached_provider(&self, account_id: &str) -> Option<std::sync::Arc<dyn Provider>> {
        if let Some(cached) = self.providers.read().await.get(account_id).cloned() {
            if cached.kind() == crate::provider::ProviderKind::GmailImap {
                return Some(cached);
            }
            let now = crate::db::now_ms();
            let token_is_fresh = self
                .tokens
                .read()
                .await
                .get(account_id)
                .is_some_and(|token| token.expires_at - now > 5 * 60 * 1000);
            if token_is_fresh {
                return Some(cached);
            }
            self.providers.write().await.remove(account_id);
        }
        None
    }

    /// Read the app password once per session; later reads come from memory so
    /// macOS does not prompt again. The copy lives only in RAM and is dropped
    /// when the account is removed or the app exits.
    pub async fn load_app_password_cached(&self, email: &str) -> Result<Option<String>, SiftError> {
        if let Some(pw) = self.app_passwords.read().await.get(email).cloned() {
            return Ok(Some(pw));
        }
        match crate::secrets::load_app_password(email)? {
            Some(pw) => {
                self.app_passwords
                    .write()
                    .await
                    .insert(email.to_string(), pw.clone());
                Ok(Some(pw))
            }
            None => Ok(None),
        }
    }

    pub async fn remember_app_password(&self, email: &str, pw: &str) {
        self.app_passwords
            .write()
            .await
            .insert(email.to_string(), pw.to_string());
    }

    pub async fn forget_app_password(&self, email: &str) {
        self.app_passwords.write().await.remove(email);
    }

    /// Drop every cached provider for the account so the next call rebuilds it
    /// from the fresh token (P4.6).
    ///
    /// One provider instance serves body reads, attachment fetches, server
    /// search and the outbox drain, and all of them resolve it through
    /// `provider_for`, so removing the entry invalidates every lane at once -
    /// there is no second cache that could keep serving a stale token.
    pub async fn invalidate_oauth_provider(&self, account_id: &str) {
        self.providers.write().await.remove(account_id);
        self.tokens.write().await.remove(account_id);
    }

    /// Refresh the account's token if needed and rebuild its provider, as one
    /// single-flight operation (P4.6). Concurrent callers share one refresh;
    /// a token that goes stale mid-flight is invalidated for every lane.
    pub async fn refresh_provider(
        &self,
        account_id: &str,
    ) -> Result<std::sync::Arc<dyn Provider>, SiftError> {
        self.invalidate_oauth_provider(account_id).await;
        self.provider_for(account_id).await
    }

    /// Back-compat shim used while call sites migrate to [`Self::provider_for`].
    pub async fn client_for(&self, account_id: &str) -> Result<GmailClient, SiftError> {
        let access = self.get_access_token(account_id).await?;
        Ok(GmailClient::new(access))
    }

    pub async fn get_access_token(&self, account_id: &str) -> Result<String, SiftError> {
        self.refuse_if_cancelled(account_id).await?;
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
                self.require_reauth(account_id);
                return Err(SiftError::reauth("Reconnect this Google account."));
            }
        };
        match crate::provider::gmail::oauth::refresh(&rt, &self.http).await {
            Ok(t) => {
                // The refresh is network work: if the account was removed (or
                // reconnected) while it was in flight, drop the result instead
                // of caching a token for a dead generation.
                self.refuse_if_cancelled(account_id).await?;
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
                self.require_reauth(account_id);
                Err(SiftError::reauth("refresh rejected"))
            }
            Err(e) => Err(e),
        }
    }

    pub fn emit_event(&self, event: &str, payload: serde_json::Value) {
        // Progress and store events go out through the runtime host; the
        // account-level events (auth:expired, connectivity:state) have no host
        // in the token path, so they use the handle registered at startup.
        if let Ok(guard) = self.emit.try_lock() {
            if let Some(app) = guard.as_ref() {
                use tauri::Emitter;
                let _ = app.emit(event, payload);
                return;
            }
        }
        log::debug!(target: "sift::events", "no emitter for {event}");
    }

    /// Register the window handle so account-level events reach the frontend.
    /// Without this an `auth:expired` had nowhere to go (P4.6).
    pub fn set_emitter(&self, app: tauri::AppHandle) {
        if let Ok(mut guard) = self.emit.try_lock() {
            *guard = Some(app);
        }
    }

    // -- connectivity (P4.6) -------------------------------------------------

    /// The host's reachability hint (frontend online/offline events).
    pub fn set_network_reachable(&self, reachable: bool) -> bool {
        self.connectivity.set_network_reachable(reachable)
    }

    pub fn network_reachable(&self) -> bool {
        self.connectivity.network_reachable()
    }

    /// Back-compat alias: `online` now means "the host network is reachable".
    pub fn set_online(&self, v: bool) {
        self.set_network_reachable(v);
    }

    pub fn is_online(&self) -> bool {
        self.network_reachable()
    }

    /// Background network work for this account should pause.
    pub fn network_paused(&self, account_id: &str) -> bool {
        self.connectivity.background_paused(account_id)
    }

    /// Record a successful provider operation for one account.
    pub fn record_provider_ok(&self, account_id: &str) -> bool {
        self.connectivity.record_ok(account_id, crate::db::now_ms())
    }

    /// Record a provider failure, and tell the UI the account's state changed.
    pub fn record_provider_error(&self, account_id: &str, error: &SiftError) -> bool {
        let changed = self
            .connectivity
            .record_failure(account_id, error, crate::db::now_ms());
        if changed {
            self.emit_connectivity(account_id);
        }
        changed
    }

    /// The account must be reconnected; sticky until an operation succeeds.
    pub fn require_reauth(&self, account_id: &str) -> bool {
        let changed = self
            .connectivity
            .require_reauth(account_id, crate::db::now_ms());
        if changed {
            self.emit_connectivity(account_id);
        }
        changed
    }

    /// Emit this account's connectivity row (state, last success, last error).
    pub fn emit_connectivity(&self, account_id: &str) {
        let row = self.connectivity.state_for(account_id, crate::db::now_ms());
        if let Ok(v) = serde_json::to_value(&row) {
            self.emit_event("connectivity:state", v);
        }
    }

    // -- sync coordination (P4.3) -------------------------------------------

    /// The account's sync coordinator, created on demand and tied to the
    /// account's live generation token so a removal/reauth cancels it too.
    pub async fn coordinator_for(
        &self,
        account_id: &str,
    ) -> Arc<crate::sync::coordinator::SyncCoordinator> {
        let mut coordinators = self.sync_coordinators.lock().await;
        if let Some(existing) = coordinators.get(account_id) {
            return existing.clone();
        }
        let token = {
            // Lock order: account_runtimes is taken and released before the
            // coordinator map is written (never the other way round).
            let runtimes = self.account_runtimes.lock().await;
            runtimes
                .get(account_id)
                .map(|rt| rt.cancel.child_token())
                .unwrap_or_default()
        };
        let coordinator = Arc::new(crate::sync::coordinator::SyncCoordinator::new(token));
        coordinators.insert(account_id.to_string(), coordinator.clone());
        coordinator
    }

    /// Drop an account's coordinator (removal, or a new generation) so its
    /// ticks stop and a fresh one binds to the new generation's token.
    async fn retire_coordinator(&self, account_id: &str) {
        let previous = self.sync_coordinators.lock().await.remove(account_id);
        if let Some(previous) = previous {
            previous.cancel_token().cancel();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    /// `SIFT_TOKEN_URL` is process-global: these tests must not run
    /// concurrently or one's mock server answers another's refresh.
    static TOKEN_ENV_LOCK: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    fn token_env_lock() -> &'static tokio::sync::Mutex<()> {
        TOKEN_ENV_LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }
    #[tokio::test]
    async fn p2_t04_singleflight_refresh() {
        let _env = token_env_lock().lock().await;
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
        let a = db
            .new_account("singleflight@x.com", None, None)
            .await
            .unwrap();
        crate::secrets::store_refresh_token("singleflight@x.com", "rt").unwrap();
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
        let _env = token_env_lock().lock().await;
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

    fn spawn_flag_after(flag: Arc<AtomicBool>, delay: Duration) -> JoinHandle<()> {
        tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        })
    }

    fn flag() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    fn is_set(flag: &AtomicBool) -> bool {
        flag.load(std::sync::atomic::Ordering::SeqCst)
    }

    async fn account_state(dir: &tempfile::TempDir, email: &str) -> (AppState, String) {
        let db = Db::open(dir.path()).unwrap();
        let account = db.new_account(email, None, None).await.unwrap();
        (
            AppState::new(db, dir.path().to_path_buf()),
            account.id,
        )
    }

    /// A new generation cancels the previous one and does not return until its
    /// tasks have stopped, so credentials can be swapped safely afterwards.
    #[tokio::test]
    async fn p44_t01_begin_generation_cancels_and_awaits_previous() {
        let dir = tempfile::tempdir().unwrap();
        let (state, id) = account_state(&dir, "gen@x.com").await;
        let (gen1, token1) = state.begin_generation(&id).await;
        assert_eq!(state.generation(&id).await, Some(gen1));

        let exited = flag();
        let handle = {
            let exited = exited.clone();
            let token = token1.clone();
            tokio::spawn(async move {
                tokio::select! {
                    _ = token.cancelled() => {}
                    _ = tokio::time::sleep(Duration::from_secs(60)) => {}
                }
                exited.store(true, std::sync::atomic::Ordering::SeqCst);
            })
        };
        state.register_task(&id, gen1, handle).await;

        let (gen2, token2) = state.begin_generation(&id).await;
        assert_eq!(gen2, gen1 + 1);
        assert!(token1.is_cancelled());
        assert!(!token2.is_cancelled());
        // The old task exited before begin_generation returned.
        assert!(is_set(&exited));
        assert!(state.is_current(&id, gen2).await);
        assert!(!state.is_current(&id, gen1).await);
    }

    /// A task that registers under a superseded generation is not tracked (and
    /// not waited on); one that registers for the live generation is.
    #[tokio::test]
    async fn p44_t02_register_task_ignores_stale_generation() {
        let dir = tempfile::tempdir().unwrap();
        let (state, id) = account_state(&dir, "stale@x.com").await;
        let (gen1, _t1) = state.begin_generation(&id).await;
        let (gen2, _t2) = state.begin_generation(&id).await;
        assert_ne!(gen1, gen2);

        let stale = flag();
        let live = flag();
        state
            .register_task(
                &id,
                gen1,
                spawn_flag_after(stale.clone(), Duration::from_millis(20)),
            )
            .await;
        state
            .register_task(
                &id,
                gen2,
                spawn_flag_after(live.clone(), Duration::from_millis(20)),
            )
            .await;

        state.cancel_account(&id).await;
        assert!(is_set(&live), "live generation task must be awaited");
        assert!(!is_set(&stale), "superseded task must be dropped");
    }

    /// Once cancelled, the account hands out neither a provider nor an access
    /// token, and its cached credentials are gone.
    #[tokio::test]
    async fn p44_t03_cancelled_account_refuses_provider_and_token() {
        let dir = tempfile::tempdir().unwrap();
        let (state, id) = account_state(&dir, "gone@x.com").await;
        state
            .db
            .accounts_set_auth_kind(&id, "app_password")
            .await
            .unwrap();
        state
            .remember_app_password("gone@x.com", "abcdefghijklmnop")
            .await;
        // Usable before removal: the password comes from the in-memory copy.
        assert!(state.provider_for(&id).await.is_ok());

        state.cancel_account(&id).await;

        let error = state.provider_for(&id).await.err().unwrap();
        assert_eq!(
            serde_json::to_value(&error).unwrap()["code"],
            "account_cancelled"
        );
        let error = state.get_access_token(&id).await.unwrap_err();
        assert_eq!(
            serde_json::to_value(&error).unwrap()["code"],
            "account_cancelled"
        );
        assert!(state.is_cancelled(&id).await);
        assert_eq!(
            state.load_app_password_cached("gone@x.com").await.unwrap(),
            None
        );
    }

    /// The wizard's setup token: cancelling is idempotent and harmless before
    /// any setup, and a new setup supersedes the old one.
    #[tokio::test]
    async fn p44_t04_setup_cancel_and_supersede() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _id) = account_state(&dir, "setup@x.com").await;
        // Harmless no-op before any setup started.
        state.cancel_setup().await;
        assert!(state.setup_token().await.is_none());

        let (gen1, token1) = state.begin_setup().await;
        assert!(state.setup_is_current(gen1).await);
        state.cancel_setup().await;
        assert!(token1.is_cancelled());
        assert!(!state.setup_is_current(gen1).await);
        assert!(state.setup_token().await.is_some());

        let (gen2, token2) = state.begin_setup().await;
        assert!(gen2 > gen1);
        assert!(!token2.is_cancelled());
        assert!(state.setup_is_current(gen2).await);
        assert!(!state.setup_is_current(gen1).await);

        state.begin_setup().await;
        assert!(token2.is_cancelled());
        assert!(!state.setup_is_current(gen2).await);
    }
}
