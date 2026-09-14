use crate::app_state::{AppState, TokenInfo};
use crate::db::Db;
use crate::dto::Account;
use crate::errors::SiftError;
use crate::provider::Provider;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

#[tauri::command]
pub async fn accounts_list(state: State<'_, AppState>) -> Result<Vec<Account>, SiftError> {
    state
        .db
        .accounts_list()
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))
}

/// Normalized account identity: `  A@X.com ` and `a@x.com` are one account, so
/// a repeated sign-in can only ever reuse or create a single row (P4.4).
pub fn normalize_email(raw: &str) -> String {
    raw.trim().to_lowercase()
}

fn db_error(error: impl std::fmt::Display) -> SiftError {
    SiftError::app("db", error.to_string(), false)
}

fn setup_cancelled() -> SiftError {
    SiftError::app("setup_cancelled", "Sign-in was cancelled.", false)
}

/// Run one step of the wizard's network work under its token: a cancelled
/// setup drops the in-flight future instead of letting a late answer commit.
async fn cancellable<T>(
    cancel: &CancellationToken,
    work: impl Future<Output = Result<T, SiftError>>,
) -> Result<T, SiftError> {
    tokio::select! {
        _ = cancel.cancelled() => Err(setup_cancelled()),
        result = work => result,
    }
}

/// Refuse to touch secrets or rows for a setup that was cancelled or replaced
/// while verification was on the network.
async fn require_live_setup(
    state: &AppState,
    generation: u64,
    cancel: &CancellationToken,
) -> Result<(), SiftError> {
    if cancel.is_cancelled() || !state.setup_is_current(generation).await {
        return Err(setup_cancelled());
    }
    Ok(())
}

/// Insert or reuse the account row for a normalized email. Reuse keeps the
/// account id — and therefore every message, draft and cursor keyed by it —
/// when the same mailbox signs in again, so a second sign-in cannot create a
/// parallel account copy (P4.4).
async fn upsert_account_row(
    db: &Db,
    email: &str,
    name: Option<String>,
    avatar: Option<String>,
    auth_kind: &str,
) -> Result<Account, SiftError> {
    let email = normalize_email(email);
    let id = match db
        .accounts_find_by_email(&email)
        .await
        .map_err(db_error)?
    {
        Some(existing) => {
            db.accounts_update_meta(&existing.id, None, name, avatar, None)
                .await
                .map_err(db_error)?;
            db.accounts_set_auth_kind(&existing.id, auth_kind)
                .await
                .map_err(db_error)?;
            db.accounts_set_state(&existing.id, "partial")
                .await
                .map_err(db_error)?;
            existing.id
        }
        None => {
            let created = db.new_account(&email, name, avatar).await.map_err(db_error)?;
            db.accounts_set_auth_kind(&created.id, auth_kind)
                .await
                .map_err(db_error)?;
            created.id
        }
    };
    db.accounts_get(&id)
        .await
        .map_err(db_error)?
        .ok_or_else(|| SiftError::NotFound("account".into()))
}

/// The Keychain value a sign-in writes, and how to take it back if the row
/// write fails afterwards.
enum Credential<'a> {
    RefreshToken(&'a str),
    AppPassword(&'a str),
}

impl Credential<'_> {
    fn load(&self, email: &str) -> Result<Option<String>, SiftError> {
        match self {
            Self::RefreshToken(_) => crate::secrets::load_refresh_token(email),
            Self::AppPassword(_) => crate::secrets::load_app_password(email),
        }
    }
    fn store(&self, email: &str, value: &str) -> Result<(), SiftError> {
        match self {
            Self::RefreshToken(_) => crate::secrets::store_refresh_token(email, value),
            Self::AppPassword(_) => crate::secrets::store_app_password(email, value),
        }
    }
    fn value(&self) -> &str {
        match self {
            Self::RefreshToken(token) => token,
            Self::AppPassword(password) => password,
        }
    }
}

