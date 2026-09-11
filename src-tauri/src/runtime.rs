use crate::app_state::AppState;
use crate::db::Db;
use tauri::{AppHandle, Emitter, Manager};

/// Background supervisor: per-account poll/drain/backfill loops plus a global
/// snooze watcher. This is what makes Sift live: without it, `sync_now` is a
/// manual one-shot and outbox ops never leave the queue.
pub fn spawn_supervisor(app: AppHandle) {
    // Snooze watcher (global, cheap).
    let app2 = app.clone();
    tauri::async_runtime::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            if let Err(e) = check_snoozes(&app2).await {
                eprintln!("snooze watcher: {e}");
            }
        }
    });

    // Per-account loops, spawned once accounts exist (and re-armed when the
    // account list changes, guarded by a watch generation).
    tauri::async_runtime::spawn(async move {
        let mut known: Vec<String> = vec![];
        loop {
            let accounts: Vec<String> = app
                .state::<AppState>()
                .db
                .accounts_list()
                .await
                .map(|v| v.into_iter().map(|a| a.id).collect())
                .unwrap_or_default();
            for aid in &accounts {
                if !known.contains(aid) {
                    known.push(aid.clone());
                    spawn_account_loops(app.clone(), aid.clone());
                }
            }
            known.retain(|id| accounts.contains(id));
            tokio::time::sleep(std::time::Duration::from_secs(10)).await;
        }
    });
}

