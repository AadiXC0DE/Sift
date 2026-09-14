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
/// Raise one native notification. `false` means it was **not** shown (the OS
/// denied permission), which is what lets a due reminder stay visibly due
/// instead of being marked delivered.
pub type NotifyFn = Arc<dyn Fn(crate::notify::NotificationRequest) -> bool + Send + Sync>;
pub type FocusedFn = Arc<dyn Fn() -> bool + Send + Sync>;
/// Whether notifications can be raised at all right now.
pub type AvailableFn = Arc<dyn Fn() -> bool + Send + Sync>;
/// Set the native dock badge (0 clears it).
pub type BadgeFn = Arc<dyn Fn(i64) + Send + Sync>;

pub struct RuntimeHost {
    pub emit: EmitFn,
    pub notify_os: NotifyFn,
    pub focused: FocusedFn,
    pub notifications_available: AvailableFn,
    pub badge: BadgeFn,
}

/// The most recent notification Sift raised, so bringing the app forward can
/// open what it was about.
///
/// The OS reports *focus*, not which notification was clicked: the desktop
/// notification API in use exposes no activation callback. Sift therefore
/// remembers the ref it last notified about and opens it when the window comes
/// forward within a short window after the banner — which is what a click does,
/// and also what a deliberate switch back to the app does. Both land the user
/// on the conversation they were just told about, and a stale ref is dropped
/// rather than surprising anyone later.
const NAV_FRESHNESS_MS: i64 = 10 * 60 * 1000;

static PENDING_NAV: std::sync::LazyLock<std::sync::Mutex<Option<(String, String, i64)>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

pub fn remember_nav_target(account_id: &str, thread_id: &str) {
    if let Ok(mut slot) = PENDING_NAV.lock() {
        *slot = Some((
            account_id.to_string(),
            thread_id.to_string(),
            crate::db::now_ms(),
        ));
    }
}

/// Consume the pending navigation target, if it is still fresh.
pub fn take_pending_nav() -> Option<(String, String)> {
    let mut slot = PENDING_NAV.lock().ok()?;
    let (account, thread, at) = slot.take()?;
    if crate::db::now_ms() - at > NAV_FRESHNESS_MS {
        return None;
    }
    Some((account, thread))
}

/// Bring the app forward and open the thread the last notification named.
/// Called when the window gains focus.
pub fn flush_pending_nav(app: &AppHandle) {
    let Some((account_id, thread_id)) = take_pending_nav() else {
        return;
    };
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    let _ = app.emit(
        "nav:open-thread",
        serde_json::json!({ "accountId": account_id, "threadId": thread_id }),
    );
}