/// Commit a sign-in: refuse a cancelled setup, write the secret, then the row.
/// Nothing is left half-written — a failed row write restores the previous
/// secret value (or removes the new one), and a failed secret write writes no
/// row at all (P4.4).
#[allow(clippy::too_many_arguments)]
async fn commit_signin(
    state: &AppState,
    setup_generation: u64,
    setup_cancel: &CancellationToken,
    credential: Credential<'_>,
    email: &str,
    name: Option<String>,
    avatar: Option<String>,
    auth_kind: &str,
) -> Result<Account, SiftError> {
    let email = normalize_email(email);
    // Re-checked right before anything is written: the wizard may have been
    // cancelled while verification was still on the network.
    require_live_setup(state, setup_generation, setup_cancel).await?;
    let previous = credential.load(&email)?;
    credential.store(&email, credential.value())?;
    match upsert_account_row(&state.db, &email, name, avatar, auth_kind).await {
        Ok(account) => Ok(account),
        Err(error) => {
            let restored = match previous.as_deref() {
                Some(previous) => credential.store(&email, previous),
                None => crate::secrets::delete(&email),
            };
            if let Err(restore_error) = restored {
                log::warn!(
                    target: "sift::setup",
                    "sign-in failed and the credential could not be restored: {restore_error}"
                );
            }
            Err(error)
        }
    }
}

/// Install the freshly verified transport for an account: bump the generation
/// (cancelling the previous one and waiting for it), drop the stale provider
/// and token, requeue ops that were mid-flight, and publish the new provider.
/// Mailbox rows are never touched, so reauthentication keeps every message,
/// draft and cursor (P4.4).
async fn install_provider(
    state: &AppState,
    account_id: &str,
    provider: Arc<dyn Provider>,
    access_token: Option<TokenInfo>,
) -> Result<(u64, CancellationToken), SiftError> {
    let runtime = state.begin_generation(account_id).await;
    state.providers.write().await.remove(account_id);
    state.tokens.write().await.remove(account_id);
    requeue_inflight(&state.db, account_id).await;
    state
        .providers
        .write()
        .await
        .insert(account_id.to_string(), provider);
    if let Some(token) = access_token {
        state.tokens.write().await.insert(account_id.to_string(), token);
    }
    state
        .db
        .accounts_set_state(account_id, "partial")
        .await
        .map_err(db_error)?;
    Ok(runtime)
}

/// An op that was mid-flight when its generation was cancelled must not be
/// lost — but it must not be replayed blindly either (P6.1).
///
/// Idempotent work (label sets, identity-addressed deletion, a draft push)
/// goes back to the queue. An `inflight` **send** does not: its acceptance is
/// unknown, so it becomes `uncertain` and waits for reconciliation against the
/// provider's Sent view. This is the same rule startup recovery applies, and
/// it is the one that stops a reconnect from sending a message twice.
async fn requeue_inflight(db: &Db, account_id: &str) {
    let account_id = account_id.to_string();
    let now = crate::db::now_ms();
    let _ = db
        .write(move |c| -> anyhow::Result<()> {
            c.execute(
                "UPDATE outbox_ops SET state='pending', not_before=0, started_at=NULL \
                 WHERE account_id=? AND state='inflight' AND kind<>'send'",
                rusqlite::params![account_id],
            )?;
            c.execute(
                "UPDATE outbox_ops SET state='uncertain', reconcile_at=?1, reconcile_attempts=0 \
                 WHERE account_id=?2 AND state='inflight' AND kind='send'",
                rusqlite::params![now, account_id],
            )?;
            Ok(())
        })
        .await;
}

