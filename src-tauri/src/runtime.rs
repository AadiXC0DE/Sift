use crate::app_state::AppState;
use crate::db::Db;
use crate::provider::Provider;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
use tokio_util::sync::CancellationToken;

/// Where the account loops push user-visible output. In the app this wraps the
/// `AppHandle`; tests record into plain closures instead of standing up a
/// Tauri runtime.
pub type EmitFn = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;
pub type NotifyFn = Arc<dyn Fn(&str, &str) + Send + Sync>;
pub type FocusedFn = Arc<dyn Fn() -> bool + Send + Sync>;

pub struct RuntimeHost {
    pub emit: EmitFn,
    pub notify_os: NotifyFn,
    pub focused: FocusedFn,
}

impl RuntimeHost {
    pub fn for_app(app: &AppHandle) -> Self {
        let emitter = app.clone();
        let notifier = app.clone();
        let windows = app.clone();
        Self {
            emit: Arc::new(move |event, payload| {
                let _ = emitter.emit(event, payload);
            }),
            notify_os: Arc::new(move |title, body| {
                let _ = (&notifier, title, body);
                #[cfg(not(test))]
                {
                    use tauri_plugin_notification::NotificationExt;
                    let _ = notifier
                        .notification()
                        .builder()
                        .title(title)
                        .body(body)
                        .show();
                }
            }),
            focused: Arc::new(move || {
                windows
                    .webview_windows()
                    .values()
                    .any(|w| w.is_focused().unwrap_or(false))
            }),
        }
    }
}