impl RuntimeHost {
    pub fn for_app(app: &AppHandle) -> Self {
        let emitter = app.clone();
        #[cfg(not(test))]
        let notifier = app.clone();
        let windows = app.clone();
        let availability = app.clone();
        let badged = app.clone();
        Self {
            emit: Arc::new(move |event, payload| {
                let _ = emitter.emit(event, payload);
            }),
            notify_os: Arc::new(move |request| {
                // The ref is remembered before the banner so a click that
                // arrives immediately still has somewhere to go.
                remember_nav_target(&request.account_id, &request.thread_id);
                #[cfg(not(test))]
                {
                    use tauri_plugin_notification::NotificationExt;
                    if crate::notify::permission_state(&notifier) == "denied" {
                        return false;
                    }
                    let mut builder = notifier
                        .notification()
                        .builder()
                        .title(request.title.clone())
                        .body(request.body.clone());
                    // Native sound only: the OS decides whether it is audible,
                    // which is what makes Do Not Disturb work.
                    if request.sound {
                        builder = builder.sound("default");
                    }
                    builder.show().is_ok()
                }
                #[cfg(test)]
                {
                    true
                }
            }),
            focused: Arc::new(move || {
                windows
                    .webview_windows()
                    .values()
                    .any(|w| w.is_focused().unwrap_or(false))
            }),
            notifications_available: Arc::new(move || {
                crate::notify::permission_state(&availability) != "denied"
            }),
            badge: Arc::new(move |count| {
                if let Some(window) = badged.get_webview_window("main") {
                    let value = if count > 0 { Some(count) } else { None };
                    let _ = window.set_badge_count(value);
                }
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
    // The scheduler (P8.2): one service for snooze, reminder and queued-send
    // deadlines. It runs once immediately on startup, so a deadline that
    // expired while the app was closed is honoured at launch, and it then
    // sleeps until the nearest deadline with a bounded fallback.
    let scheduler_app = app.clone();
    tauri::async_runtime::spawn(async move {
        crate::scheduler::run(scheduler_app).await;
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
    // Fictional demo accounts have no credentials or remote mailbox. In
    // particular, constructing their IDLE provider would falsely request reauth.
    if crate::demo::is_demo() {
        return;
    }
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
        spawn_loop!(rules_loop),
    ];
    for handle in handles {
        state.register_task(account_id, generation, handle).await;
    }
}

/// A provider for the account, or `None` when the generation was cancelled
/// while it was being built.
async fn provider(
    state: &AppState,
    account_id: &str,
    cancel: &CancellationToken,
) -> Option<Arc<dyn Provider>> {
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
        // While the account is offline (or awaiting reauth) repeated network
        // work pauses; the resume path and the user's Retry run a tick
        // directly instead of waiting for this cadence (P4.6).
        if !crate::demo::is_demo() && !state.network_paused(account_id) {
            if let Some(provider) = provider(state, account_id, cancel).await {
                let coordinator = state.coordinator_for(account_id).await;
                coordinator
                    .tick_now(|| {
                        partial_tick(state, account_id, generation, cancel, &*provider, host)
                    })
                    .await;
                // Remote drafts that were created or edited elsewhere become
                // editable local drafts (P5.2). Best effort: an unreachable
                // server must not stop the mailbox from syncing, and an
                // unsynced message is retried on the next tick.
                if state.is_current(account_id, generation).await {
                    match crate::outbox::sync_remote_drafts(&state.db, &*provider, account_id).await
                    {
                        Ok(report) if report != Default::default() => {
                            log::info!(
                                "draft reconcile for {account_id}: {} imported, {} adopted, {} recovered, {} stranded",
                                report.imported.len(),
                                report.adopted.len(),
                                report.recovered.len(),
                                report.stranded.len()
                            );
                        }
                        Ok(_) => {}
                        Err(e) => log::debug!("draft reconcile for {account_id} skipped: {e}"),
                    }
                }
            }
        }
        if !sleep_or_cancel(cancel, wait).await {
            return;
        }
    }
}

/// Outbox drain loop.
///
/// It no longer relies on the poll interval alone (P6.6): a nudge arrives the
/// moment something is queued or the network comes back, and the interval
/// remains as the backstop for time-based work (a send's undo deadline, an
/// uncertain send's reconciliation schedule).
async fn drain_loop(
    state: &AppState,
    account_id: &str,
    generation: u64,
    cancel: &CancellationToken,
    host: &RuntimeHost,
) {
    loop {
        let wake = state.outbox_wake(account_id).await;
        let mut worked = false;
        if !crate::demo::is_demo() && !state.network_paused(account_id) {
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
                        // Drain queued gestures before a potentially large refresh.
                        // The periodic sync still runs while the queue is busy.
                        if state.db.outbox_pending_count(account_id).await.unwrap_or(0) == 0 {
                            let coordinator = state.coordinator_for(account_id).await;
                            coordinator
                                .tick_now(|| {
                                    refresh_after_outbox(
                                        state, account_id, generation, cancel, &*provider, host,
                                    )
                                })
                                .await;
                        }
                    }
                    Ok(false) => {}
                    Err(e) => {
                        // Failed ops revert + toast at the action layer via
                        // outbox:state; connectivity records the evidence.
                        state.record_provider_error(account_id, &e);
                    }
                }
            }
        }
        if let Ok(n) = state.db.outbox_pending_count(account_id).await {
            let failed = state.db.outbox_failed_count(account_id).await.unwrap_or(0);
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
                serde_json::json!({"account_id": account_id, "pending": n, "failed": failed, "summary": summary}),
            );
        }
        // Sleep until this account's nearest persisted deadline (P8.1). The
        // deadline is the schedule, so a message queued for 08:00 leaves at
        // 08:00 rather than when a poll interval happens to fall; a nudge from
        // `kick_outbox` still cuts the sleep short.
        let wait_ms = if worked {
            1_000
        } else {
            let deadline = state
                .db
                .outbox_next_deadline(Some(account_id.to_string()))
                .await
                .ok()
                .flatten();
            match deadline {
                Some(when) => (when - crate::db::now_ms()).clamp(1_000, 60_000),
                None => 60_000,
            }
        };
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = wake.notified() => {}
            _ = tokio::time::sleep(Duration::from_millis(wait_ms as u64)) => {}
        }
    }
}