#[tauri::command]
pub async fn accounts_add_google(
    app: AppHandle,
    state: State<'_, AppState>,
    login_hint: Option<String>,
) -> Result<Account, SiftError> {
    let (setup_generation, setup_cancel) = state.begin_setup().await;
    // PKCE + loopback flow. Waiting for the browser callback, exchanging the
    // code and reading the profile are all cancellable: a cancelled wizard
    // drops the work instead of letting a late success insert an account.
    let flow = crate::provider::gmail::oauth::start_flow()?;
    let mut url = flow.auth_url.clone();
    if let Some(hint) = login_hint {
        url.push_str(&format!("&login_hint={}", urlencoding::encode(&hint)));
    }
    // open browser
    let _ = crate::opener::open(&app, &url);
    let verifier = flow.verifier.clone();
    let port = flow.port;
    let code = cancellable(&setup_cancel, flow.wait(300)).await?;
    let tokens = cancellable(
        &setup_cancel,
        crate::provider::gmail::oauth::exchange(&code, &verifier, port, &state.http),
    )
    .await?;
    let userinfo_url = std::env::var("SIFT_USERINFO_URL")
        .unwrap_or_else(|_| "https://openidconnect.googleapis.com/v1/userinfo".into());
    let info: crate::provider::gmail::types::Userinfo = cancellable(&setup_cancel, async {
        let response = state
            .http
            .get(userinfo_url)
            .bearer_auth(&tokens.access_token)
            .send()
            .await
            .map_err(SiftError::from)?;
        response.json().await.map_err(SiftError::from)
    })
    .await?;
    let refresh_token = tokens
        .refresh_token
        .as_deref()
        .ok_or_else(|| SiftError::reauth("Google did not return a refresh token"))?;

    let email = normalize_email(&info.email);
    let account = commit_signin(
        &state,
        setup_generation,
        &setup_cancel,
        Credential::RefreshToken(refresh_token),
        &email,
        info.name.clone(),
        info.picture.clone(),
        "oauth",
    )
    .await?;

    if tokens.scope.is_some() {
        state
            .db
            .accounts_set_scope(&account.id, tokens.scope.as_deref())
            .await
            .map_err(db_error)?;
    }
    let provider: Arc<dyn Provider> = Arc::new(crate::provider::gmail::api::GmailApiProvider::new(
        account.id.clone(),
        crate::provider::gmail::client::GmailClient::new(tokens.access_token.clone()),
    ));
    install_provider(
        &state,
        &account.id,
        provider,
        Some(TokenInfo {
            access: tokens.access_token.clone(),
            expires_at: crate::db::now_ms() + tokens.expires_in * 1000,
        }),
    )
    .await?;
    Ok(account)
}

/// Cancel the wizard's in-flight sign-in. Harmless when nothing is running.
#[tauri::command]
pub async fn accounts_cancel_add(state: State<'_, AppState>) -> Result<(), SiftError> {
    state.cancel_setup().await;
    Ok(())
}

/// What is still local to the account when the user asks to remove it: drafts
/// that never left, ops still queued, and ops whose outcome is unknown.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct RemovalCounts {
    pub drafts: i64,
    pub queued: i64,
    #[serde(rename = "uncertainSends")]
    pub uncertain_sends: i64,
}

/// Ops that already left local control: accepted for sending, or mid-flight
/// when the account was removed. Their outcome is unknown, so removal records
/// them rather than dropping them.
const UNCERTAIN_SENDS: &str = "SELECT id, kind, payload, state, created_at FROM outbox_ops \
                               WHERE account_id=? AND (state='inflight' OR kind='send') ORDER BY id";
const UNCERTAIN_SENDS_COUNT: &str = "SELECT count(*) FROM outbox_ops \
                                      WHERE account_id=? AND (state='inflight' OR kind='send')";

#[derive(serde::Serialize)]
struct RemovalOp {
    id: i64,
    kind: String,
    payload: String,
    state: String,
    created_at: i64,
}

#[derive(serde::Serialize)]
struct RemovalReport {
    account_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    email: Option<String>,
    removed_at: i64,
    uncertain_sends: Vec<RemovalOp>,
    unsent_drafts: i64,
}