fn spawn_account_loops(app: AppHandle, account_id: String) {
    // Poll loop.
    let (app2, aid) = (app.clone(), account_id.clone());
    tauri::async_runtime::spawn(async move {
        loop {
            let state = app2.state::<AppState>();
            let settings = state.db.settings_get().await.unwrap_or_default();
            // Focused/background distinction comes from the window; poll at the
            // focused cadence when any window is focused, else background.
            let focused = app2
                .webview_windows()
                .values()
                .any(|w| w.is_focused().unwrap_or(false));
            let wait = if focused {
                settings.poll_focused.max(5) as u64
            } else {
                settings.poll_background.max(15) as u64
            };
            if state.is_online() && !crate::demo::is_demo() {
                if let Ok(provider) = state.provider_for(&aid).await {
                    partial_tick(&app2, &state.db, &aid, &*provider).await;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        }
    });

    // Outbox drain loop.
    let (app3, aid) = (app.clone(), account_id.clone());
    tauri::async_runtime::spawn(async move {
        loop {
            let state = app3.state::<AppState>();
            let online = state.is_online();
            let mut worked = false;
            if online && !crate::demo::is_demo() {
                if let Ok(provider) = state.provider_for(&aid).await {
                    match crate::outbox::drain_one(&state.db, &*provider, &aid, true).await {
                        Ok(true) => {
                            worked = true;
                            // Fresh server state after every mutation.
                            let cursor = read_cursor(&state.db, &aid).await;
                            if let Ok(crate::provider::PartialOutcome::Synced {
                                changed_threads,
                                ..
                            }) = provider.partial_sync(&cursor, &db_sink(&state.db)).await
                            {
                                emit_store(&app3, &aid, &changed_threads).await;
                            }
                        }
                        Ok(false) => {}
                        Err(_) => {
                            // Failed ops revert + toast at the action layer via outbox:state.
                        }
                    }
                }
            }
            if let Ok(n) = state.db.outbox_pending_count(&aid).await {
                let summary = state
                    .db
                    .outbox_summary(&aid)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(label, count)| serde_json::json!({ "label": label, "count": count }))
                    .collect::<Vec<_>>();
                let _ = app3.emit(
                    "outbox:state",
                    serde_json::json!({"account_id": aid, "pending": n, "failed": 0, "summary": summary}),
                );
            }
            tokio::time::sleep(std::time::Duration::from_secs(if worked { 1 } else { 5 })).await;
        }
    });

    // Body backfill loop (low priority, yields to foreground fetches).
    let (app4, aid4) = (app.clone(), account_id.clone());
    tauri::async_runtime::spawn(async move {
        loop {
            let state = app4.state::<AppState>();
            if state.is_online() && !crate::demo::is_demo() {
                if let Ok(provider) = state.provider_for(&aid4).await {
                    let settings = state.db.settings_get().await.unwrap_or_default();
                    let horizon_days = match settings.offline_body_cache.as_str() {
                        "6m" => 180,
                        "all" => 365 * 30,
                        _ => 730,
                    };
                    let sink = db_sink(&state.db);
                    let _ = crate::sync::backfill::run_backfill_once(
                        &sink,
                        &aid4,
                        &*provider,
                        &state.gate,
                        horizon_days,
                    )
                    .await;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        }
    });

    // IDLE push consumer (IMAP only; `watch()` is None for REST).
    // Debounces 1 s, then runs the same partial tick as the poll loop.
    let (app5, aid) = (app.clone(), account_id.clone());
    tauri::async_runtime::spawn(async move {
        use futures::StreamExt;
        loop {
            let stream = {
                let state = app5.state::<AppState>();
                match state.provider_for(&aid).await {
                    Ok(p) => p.watch(),
                    Err(_) => None,
                }
            };
            let Some(mut stream) = stream else {
                // REST accounts (and unknown): the poll loop covers them.
                return;
            };
            while let Some(_evt) = stream.next().await {
                // Debounce: collapse IDLE flurries, drain anything queued.
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                while tokio::time::timeout(std::time::Duration::from_millis(200), stream.next())
                    .await
                    .is_ok_and(|v| v.is_some())
                {}
                let state = app5.state::<AppState>();
                if state.is_online() && !crate::demo::is_demo() {
                    if let Ok(provider) = state.provider_for(&aid).await {
                        partial_tick(&app5, &state.db, &aid, &*provider).await;
                    }
                }
            }
            // Stream ended (IDLE error path): polling covered the gap;
            // re-arm shortly (IDLE task itself also retries internally).
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
    });
}

/// One partial tick shared by the poll loop and the IDLE consumer.
async fn partial_tick(
    app: &AppHandle,
    db: &Db,
    aid: &str,
    provider: &dyn crate::provider::Provider,
) {
    use crate::provider::PartialOutcome;
    let cursor = read_cursor(db, aid).await;
    match provider.partial_sync(&cursor, &db_sink(db)).await {
        Ok(PartialOutcome::Synced {
            changed_threads,
            new_inbox,
        }) => {
            emit_store(app, aid, &changed_threads).await;
            notify_new(app, db, &new_inbox).await;
        }
        Ok(PartialOutcome::NeedsFull) => {
            recover_full(app, db, aid, provider).await;
        }
        Err(e) => {
            let _ = app.emit(
                "sync:state",
                serde_json::json!({"account_id": aid, "phase": "error", "done": 0, "total": 0, "last_error": e.to_string()}),
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
    app: &AppHandle,
    db: &Db,
    account_id: &str,
    provider: &dyn crate::provider::Provider,
) {
    use crate::provider::{Cursor, ProviderKind};
    let sink = db_sink(db);
    match provider.kind() {
        ProviderKind::GmailApi => {
            // History lost mid-tick (rare: provider already reconciles once
            // internally). Reconcile again defensively; the next tick continues.
            let cursor = Cursor::Gmail {
                history_id: String::new(),
            };
            if provider.partial_sync(&cursor, &sink).await.is_ok() {
                let _ = app.emit(
                    "store:labels",
                    serde_json::json!({ "account_id": account_id }),
                );
            }
        }
        ProviderKind::GmailImap => {
            // UIDVALIDITY changed: folder state is garbage. full_sync rebuilds
            // each folder idempotently, then persists the fresh cursor.
            let cancel = tokio_util::sync::CancellationToken::new();
            match provider.full_sync(&sink, cancel).await {
                Ok(cursor) => {
                    let _ = db
                        .accounts_set_history(account_id, &cursor.render(), crate::db::now_ms())
                        .await;
                    let _ = app.emit(
                        "store:labels",
                        serde_json::json!({ "account_id": account_id }),
                    );
                    let _ = app.emit(
                        "sync:state",
                        serde_json::json!({"account_id": account_id, "phase": "done", "done": 0, "total": 0, "last_error": null}),
                    );
                }
                Err(e) => {
                    let _ = app.emit(
                        "sync:state",
                        serde_json::json!({"account_id": account_id, "phase": "error", "done": 0, "total": 0, "last_error": e.to_string()}),
                    );
                }
            }
        }
    }
}

async fn emit_store(app: &AppHandle, account_id: &str, changed: &[(String, String)]) {
    let thread_ids: Vec<String> = changed
        .iter()
        .filter(|(a, _)| a == account_id)
        .map(|(_, t)| t.clone())
        .collect();
    if !thread_ids.is_empty() {
        let _ = app.emit(
            "store:threads",
            serde_json::json!({"account_id": account_id, "thread_ids": thread_ids}),
        );
    }
    let _ = app.emit(
        "store:labels",
        serde_json::json!({"account_id": account_id}),
    );
}

async fn notify_new(app: &AppHandle, db: &Db, items: &[(String, String, String, String)]) {
    if items.is_empty() {
        return;
    }
    let settings = db.settings_get().await.unwrap_or_default();
    if settings.notifications == "off" {
        return;
    }
    for (aid, tid, from, subject) in items.iter().take(3) {
        let _ = app.emit(
            "notify:new-mail",
            serde_json::json!({"account_id": aid, "thread_id": tid, "from": from, "subject": subject}),
        );
        #[cfg(not(test))]
        {
            use tauri_plugin_notification::NotificationExt;
            let _ = app
                .notification()
                .builder()
                .title(from.clone())
                .body(subject.clone())
                .show();
        }
    }
    if items.len() > 3 {
        let _ = app.emit(
            "notify:new-mail",
            serde_json::json!({"account_id": items[0].0, "thread_id": items[0].1, "from": "Sift", "subject": format!("{} new messages", items.len())}),
        );
    }
}

async fn check_snoozes(app: &AppHandle) -> anyhow::Result<()> {
    let state = app.state::<AppState>();
    let now = crate::db::now_ms();
    let settings = state.db.settings_get().await.unwrap_or_default();
    let due = crate::scheduler::process_overdue(&state.db, now).await?;
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
            let _ = state.db.apply_label_change(mid, &add, &[]).await;
        }
        let payload = serde_json::json!({"ids": mids, "add": ["INBOX"], "remove": []}).to_string();
        let _ = state
            .db
            .outbox_enqueue(aid, "modify_labels", &payload, None, 0)
            .await;
        emit_store(app, aid, &[(aid.clone(), tid.clone())]).await;
    }
    if !due.is_empty() {
        let tids: Vec<String> = due.iter().map(|(_, t)| t.clone()).collect();
        let _ = app.emit(
            "snooze:woke",
            serde_json::json!({"account_id": due[0].0, "thread_ids": tids}),
        );
    }
    Ok(())
}