/// Post-outbox refresh: the same partial tick the poll loop runs, sharing the
/// account's coordinator so a mutation never races a full sync (P4.3).
async fn refresh_after_outbox(
    state: &AppState,
    account_id: &str,
    generation: u64,
    cancel: &CancellationToken,
    provider: &dyn crate::provider::Provider,
    host: &RuntimeHost,
) {
    partial_tick(state, account_id, generation, cancel, provider, host).await;
}

/// Rule evaluation loop (P8.3).
///
/// It runs *after* ingest, never inside it: the queue row is written with the
/// message, and this loop evaluates it in bounded batches that yield to
/// foreground work. Rules are off by default, so a mailbox without rules costs
/// one indexed query per tick and nothing else.
async fn rules_loop(
    state: &AppState,
    account_id: &str,
    _generation: u64,
    cancel: &CancellationToken,
    _host: &RuntimeHost,
) {
    loop {
        if !crate::demo::is_demo() {
            rules_tick(state, account_id).await;
        }
        if !sleep_or_cancel(cancel, 5).await {
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
        if !crate::demo::is_demo() && !state.network_paused(account_id) {
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
            if !crate::demo::is_demo() && !state.network_paused(account_id) {
                if let Some(provider) = provider(state, account_id, cancel).await {
                    let coordinator = state.coordinator_for(account_id).await;
                    coordinator
                        .tick_now(|| {
                            partial_tick(state, account_id, generation, cancel, &*provider, host)
                        })
                        .await;
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

/// One partial tick shared by the poll loop, the IDLE consumer and the
/// post-outbox refresh. Network results that land after the account's
/// generation ended are dropped, never written or emitted (P4.4). Every
/// outcome is also evidence for the account's connectivity state (P4.6).
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
            state.record_provider_ok(account_id);
            emit_store(host, account_id, &changed_threads).await;
            notify_new(&state.db, host, &new_inbox).await;
            // The same query the sidebar uses (P8.4): the OS badge and the UI
            // count are computed from one definition, so they cannot disagree.
            refresh_badge(state, host).await;
            // A snoozed thread that received an incoming message is live
            // again: wake it by the same policy as a due timer (P6.5).
            let touched: Vec<String> = changed_threads
                .iter()
                .filter(|(a, _)| a == account_id)
                .map(|(_, t)| t.clone())
                .collect();
            if let Ok(woken) =
                crate::snooze::wake_threads_with_new_mail(&state.db, account_id, &touched).await
            {
                for thread in woken {
                    state.kick_outbox(account_id).await;
                    (host.emit)(
                        "snooze:woke",
                        serde_json::json!({"account_id": account_id, "thread_ids": [thread]}),
                    );
                }
            }
        }
        Ok(PartialOutcome::NeedsFull) => {
            state.record_provider_ok(account_id);
            recover_full(state, account_id, generation, cancel, provider, host).await;
        }
        Err(e) => {
            state.record_provider_error(account_id, &e);
            (host.emit)(
                "sync:state",
                serde_json::json!({"account_id": account_id, "phase": "error", "done": 0, "total": 0, "total_known": false, "last_error": e.to_string()}),
            );
        }
    }
}

/// Resume every account's background work after the host reports the network
/// came back (P4.6).
///
/// One coordinator trigger per account, jittered so a fleet of accounts does
/// not stampede the provider at the same instant, and never a second tick when
/// one is already running (the coordinator coalesces it).
pub fn resume_after_reconnect(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let accounts: Vec<String> = app
            .state::<AppState>()
            .db
            .accounts_list()
            .await
            .map(|v| v.into_iter().map(|a| a.id).collect())
            .unwrap_or_default();
        for (i, account_id) in accounts.into_iter().enumerate() {
            let app = app.clone();
            tokio::spawn(async move {
                // Deterministic-to-the-millisecond jitter: 0-800 ms, spread by
                // account index, so recovery is immediate but not synchronized.
                let jitter_ms = 80 * i as u64 + (crate::db::now_ms() as u64 % 80);
                tokio::time::sleep(Duration::from_millis(jitter_ms)).await;
                resume_account(&app, &account_id).await;
            });
        }
    });
}

/// One account's recovery tick: the same partial tick the poll loop runs.
pub async fn resume_account(app: &AppHandle, account_id: &str) {
    let state = app.state::<AppState>();
    let Some((generation, cancel)) = state.runtime_state(account_id).await else {
        return;
    };
    if cancel.is_cancelled() {
        return;
    }
    let Some(provider) = provider(&state, account_id, &cancel).await else {
        return;
    };
    let host = RuntimeHost::for_app(app);
    // Reconnect is an explicit drain trigger (P6.6): queued work goes out
    // immediately instead of waiting for the next poll interval.
    state.kick_outbox(account_id).await;
    let coordinator = state.coordinator_for(account_id).await;
    coordinator
        .tick_now(|| partial_tick(&state, account_id, generation, &cancel, &*provider, &host))
        .await;
    state.emit_connectivity(account_id);
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
                    state.record_provider_ok(account_id);
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
                        serde_json::json!({"account_id": account_id, "phase": "done", "done": 0, "total": 0, "total_known": false, "last_error": null}),
                    );
                }
                Err(e) => {
                    state.record_provider_error(account_id, &e);
                    (host.emit)(
                        "sync:state",
                        serde_json::json!({"account_id": account_id, "phase": "error", "done": 0, "total": 0, "total_known": false, "last_error": e.to_string()}),
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

/// Announce newly arrived Inbox mail (P8.4).
///
/// Rust raises the notification; the frontend only receives the state event, so
/// there is exactly one banner per message and no second sender to duplicate
/// it. Delivery is recorded durably before anything is shown, so a replayed
/// sync or a restart stays quiet.
async fn notify_new(db: &Db, host: &RuntimeHost, items: &[crate::provider::NewMail]) {
    if items.is_empty() {
        return;
    }
    let settings = db.settings_get().await.unwrap_or_default();
    let policy = crate::notify::Policy::from_settings(&settings);
    let mut notices: Vec<crate::notify::Notice> = Vec::with_capacity(items.len());
    for item in items {
        // The filter decides on the state Sift stored, not on the transport's
        // summary: the same Inbox membership and sender the list shows.
        let (from_email, in_inbox) = db
            .notify_context(&item.account_id, &item.message_id)
            .await
            .unwrap_or_default();
        let is_vip = if policy.account_is_muted(&item.account_id) {
            false
        } else {
            db.vip_is_vip(&item.account_id, &from_email)
                .await
                .unwrap_or(false)
        };
        notices.push(crate::notify::Notice {
            account_id: item.account_id.clone(),
            thread_id: item.thread_id.clone(),
            message_id: item.message_id.clone(),
            from: item.from.clone(),
            subject: item.subject.clone(),
            in_inbox,
            is_vip,
        });
    }
    let results = crate::notify::deliver_new_mail(db, host, &notices).await;
    for (notice, delivery) in notices.iter().zip(results.iter()) {
        if *delivery != crate::notify::Delivery::Suppressed {
            (host.emit)(
                "notify:new-mail",
                serde_json::json!({
                    "accountId": notice.account_id,
                    "threadId": notice.thread_id,
                    "messageId": notice.message_id,
                    "from": notice.from,
                    "subject": notice.subject,
                    "hidden": policy.hide_subject,
                    "shown": *delivery == crate::notify::Delivery::Shown,
                }),
            );
        }
    }
}

/// Set the dock badge from the same query the sidebar's Inbox count uses
/// (P8.4), so the OS number and the UI number cannot disagree. `dock_badge`
/// of `off` clears it.
pub async fn refresh_badge(state: &AppState, host: &RuntimeHost) {
    let settings = state.db.settings_get().await.unwrap_or_default();
    let count = if settings.dock_badge == "off" {
        0
    } else {
        let accounts: Vec<String> = state
            .db
            .accounts_list()
            .await
            .map(|v| v.into_iter().map(|a| a.id).collect())
            .unwrap_or_default();
        state
            .db
            .unread_inbox_count(&accounts, &[])
            .await
            .unwrap_or(0)
    };
    (host.badge)(count);
    (host.emit)("badge:update", serde_json::json!({ "count": count }));
}

/// Startup and post-commit entry point for the badge, from an app handle.
pub async fn refresh_badge_for_app(app: &AppHandle) {
    let state = app.state::<AppState>();
    let host = RuntimeHost::for_app(app);
    refresh_badge(&state, &host).await;
}

/// Drain newly ingested mail through the enabled rules (P8.3).
///
/// Runs outside the ingest path, in bounded batches, and yields to foreground
/// work: a first sync of a large mailbox cannot be slowed down by a rule, and a
/// rule can never fail the sync it follows.
async fn rules_tick(state: &AppState, account_id: &str) {
    let busy = state
        .foreground_inflight
        .load(std::sync::atomic::Ordering::Relaxed)
        > 0;
    match crate::rules::process_queue(&state.db, account_id, busy).await {
        Ok(report) if report.messages > 0 => {
            log::debug!(
                "rules for {account_id}: {} messages, {} applied, yielded={}",
                report.messages,
                report.applied,
                report.yielded
            );
            if report.applied > 0 {
                state.kick_outbox(account_id).await;
            }
        }
        Ok(_) => {}
        Err(e) => log::warn!("rules for {account_id}: {e}"),
    }
}

pub(crate) async fn check_snoozes(app: &AppHandle) -> anyhow::Result<usize> {
    let state = app.state::<AppState>();
    let host = RuntimeHost::for_app(app);
    let now = crate::db::now_ms();
    // The timer removal, the label change and the queued operation are one
    // transaction per account, so a wake can never be half-applied (P6.5).
    let due = crate::snooze::wake_due(&state.db, now)
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let mut live: Vec<(String, String)> = Vec::with_capacity(due.len());
    for (aid, tid) in due {
        // A removed account must not get new rows or events from a wake-up.
        if state.is_cancelled(&aid).await {
            continue;
        }
        live.push((aid, tid));
    }
    for (aid, _) in &live {
        state.kick_outbox(aid).await;
    }
    let woken = live.len();
    if !live.is_empty() {
        let mut by_account: HashMap<String, Vec<String>> = HashMap::new();
        for (aid, tid) in &live {
            by_account.entry(aid.clone()).or_default().push(tid.clone());
        }
        for (aid, tids) in by_account {
            emit_store(
                &host,
                &aid,
                &tids
                    .iter()
                    .map(|t| (aid.clone(), t.clone()))
                    .collect::<Vec<_>>(),
            )
            .await;
            (host.emit)(
                "snooze:woke",
                serde_json::json!({"account_id": aid, "thread_ids": tids}),
            );
        }
    }
    Ok(woken)
}

/// Weekly maintenance (P6.6): finished operations lose their payload after
/// seven days, sent drafts lose their recovery copy after the same window.
/// Pending, failed, uncertain and scheduled work is never touched.
pub async fn prune_finished_work(app: &AppHandle) {
    let state = app.state::<AppState>();
    let files = state
        .db
        .outbox_prune(crate::db::outbox::OP_PAYLOAD_RETENTION_MS)
        .await
        .unwrap_or_default();
    for path in files {
        let _ = tokio::fs::remove_file(path).await;
    }
    let pruned = state
        .db
        .drafts_prune_sent(crate::db::outbox::OP_PAYLOAD_RETENTION_MS)
        .await
        .unwrap_or_default();
    for local_id in pruned {
        let dir = crate::outgoing::draft_send_dir(&state.data_dir, &local_id);
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dto::Label;
    use crate::errors::SiftError;
    use crate::provider::{
        ApplyOutcome, BoxStream, Cursor, OutboxOp, PartialOutcome, ProfileInfo, ProviderKind,
        SendAs, SentInfo, SyncSink, ThreadRef, WatchEvent,
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
            let (db, account_id, kind) =
                (self.db.clone(), self.account_id.clone(), kind.to_string());
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
        async fn rename_label(&self, _id: &str, _name: &str) -> Result<Label, SiftError> {
            Err(unused())
        }
        async fn delete_label(&self, _id: &str) -> Result<(), SiftError> {
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
        async fn fetch_raw_bytes(&self, _message_id: &str) -> Result<Vec<u8>, SiftError> {
            Err(unused())
        }
        async fn apply(&self, _op: &OutboxOp) -> Result<ApplyOutcome, SiftError> {
            Err(unused())
        }
        /// The drain hands the prepared delivery to the transport: that call is
        /// what the removal tests observe.
        async fn send(&self, _req: &crate::provider::SendRequest) -> Result<SentInfo, SiftError> {
            self.mark("send-entered").await;
            self.block_until_released().await;
            self.mark("send-late").await;
            Ok(SentInfo {
                id: "sent".into(),
                thread_id: "t1".into(),
            })
        }
        async fn draft_upsert(
            &self,
            _remote_id: Option<&str>,
            _raw: &[u8],
            _rfc_message_id: &str,
        ) -> Result<crate::dto::RemoteDraft, SiftError> {
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
            // A test host always "shows" the notification: the callbacks that
            // matter here are the state transitions, not the platform banner.
            notify_os: Arc::new(|_| true),
            focused: Arc::new(|| true),
            notifications_available: Arc::new(|| true),
            badge: Arc::new(|_| {}),
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
        // A prepared send: the drain reads the frozen MIME from disk and hands
        // the envelope to the transport.
        let raw_path = dir.path().join("prepared.eml");
        let raw = b"From: outbox@x.com\r\nTo: someone@y.com\r\n\r\nbody";
        std::fs::write(&raw_path, raw).unwrap();
        let payload = serde_json::json!({
            "localId": "d1",
            "revision": 1,
            "rawPath": raw_path.to_string_lossy(),
            "rawSize": raw.len() as i64,
            "from": "outbox@x.com",
            "envelopeRecipients": ["someone@y.com"],
            "bccRecipients": [],
            "rfcMessageId": "<prepared@x.com>",
        })
        .to_string();
        state
            .db
            .outbox_enqueue(&account.id, "send", &payload, None, 0)
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