/// Background supervisor: per-account poll/drain/backfill/IDLE loops plus a
/// global snooze watcher. This is what makes Sift live: without it, `sync_now`
/// is a manual one-shot and outbox ops never leave the queue.
///
/// Every account's loops belong to a *generation*. Removing or reconnecting an
/// account cancels that generation, and the next supervisor tick re-arms the
/// account only if a newer, live generation exists (P4.4).
pub fn spawn_supervisor(app: AppHandle) {
    // Snooze watcher (global, cheap).
    let snooze_app = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(30)).await;
            if let Err(e) = check_snoozes(&snooze_app).await {
                eprintln!("snooze watcher: {e}");
            }
        }
    });

    // Per-account loops, spawned once accounts exist and re-armed whenever an
    // account's generation changes (reauth, re-add).
    tauri::async_runtime::spawn(async move {
        let mut armed: HashMap<String, u64> = HashMap::new();
        loop {
            let accounts: Vec<String> = app
                .state::<AppState>()
                .db
                .accounts_list()
                .await
                .map(|v| v.into_iter().map(|a| a.id).collect())
                .unwrap_or_default();
            for account_id in &accounts {
                let runtime = {
                    let state = app.state::<AppState>();
                    match state.runtime_state(account_id).await {
                        Some(runtime) => runtime,
                        None => state.begin_generation(account_id).await,
                    }
                };
                let (generation, cancel) = runtime;
                if armed.get(account_id) == Some(&generation) {
                    continue;
                }
                armed.remove(account_id);
                // A cancelled generation is an account on its way out (or
                // between reauth generations): arming work for it would only
                // start tasks that immediately stop again.
                if cancel.is_cancelled() {
                    continue;
                }
                armed.insert(account_id.clone(), generation);
                spawn_account_loops(&app, account_id, generation, cancel).await;
            }
            armed.retain(|id, _| accounts.contains(id));
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

/// Spawn one account's loops for one generation and register the handles, so
/// removal and reauth can cancel and await them.
async fn spawn_account_loops(
    app: &AppHandle,
    account_id: &str,
    generation: u64,
    cancel: CancellationToken,
) {
    let state = app.state::<AppState>();
    macro_rules! spawn_loop {
        ($loops:ident) => {{
            let app = app.clone();
            let account_id = account_id.to_string();
            let cancel = cancel.clone();
            tokio::spawn(async move {
                let state = app.state::<AppState>();
                let host = RuntimeHost::for_app(&app);
                $loops(&state, &account_id, generation, &cancel, &host).await;
            })
        }};
    }
    let handles = vec![
        spawn_loop!(poll_loop),
        spawn_loop!(drain_loop),
        spawn_loop!(backfill_loop),
        spawn_loop!(idle_loop),
    ];
    for handle in handles {
        state.register_task(account_id, generation, handle).await;
    }
}

/// A provider for the account, or `None` when the generation was cancelled
/// while it was being built.
async fn provider(state: &AppState, account_id: &str, cancel: &CancellationToken) -> Option<Arc<dyn Provider>> {
    tokio::select! {
        _ = cancel.cancelled() => None,
        built = state.provider_for(account_id) => built.ok(),
    }
}

/// Sleep unless the account's generation ends first. `false` means: stop.
async fn sleep_or_cancel(cancel: &CancellationToken, secs: u64) -> bool {
    tokio::select! {
        _ = cancel.cancelled() => false,
        _ = tokio::time::sleep(Duration::from_secs(secs)) => true,
    }
}

/// Poll loop: partial sync at the focused or background cadence.
async fn poll_loop(
    state: &AppState,
    account_id: &str,
    generation: u64,
    cancel: &CancellationToken,
    host: &RuntimeHost,
) {
    loop {
        let settings = state.db.settings_get().await.unwrap_or_default();
        // Focused/background distinction comes from the window; poll at the
        // focused cadence when any window is focused, else background.
        let wait = if (host.focused)() {
            settings.poll_focused.max(5) as u64
        } else {
            settings.poll_background.max(15) as u64
        };
        if state.is_online() && !crate::demo::is_demo() {
            if let Some(provider) = provider(state, account_id, cancel).await {
                partial_tick(state, account_id, generation, cancel, &*provider, host).await;
            }
        }
        if !sleep_or_cancel(cancel, wait).await {
            return;
        }
    }
}

/// Outbox drain loop.
async fn drain_loop(
    state: &AppState,
    account_id: &str,
    generation: u64,
    cancel: &CancellationToken,
    host: &RuntimeHost,
) {
    loop {
        let online = state.is_online();
        let mut worked = false;
        if online && !crate::demo::is_demo() {
            if let Some(provider) = provider(state, account_id, cancel).await {
                let drained = tokio::select! {
                    _ = cancel.cancelled() => return,
                    drained = crate::outbox::drain_one(&state.db, &*provider, account_id, true) => drained,
                };
                match drained {
                    Ok(true) => {
                        worked = true;
                        if !state.is_current(account_id, generation).await {
                            return;
                        }
                        // Fresh server state after every mutation.
                        let cursor = read_cursor(&state.db, account_id).await;
                        let sink = db_sink(&state.db);
                        let synced = tokio::select! {
                            _ = cancel.cancelled() => return,
                            synced = provider.partial_sync(&cursor, &sink) => synced,
                        };
                        if !state.is_current(account_id, generation).await {
                            return;
                        }
                        if let Ok(crate::provider::PartialOutcome::Synced {
                            changed_threads,
                            ..
                        }) = synced
                        {
                            emit_store(host, account_id, &changed_threads).await;
                        }
                    }
                    Ok(false) => {}
                    Err(_) => {
                        // Failed ops revert + toast at the action layer via outbox:state.
                    }
                }
            }
        }
        if let Ok(n) = state.db.outbox_pending_count(account_id).await {
            let summary = state
                .db
                .outbox_summary(account_id)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(label, count)| serde_json::json!({ "label": label, "count": count }))
                .collect::<Vec<_>>();
            (host.emit)(
                "outbox:state",
                serde_json::json!({"account_id": account_id, "pending": n, "failed": 0, "summary": summary}),
            );
        }
        if !sleep_or_cancel(cancel, if worked { 1 } else { 5 }).await {
            return;
        }
    }
}

/// Body backfill loop (low priority, yields to foreground fetches).
async fn backfill_loop(
    state: &AppState,
    account_id: &str,
    generation: u64,
    cancel: &CancellationToken,
    _host: &RuntimeHost,
) {
    loop {
        if state.is_online() && !crate::demo::is_demo() {
            if let Some(provider) = provider(state, account_id, cancel).await {
                let settings = state.db.settings_get().await.unwrap_or_default();
                let horizon_days = match settings.offline_body_cache.as_str() {
                    "6m" => 180,
                    "all" => 365 * 30,
                    _ => 730,
                };
                let sink = db_sink(&state.db);
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = crate::sync::backfill::run_backfill_once(
                        &sink,
                        account_id,
                        &*provider,
                        &state.gate,
                        horizon_days,
                    ) => {}
                }
                if !state.is_current(account_id, generation).await {
                    return;
                }
            }
        }
        if !sleep_or_cancel(cancel, 20).await {
            return;
        }
    }
}

/// IDLE push consumer (IMAP only; `watch()` is None for REST).
/// Debounces 1 s, then runs the same partial tick as the poll loop.
async fn idle_loop(
    state: &AppState,
    account_id: &str,
    generation: u64,
    cancel: &CancellationToken,
    host: &RuntimeHost,
) {
    use futures::StreamExt;
    loop {
        let stream = tokio::select! {
            _ = cancel.cancelled() => return,
            provider = state.provider_for(account_id) => match provider {
                Ok(provider) => provider.watch(),
                Err(_) => None,
            },
        };
        let Some(mut stream) = stream else {
            // REST accounts (and unknown): the poll loop covers them.
            return;
        };
        loop {
            let event = tokio::select! {
                _ = cancel.cancelled() => return,
                event = stream.next() => event,
            };
            if event.is_none() {
                break;
            }
            // Debounce: collapse IDLE flurries, drain anything queued.
            if !sleep_or_cancel(cancel, 1).await {
                return;
            }
            while tokio::time::timeout(Duration::from_millis(200), stream.next())
                .await
                .is_ok_and(|v| v.is_some())
            {
                if cancel.is_cancelled() {
                    return;
                }
            }
            if state.is_online() && !crate::demo::is_demo() {
                if let Some(provider) = provider(state, account_id, cancel).await {
                    partial_tick(state, account_id, generation, cancel, &*provider, host).await;
                }
            }
        }
        // Stream ended (IDLE error path): polling covered the gap;
        // re-arm shortly (IDLE task itself also retries internally).
        if !sleep_or_cancel(cancel, 30).await {
            return;
        }
    }
}

/// One partial tick shared by the poll loop and the IDLE consumer. Network
/// results that land after the account's generation ended are dropped, never
/// written or emitted (P4.4).
async fn partial_tick(
    state: &AppState,
    account_id: &str,
    generation: u64,
    cancel: &CancellationToken,
    provider: &dyn crate::provider::Provider,
    host: &RuntimeHost,
) {
    use crate::provider::PartialOutcome;
    let cursor = read_cursor(&state.db, account_id).await;
    let sink = db_sink(&state.db);
    let outcome = tokio::select! {
        _ = cancel.cancelled() => return,
        outcome = provider.partial_sync(&cursor, &sink) => outcome,
    };
    if !state.is_current(account_id, generation).await {
        return;
    }
    match outcome {
        Ok(PartialOutcome::Synced {
            changed_threads,
            new_inbox,
        }) => {
            emit_store(host, account_id, &changed_threads).await;
            notify_new(&state.db, host, &new_inbox).await;
        }
        Ok(PartialOutcome::NeedsFull) => {
            recover_full(state, account_id, generation, cancel, provider, host).await;
        }
        Err(e) => {
            (host.emit)(
                "sync:state",
                serde_json::json!({"account_id": account_id, "phase": "error", "done": 0, "total": 0, "last_error": e.to_string()}),
            );
        }
    }
}

fn db_sink(db: &Db) -> crate::provider::DbSink {
    crate::provider::DbSink::new(db.clone())
}

async fn read_cursor(db: &Db, account_id: &str) -> crate::provider::Cursor {
    let raw: Option<String> = db
        .accounts_get(account_id)
        .await
        .ok()
        .flatten()
        .and_then(|a| a.history_id);
    crate::provider::Cursor::parse(raw.as_deref().unwrap_or(""))
}

/// Recover from an unusable cursor: reconcile REST history, rebuild IMAP
/// folders from scratch. Providers persist their own cursors; the runtime
/// only refreshes what the UI shows.
async fn recover_full(
    state: &AppState,
    account_id: &str,
    generation: u64,
    cancel: &CancellationToken,
    provider: &dyn crate::provider::Provider,
    host: &RuntimeHost,
) {
    use crate::provider::{Cursor, ProviderKind};
    let sink = db_sink(&state.db);
    match provider.kind() {
        ProviderKind::GmailApi => {
            // History lost mid-tick (rare: provider already reconciles once
            // internally). Reconcile again defensively; the next tick continues.
            let cursor = Cursor::Gmail {
                history_id: String::new(),
            };
            let reconciled = tokio::select! {
                _ = cancel.cancelled() => return,
                reconciled = provider.partial_sync(&cursor, &sink) => reconciled,
            };
            if !state.is_current(account_id, generation).await {
                return;
            }
            if reconciled.is_ok() {
                (host.emit)(
                    "store:labels",
                    serde_json::json!({ "account_id": account_id }),
                );
            }
        }
        ProviderKind::GmailImap => {
            // UIDVALIDITY changed: folder state is garbage. full_sync rebuilds
            // each folder idempotently, then persists the fresh cursor.
            let rebuilt = tokio::select! {
                _ = cancel.cancelled() => return,
                rebuilt = provider.full_sync(&sink, cancel.clone()) => rebuilt,
            };
            if !state.is_current(account_id, generation).await {
                return;
            }
            match rebuilt {
                Ok(cursor) => {
                    let _ = state
                        .db
                        .accounts_set_history(account_id, &cursor.render(), crate::db::now_ms())
                        .await;
                    (host.emit)(
                        "store:labels",
                        serde_json::json!({ "account_id": account_id }),
                    );
                    (host.emit)(
                        "sync:state",
                        serde_json::json!({"account_id": account_id, "phase": "done", "done": 0, "total": 0, "last_error": null}),
                    );
                }
                Err(e) => {
                    (host.emit)(
                        "sync:state",
                        serde_json::json!({"account_id": account_id, "phase": "error", "done": 0, "total": 0, "last_error": e.to_string()}),
                    );
                }
            }
        }
    }
}

async fn emit_store(host: &RuntimeHost, account_id: &str, changed: &[(String, String)]) {
    let thread_ids: Vec<String> = changed
        .iter()
        .filter(|(a, _)| a == account_id)
        .map(|(_, t)| t.clone())
        .collect();
    if !thread_ids.is_empty() {
        (host.emit)(
            "store:threads",
            serde_json::json!({"account_id": account_id, "thread_ids": thread_ids}),
        );
    }
    (host.emit)(
        "store:labels",
        serde_json::json!({ "account_id": account_id }),
    );
}

async fn notify_new(db: &Db, host: &RuntimeHost, items: &[(String, String, String, String)]) {
    if items.is_empty() {
        return;
    }
    let settings = db.settings_get().await.unwrap_or_default();
    if settings.notifications == "off" {
        return;
    }
    for (aid, tid, from, subject) in items.iter().take(3) {
        (host.emit)(
            "notify:new-mail",
            serde_json::json!({"account_id": aid, "thread_id": tid, "from": from, "subject": subject}),
        );
        (host.notify_os)(from, subject);
    }
    if items.len() > 3 {
        (host.emit)(
            "notify:new-mail",
            serde_json::json!({"account_id": items[0].0, "thread_id": items[0].1, "from": "Sift", "subject": format!("{} new messages", items.len())}),
        );
    }
}

async fn check_snoozes(app: &AppHandle) -> anyhow::Result<()> {
    let state = app.state::<AppState>();
    let host = RuntimeHost::for_app(app);
    let now = crate::db::now_ms();
    let settings = state.db.settings_get().await.unwrap_or_default();
    let due = crate::scheduler::process_overdue(&state.db, now).await?;
    let mut live = Vec::with_capacity(due.len());
    for (aid, tid) in due {
        // A removed account must not get new rows or events from a wake-up.
        if state.is_cancelled(&aid).await {
            continue;
        }
        live.push((aid, tid));
    }
    let due = live;
    for (aid, tid) in &due {
        // Restore INBOX (+UNREAD per setting) and enqueue the server op.
        let mids: Vec<String> = state
            .db
            .read({
                let (a, t) = (aid.clone(), tid.clone());
                move |c| -> anyhow::Result<Vec<String>> {
                    let mut s =
                        c.prepare("SELECT id FROM messages WHERE account_id=? AND thread_id=?")?;
                    let v: Vec<String> = s
                        .query_map(rusqlite::params![a, t], |r| r.get(0))?
                        .collect::<Result<Vec<String>, rusqlite::Error>>()?;
                    Ok(v)
                }
            })
            .await?;
        for mid in &mids {
            let mut add = vec!["INBOX".to_string()];
            if settings.wake_snoozed_unread {
                add.push("UNREAD".to_string());
            }
            let _ = state
                .db
                .apply_label_change(&crate::dto::MessageRef::new(aid.clone(), mid.clone()), &add, &[])
                .await;
        }
        let payload = serde_json::json!({"ids": mids, "add": ["INBOX"], "remove": []}).to_string();
        let _ = state
            .db
            .outbox_enqueue(aid, "modify_labels", &payload, None, 0)
            .await;
        emit_store(&host, aid, &[(aid.clone(), tid.clone())]).await;
    }
    if !due.is_empty() {
        let tids: Vec<String> = due.iter().map(|(_, t)| t.clone()).collect();
        (host.emit)(
            "snooze:woke",
            serde_json::json!({"account_id": due[0].0, "thread_ids": tids}),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::Label;
    use crate::errors::SiftError;
    use crate::provider::{
        ApplyOutcome, BoxStream, Cursor, OutboxOp, PartialOutcome, ProfileInfo, ProviderKind,
        SentInfo, SendAs, SyncSink, ThreadRef, WatchEvent,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn unused() -> SiftError {
        SiftError::app("unused", "not used by the runtime loops", false)
    }

    /// A provider with no server behind it: it records that work started, then
    /// blocks until the test releases it — or is dropped because the account's
    /// generation ended.
    struct BlockingProvider {
        account_id: String,
        db: Db,
        entered: Arc<tokio::sync::Notify>,
        release: Arc<tokio::sync::Notify>,
    }

    impl BlockingProvider {
        fn new(account_id: &str, db: Db) -> Self {
            Self {
                account_id: account_id.to_string(),
                db,
                entered: Arc::new(tokio::sync::Notify::new()),
                release: Arc::new(tokio::sync::Notify::new()),
            }
        }

        /// Record a row so the test can see exactly how far the provider got.
        async fn mark(&self, kind: &str) {
            let (db, account_id, kind) = (
                self.db.clone(),
                self.account_id.clone(),
                kind.to_string(),
            );
            db.write(move |c| -> anyhow::Result<()> {
                c.execute(
                    "INSERT INTO sync_log (account_id, at, kind) VALUES (?,?,?)",
                    rusqlite::params![account_id, crate::db::now_ms(), kind],
                )?;
                Ok(())
            })
            .await
            .unwrap();
            self.entered.notify_one();
        }

        async fn block_until_released(&self) {
            self.release.notified().await;
        }
    }

    #[async_trait::async_trait]
    impl Provider for BlockingProvider {
        fn kind(&self) -> ProviderKind {
            ProviderKind::GmailImap
        }
        async fn verify(&self) -> Result<ProfileInfo, SiftError> {
            Err(unused())
        }
        async fn list_labels(&self) -> Result<Vec<Label>, SiftError> {
            Ok(vec![])
        }
        async fn create_label(&self, _name: &str) -> Result<Label, SiftError> {
            Err(unused())
        }
        async fn full_sync(
            &self,
            _sink: &dyn SyncSink,
            _cancel: CancellationToken,
        ) -> Result<Cursor, SiftError> {
            Err(unused())
        }
        async fn partial_sync(
            &self,
            _cursor: &Cursor,
            _sink: &dyn SyncSink,
        ) -> Result<PartialOutcome, SiftError> {
            self.mark("sync-entered").await;
            self.block_until_released().await;
            self.mark("sync-late").await;
            Ok(PartialOutcome::Synced {
                changed_threads: vec![(self.account_id.clone(), "t1".into())],
                new_inbox: vec![],
            })
        }
        async fn fetch_body(
            &self,
            _message_id: &str,
        ) -> Result<crate::provider::gmail::mime::ParsedMessage, SiftError> {
            Err(unused())
        }
        async fn fetch_attachment(
            &self,
            _message_id: &str,
            _attachment_id: &str,
        ) -> Result<Vec<u8>, SiftError> {
            Err(unused())
        }
        async fn fetch_raw(&self, _message_id: &str) -> Result<String, SiftError> {
            Err(unused())
        }
        async fn apply(&self, _op: &OutboxOp) -> Result<ApplyOutcome, SiftError> {
            self.mark("send-entered").await;
            self.block_until_released().await;
            self.mark("send-late").await;
            Ok(ApplyOutcome::Done)
        }
        async fn send(&self, _raw: &[u8], _thread_id: Option<&str>) -> Result<SentInfo, SiftError> {
            Err(unused())
        }
        async fn draft_upsert(
            &self,
            _remote_id: Option<&str>,
            _raw: &[u8],
        ) -> Result<String, SiftError> {
            Err(unused())
        }
        async fn draft_delete(&self, _remote_id: &str) -> Result<(), SiftError> {
            Err(unused())
        }
        async fn server_search(
            &self,
            _q: &str,
            _limit: u32,
            _sink: &dyn SyncSink,
        ) -> Result<Vec<ThreadRef>, SiftError> {
            Err(unused())
        }
        async fn send_as_list(&self) -> Result<Vec<SendAs>, SiftError> {
            Ok(vec![])
        }
        fn watch(&self) -> Option<BoxStream<'static, WatchEvent>> {
            None
        }
    }

    fn host(events: Arc<AtomicUsize>) -> RuntimeHost {
        RuntimeHost {
            emit: Arc::new(move |_, _| {
                events.fetch_add(1, Ordering::SeqCst);
            }),
            notify_os: Arc::new(|_, _| {}),
            focused: Arc::new(|| true),
        }
    }

    async fn sync_log_kinds(db: &Db, account_id: &str) -> Vec<String> {
        let account_id = account_id.to_string();
        db.read(move |c| -> anyhow::Result<Vec<String>> {
            let mut statement =
                c.prepare("SELECT kind FROM sync_log WHERE account_id=? ORDER BY id")?;
            let kinds = statement
                .query_map(rusqlite::params![account_id], |row| row.get(0))?
                .collect::<Result<Vec<String>, rusqlite::Error>>()?;
            Ok(kinds)
        })
        .await
        .unwrap()
    }

    async fn outbox_states(db: &Db, account_id: &str) -> Vec<String> {
        let account_id = account_id.to_string();
        db.read(move |c| -> anyhow::Result<Vec<String>> {
            let mut statement =
                c.prepare("SELECT state FROM outbox_ops WHERE account_id=? ORDER BY id")?;
            let states = statement
                .query_map(rusqlite::params![account_id], |row| row.get(0))?
                .collect::<Result<Vec<String>, rusqlite::Error>>()?;
            Ok(states)
        })
        .await
        .unwrap()
    }

    /// Remove during sync: the loop exits, and the network result that lands
    /// afterwards is dropped instead of being written or emitted.
    #[tokio::test]
    async fn p44_t05_removal_stops_sync_loop_without_late_writes() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let account = db.new_account("sync@x.com", None, None).await.unwrap();
        let state = Arc::new(AppState::new(db, dir.path().to_path_buf()));
        let provider = Arc::new(BlockingProvider::new(&account.id, state.db.clone()));
        state
            .providers
            .write()
            .await
            .insert(account.id.clone(), provider.clone());
        let events = Arc::new(AtomicUsize::new(0));
        let (generation, cancel) = state.begin_generation(&account.id).await;
        let handle = {
            let (state, id, cancel, host) = (
                state.clone(),
                account.id.clone(),
                cancel.clone(),
                host(events.clone()),
            );
            tokio::spawn(async move { poll_loop(&state, &id, generation, &cancel, &host).await })
        };
        state.register_task(&account.id, generation, handle).await;

        // Generous bound: the assertion is about post-removal writes, not about
        // how fast the loop starts under a fully loaded parallel test run.
        tokio::time::timeout(Duration::from_secs(30), provider.entered.notified())
            .await
            .expect("the poll loop must reach the sync");

        let started = std::time::Instant::now();
        state.cancel_account(&account.id).await;
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "removal must not sit out the whole join timeout"
        );
        assert!(!state.is_current(&account.id, generation).await);

        // Let the blocked network call finish: a cancelled loop no longer owns
        // it, so nothing else may be written.
        provider.release.notify_waiters();
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(
            sync_log_kinds(&state.db, &account.id).await,
            vec!["sync-entered".to_string()]
        );
        assert_eq!(events.load(Ordering::SeqCst), 0, "no events after removal");
    }

    /// Remove with a paused outbox: the drain stops, the accepted send is left
    /// inflight (so removal records it as uncertain) and no late write lands.
    #[tokio::test]
    async fn p44_t06_removal_stops_a_paused_outbox_drain() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let account = db.new_account("outbox@x.com", None, None).await.unwrap();
        let state = Arc::new(AppState::new(db, dir.path().to_path_buf()));
        let provider = Arc::new(BlockingProvider::new(&account.id, state.db.clone()));
        state
            .providers
            .write()
            .await
            .insert(account.id.clone(), provider.clone());
        state
            .db
            .outbox_enqueue(&account.id, "send", "{\"thread_id\":\"t1\"}", None, 0)
            .await
            .unwrap();
        let events = Arc::new(AtomicUsize::new(0));
        let (generation, cancel) = state.begin_generation(&account.id).await;
        let handle = {
            let (state, id, cancel, host) = (
                state.clone(),
                account.id.clone(),
                cancel.clone(),
                host(events.clone()),
            );
            tokio::spawn(async move { drain_loop(&state, &id, generation, &cancel, &host).await })
        };
        state.register_task(&account.id, generation, handle).await;

        // Generous bound: the assertion is about post-removal writes, not about
        // how fast the loop starts under a fully loaded parallel test run.
        tokio::time::timeout(Duration::from_secs(30), provider.entered.notified())
            .await
            .expect("the drain loop must start the send");
        assert_eq!(
            outbox_states(&state.db, &account.id).await,
            vec!["inflight".to_string()],
            "an accepted send is inflight while the provider applies it"
        );

        let started = std::time::Instant::now();
        state.cancel_account(&account.id).await;
        assert!(started.elapsed() < Duration::from_secs(3));

        provider.release.notify_waiters();
        tokio::time::sleep(Duration::from_millis(250)).await;
        assert_eq!(
            sync_log_kinds(&state.db, &account.id).await,
            vec!["send-entered".to_string()],
            "a released waiter would have written the late row"
        );
        // Nothing confirmed the send, so it stays inflight for the removal
        // record — and the loop emitted nothing after the removal.
        assert_eq!(
            outbox_states(&state.db, &account.id).await,
            vec!["inflight".to_string()]
        );
        assert_eq!(events.load(Ordering::SeqCst), 0);
    }
}