async fn removal_counts(db: &Db, account_id: &str) -> Result<RemovalCounts, SiftError> {
    let drafts = db.drafts_count(account_id).await.map_err(db_error)?;
    let queued = db.outbox_pending_count(account_id).await.map_err(db_error)?;
    let account_id = account_id.to_string();
    let uncertain_sends = db
        .read(move |c| -> anyhow::Result<i64> {
            Ok(c.query_row(UNCERTAIN_SENDS_COUNT, rusqlite::params![account_id], |r| {
                r.get(0)
            })?)
        })
        .await
        .map_err(db_error)?;
    Ok(RemovalCounts {
        drafts,
        queued,
        uncertain_sends,
    })
}

/// Counts for the removal dialog, so the UI can explain (and export) what is
/// still unsent before the account disappears. R1 registers this command.
#[tauri::command]
pub async fn accounts_removal_preview(
    state: State<'_, AppState>,
    id: String,
) -> Result<RemovalCounts, SiftError> {
    removal_counts(&state.db, &id).await
}

/// Write the durable record of a removal under `data_dir/removed-accounts/`.
/// Returns the path, or `None` when there was nothing to record.
async fn write_removal_report(
    state: &AppState,
    account_id: &str,
    email: Option<&str>,
) -> Result<Option<PathBuf>, SiftError> {
    let key = account_id.to_string();
    let ops: Vec<RemovalOp> = state
        .db
        .read(move |c| -> anyhow::Result<Vec<RemovalOp>> {
            let mut statement = c.prepare(UNCERTAIN_SENDS)?;
            let rows = statement.query_map(rusqlite::params![key], |row| {
                Ok(RemovalOp {
                    id: row.get(0)?,
                    kind: row.get(1)?,
                    payload: row.get(2)?,
                    state: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
        .await
        .map_err(db_error)?;
    let unsent_drafts = state
        .db
        .drafts_count(account_id)
        .await
        .map_err(db_error)?;
    if ops.is_empty() && unsent_drafts == 0 {
        return Ok(None);
    }
    let dir = state.data_dir.join("removed-accounts");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{account_id}.json"));
    let report = RemovalReport {
        account_id: account_id.to_string(),
        email: email.map(str::to_string),
        removed_at: crate::db::now_ms(),
        uncertain_sends: ops,
        unsent_drafts,
    };
    std::fs::write(&path, serde_json::to_vec_pretty(&report)?)?;
    Ok(Some(path))
}

/// Remove an account: stop its work first, record anything whose outcome is
/// still unknown, then delete secrets and rows. The ordering matters — a send
/// that was already accepted must be reported before its outbox row goes away.
pub async fn remove_account(state: &AppState, id: &str) -> Result<(), SiftError> {
    let account = state.db.accounts_get(id).await.map_err(db_error)?;
    let email = account.as_ref().map(|a| a.email.clone());
    // 1. Cancel the account's generation, await its tasks (bounded) and drop
    //    its providers: nothing can write or emit for it after this returns.
    state.cancel_account(id).await;
    // 2. Record accepted-but-unconfirmed sends and unsent drafts durably.
    match write_removal_report(state, id, email.as_deref()).await {
        Ok(Some(path)) => log::warn!(
            target: "sift::accounts",
            "removing {id}: uncertain/unsent work recorded at {}",
            path.display()
        ),
        Ok(None) => {}
        Err(error) => log::warn!(
            target: "sift::accounts",
            "removing {id}: could not record uncertain/unsent work: {error}"
        ),
    }
    // 3. Secrets, then the rows. `accounts_remove` covers every account-scoped
    //    table, including the outbox.
    if let Some(email) = &email {
        if let Err(error) = crate::secrets::delete(email) {
            log::warn!(target: "sift::accounts", "removing {id}: secret delete failed: {error}");
        }
    }
    state.db.accounts_remove(id).await.map_err(db_error)
}

#[tauri::command]
pub async fn accounts_remove(state: State<'_, AppState>, id: String) -> Result<(), SiftError> {
    remove_account(&state, &id).await
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
/// (null) when DNS fails/offline - the wizard proceeds silently then.
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
/// starts only after the row exists (P11-T14). A cancelled wizard can never
/// commit, and a re-add of a known mailbox reuses its account id (P4.4).
#[tauri::command]
pub async fn accounts_add_app_password(
    app: AppHandle,
    state: tauri::State<'_, AppState>,
    email: String,
    app_password: String,
    progress: tauri::ipc::Channel<SetupProgress>,
) -> Result<Account, SiftError> {
    let email = normalize_email(&email);
    let pw = normalize_app_password(&app_password).ok_or_else(|| {
        SiftError::app(
            "imap_bad_password",
            "That app password didn\'t work. Create a fresh one and paste it again.",
            false,
        )
    })?;
    let (setup_generation, setup_cancel) = state.begin_setup().await;
    log::info!(target: "sift::setup", "app-password sign-in: connecting");
    let _ = progress.send(SetupProgress::Connecting);
    // Build a non-cached pool for verification (no DB writes yet).
    let pool = crate::provider::imap::conn::ImapPool::gmail(email.clone(), pw.clone());
    let verifier = crate::provider::imap::provider::GmailImapProvider::new(
        "verify".into(),
        pool.clone(),
        state.db.clone(),
    );
    let _ = progress.send(SetupProgress::Authenticating);
    cancellable(&setup_cancel, verifier.verify())
        .await
        .inspect_err(|e| {
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
    cancellable(&setup_cancel, verifier.list_labels())
        .await
        .inspect_err(|e| {
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
    let account = commit_signin(
        &state,
        setup_generation,
        &setup_cancel,
        Credential::AppPassword(&pw),
        &email,
        None,
        None,
        "app_password",
    )
    .await?;
    state.remember_app_password(&email, &pw).await;
    // Provider + background full sync (first store:threads within ~2 s).
    let provider: Arc<dyn Provider> = Arc::new(
        crate::provider::imap::provider::GmailImapProvider::new(
            account.id.clone(),
            pool,
            state.db.clone(),
        ),
    );
    let (generation, account_cancel) = install_provider(&state, &account.id, provider.clone(), None).await?;
    log::info!(target: "sift::setup", "app-password sign-in: account created, starting sync");
    let _ = progress.send(SetupProgress::Syncing);
    let (db, sink_acc, app2, app3) = (
        state.db.clone(),
        account.id.clone(),
        app.clone(),
        app.clone(),
    );
    let task = tokio::spawn(async move {
        let sink = crate::provider::DbSink::with_progress(db.clone(), move |s| {
            let _ = app2.emit("sync:state", &s);
        });
        let synced = tokio::select! {
            _ = account_cancel.cancelled() => return,
            synced = provider.full_sync(&sink, account_cancel.clone()) => synced,
        };
        // A removed account must not write a cursor or emit anything else.
        if account_cancel.is_cancelled() {
            return;
        }
        match synced {
            Ok(cursor) => {
                let _ = db
                    .accounts_set_history(&sink_acc, &cursor.render(), crate::db::now_ms())
                    .await;
                let _ = db.accounts_set_state(&sink_acc, "partial").await;
                // Tell the UI the first sync finished so it can stop showing
                // progress and refresh counts.
                let _ = app3.emit(
                    "sync:state",
                    serde_json::json!({"account_id": sink_acc, "phase": "done", "done": 0, "total": 0, "last_error": null}),
                );
                let _ = app3.emit(
                    "store:labels",
                    serde_json::json!({ "account_id": sink_acc }),
                );
                let _ = app3.emit(
                    "store:threads",
                    serde_json::json!({ "account_id": sink_acc, "thread_ids": [] }),
                );
            }
            Err(e) => {
                let _ = db.accounts_set_state(&sink_acc, "error").await;
                let _ = app3.emit(
                    "sync:state",
                    serde_json::json!({"account_id": sink_acc, "phase": "error", "done": 0, "total": 0, "last_error": e.to_string()}),
                );
                eprintln!("app-password full sync failed: {e}");
            }
        }
    });
    state.register_task(&account.id, generation, task).await;
    let _ = progress.send(SetupProgress::Done);
    Ok(account)
}

/// Re-auth with a fresh app password (banner → Step C). Verification happens
/// before anything is replaced, then the provider generation is swapped and
/// IDLE replaced. Mailbox rows, drafts and cursors are never touched.
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
    let verifier = crate::provider::imap::provider::GmailImapProvider::new(
        id.clone(),
        crate::provider::imap::conn::ImapPool::gmail(acc.email.clone(), pw.clone()),
        state.db.clone(),
    );
    verifier.verify().await?;
    crate::secrets::store_app_password(&acc.email, &pw)?;
    state.remember_app_password(&acc.email, &pw).await;
    let provider: Arc<dyn Provider> = Arc::new(
        crate::provider::imap::provider::GmailImapProvider::new(
            id.clone(),
            crate::provider::imap::conn::ImapPool::gmail(acc.email.clone(), pw),
            state.db.clone(),
        ),
    );
    install_provider(&state, &id, provider, None).await?;
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

    async fn state_with_account(dir: &tempfile::TempDir, email: &str) -> (AppState, Account) {
        let db = Db::open(dir.path()).unwrap();
        let account = db.new_account(email, None, None).await.unwrap();
        (
            AppState::new(db, dir.path().to_path_buf()),
            account,
        )
    }

    /// Removal stops the account's work, records the send it already accepted
    /// (plus unsent drafts) and leaves no rows behind.
    #[tokio::test]
    async fn p44_t07_removal_records_uncertain_sends_and_counts() {
        let dir = tempfile::tempdir().unwrap();
        let (state, account) = state_with_account(&dir, "gone@x.com").await;
        let id = account.id.clone();
        state
            .db
            .outbox_enqueue(&id, "send", "{\"thread_id\":\"t1\"}", None, 0)
            .await
            .unwrap();
        state
            .db
            .outbox_enqueue(&id, "modify_labels", "{\"ids\":[]}", None, 0)
            .await
            .unwrap();
        // The send was accepted: it is mid-flight when the account is removed.
        state
            .db
            .write({
                let id = id.clone();
                move |c| -> anyhow::Result<()> {
                    c.execute(
                        "UPDATE outbox_ops SET state='inflight' WHERE account_id=? AND kind='send'",
                        rusqlite::params![id],
                    )?;
                    Ok(())
                }
            })
            .await
            .unwrap();
        // Two drafts that never left, and a thread that must cascade away.
        state
            .db
            .write({
                let id = id.clone();
                move |c| -> anyhow::Result<()> {
                    for draft in ["d1", "d2"] {
                        c.execute(
                            "INSERT INTO drafts (local_id, account_id, mode, updated_at) VALUES (?,?,'new',1)",
                            rusqlite::params![draft, id],
                        )?;
                    }
                    c.execute(
                        "INSERT INTO threads (account_id,id,subject,last_message_at,first_message_at) VALUES (?, 't1','s',1,1)",
                        rusqlite::params![id],
                    )?;
                    Ok(())
                }
            })
            .await
            .unwrap();

        let preview = removal_counts(&state.db, &id).await.unwrap();
        assert_eq!(preview.drafts, 2);
        assert_eq!(preview.queued, 2);
        assert_eq!(preview.uncertain_sends, 1);

        remove_account(&state, &id).await.unwrap();

        assert!(state.db.accounts_list().await.unwrap().is_empty());
        assert_eq!(state.db.outbox_pending_count(&id).await.unwrap(), 0);
        assert!(state.db.drafts_list(std::slice::from_ref(&id), None, 10).await.unwrap().drafts.is_empty());
        let threads: i64 = state
            .db
            .read({
                let id = id.clone();
                move |c| -> anyhow::Result<i64> {
                    Ok(c.query_row(
                        "SELECT count(*) FROM threads WHERE account_id=?",
                        rusqlite::params![id],
                        |r| r.get(0),
                    )?)
                }
            })
            .await
            .unwrap();
        assert_eq!(threads, 0);

        let report: serde_json::Value = serde_json::from_slice(
            &std::fs::read(dir.path().join("removed-accounts").join(format!("{id}.json"))).unwrap(),
        )
        .unwrap();
        assert_eq!(report["account_id"], id);
        assert_eq!(report["email"], "gone@x.com");
        assert_eq!(report["unsent_drafts"], 2);
        let sends = report["uncertain_sends"].as_array().unwrap();
        assert_eq!(sends.len(), 1);
        assert_eq!(sends[0]["kind"], "send");
        assert_eq!(sends[0]["state"], "inflight");
        assert_eq!(sends[0]["payload"], "{\"thread_id\":\"t1\"}");
        assert!(sends[0]["created_at"].as_i64().unwrap() > 0);
    }

    /// A wizard cancelled while verification is on the network cannot insert
    /// an account afterwards.
    #[tokio::test]
    async fn p44_t08_cancelled_setup_never_inserts() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _) = state_with_account(&dir, "other@x.com").await;
        let (setup_generation, setup_cancel) = state.begin_setup().await;
        state.cancel_setup().await;
        let error = commit_signin(
            &state,
            setup_generation,
            &setup_cancel,
            Credential::AppPassword("abcdefghijklmnop"),
            "Late@X.com",
            None,
            None,
            "app_password",
        )
        .await
        .err()
        .unwrap();
        assert_eq!(
            serde_json::to_value(&error).unwrap()["code"],
            "setup_cancelled"
        );
        let emails: Vec<String> = state
            .db
            .accounts_list()
            .await
            .unwrap()
            .into_iter()
            .map(|a| a.email)
            .collect();
        assert_eq!(emails, vec!["other@x.com".to_string()]);
        // Nothing was written to the Keychain either.
        assert_eq!(
            crate::secrets::load_app_password("late@x.com").unwrap(),
            None
        );
    }

    /// Signing in again with the same email (any casing or padding) reuses the
    /// account id instead of creating a parallel copy.
    #[tokio::test]
    async fn p44_t09_readd_same_email_reuses_account_id() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _) = state_with_account(&dir, "seed@x.com").await;
        let (generation, cancel) = state.begin_setup().await;
        let first = commit_signin(
            &state,
            generation,
            &cancel,
            Credential::AppPassword("abcdefghijklmnop"),
            "A@X.com",
            None,
            None,
            "app_password",
        )
        .await
        .unwrap();
        let (generation, cancel) = state.begin_setup().await;
        let again = commit_signin(
            &state,
            generation,
            &cancel,
            Credential::RefreshToken("refresh"),
            "  A@X.com ",
            Some("Ann".into()),
            None,
            "oauth",
        )
        .await
        .unwrap();
        assert_eq!(again.id, first.id);
        assert_eq!(again.email, "a@x.com");
        assert_eq!(again.auth_kind, "oauth");
        assert_eq!(again.display_name.as_deref(), Some("Ann"));
        let accounts = state.db.accounts_list().await.unwrap();
        assert_eq!(accounts.len(), 2, "seed + one account, never a copy");
        assert_eq!(
            state
                .db
                .accounts_find_by_email(" A@X.com ")
                .await
                .unwrap()
                .unwrap()
                .id,
            first.id
        );
    }

    /// A failed row write must not leave the credential behind.
    #[tokio::test]
    async fn p44_t10_failed_row_write_restores_secret() {
        let dir = tempfile::tempdir().unwrap();
        let (state, _) = state_with_account(&dir, "seed@x.com").await;
        // Any insert into `accounts` fails, so the row write cannot succeed.
        state
            .db
            .write(|c| -> anyhow::Result<()> {
                c.execute_batch(
                    "CREATE TRIGGER accounts_insert_fails BEFORE INSERT ON accounts \
                     BEGIN SELECT RAISE(ABORT, 'accounts table is unavailable'); END;",
                )?;
                Ok(())
            })
            .await
            .unwrap();
        let (generation, cancel) = state.begin_setup().await;
        let error = commit_signin(
            &state,
            generation,
            &cancel,
            Credential::AppPassword("abcdefghijklmnop"),
            "broken@x.com",
            None,
            None,
            "app_password",
        )
        .await
        .err()
        .unwrap();
        assert_eq!(serde_json::to_value(&error).unwrap()["code"], "db");
        assert_eq!(
            crate::secrets::load_app_password("broken@x.com").unwrap(),
            None,
            "the app password written before the failed row must be removed"
        );
        let emails: Vec<String> = state
            .db
            .accounts_list()
            .await
            .unwrap()
            .into_iter()
            .map(|a| a.email)
            .collect();
        assert_eq!(emails, vec!["seed@x.com".to_string()]);
    }

    /// Reauthentication swaps the provider generation and preserves mailbox
    /// rows; an op that was mid-flight goes back in the queue instead of
    /// being lost.
    #[tokio::test]
    async fn p44_t11_reauth_swaps_generation_and_preserves_rows() {
        let dir = tempfile::tempdir().unwrap();
        let (state, account) = state_with_account(&dir, "reauth@x.com").await;
        let id = account.id.clone();
        state
            .db
            .accounts_set_auth_kind(&id, "app_password")
            .await
            .unwrap();
        state
            .db
            .write({
                let id = id.clone();
                move |c| -> anyhow::Result<()> {
                    c.execute(
                        "INSERT INTO threads (account_id,id,subject,last_message_at,first_message_at) VALUES (?, 't1','s',1,1)",
                        rusqlite::params![id],
                    )?;
                    c.execute(
                        "INSERT INTO drafts (local_id, account_id, mode, updated_at) VALUES ('d1',?, 'new',1)",
                        rusqlite::params![id],
                    )?;
                    Ok(())
                }
            })
            .await
            .unwrap();
        state
            .db
            .outbox_enqueue(&id, "modify_labels", "{}", None, 0)
            .await
            .unwrap();
        state
            .db
            .write({
                let id = id.clone();
                move |c| -> anyhow::Result<()> {
                    c.execute(
                        "UPDATE outbox_ops SET state='inflight' WHERE account_id=?",
                        rusqlite::params![id],
                    )?;
                    Ok(())
                }
            })
            .await
            .unwrap();
        let (old_generation, _) = state.begin_generation(&id).await;

        let provider: Arc<dyn Provider> = Arc::new(
            crate::provider::imap::provider::GmailImapProvider::new(
                id.clone(),
                crate::provider::imap::conn::ImapPool::gmail("reauth@x.com".into(), "abcdefghijklmnop".into()),
                state.db.clone(),
            ),
        );
        let (generation, _) = install_provider(&state, &id, provider.clone(), None)
            .await
            .unwrap();

        assert!(generation > old_generation);
        assert!(state.is_current(&id, generation).await);
        assert!(!state.is_current(&id, old_generation).await);
        assert!(state.providers.read().await.contains_key(&id));
        let threads: i64 = state
            .db
            .read({
                let id = id.clone();
                move |c| -> anyhow::Result<i64> {
                    Ok(c.query_row(
                        "SELECT count(*) FROM threads WHERE account_id=?",
                        rusqlite::params![id],
                        |r| r.get(0),
                    )?)
                }
            })
            .await
            .unwrap();
        assert_eq!(threads, 1);
        assert_eq!(state.db.drafts_list(std::slice::from_ref(&id), None, 10).await.unwrap().drafts.len(), 1);
        // The mid-flight op is queued again for the new provider.
        assert_eq!(state.db.outbox_pending_count(&id).await.unwrap(), 1);
        assert_eq!(
            state.db.accounts_get(&id).await.unwrap().unwrap().sync_state,
            "partial"
        );
    }
}
