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
                if let Ok(client) = state.client_for(&aid).await {
                    match crate::sync::partial::run_partial_sync(&state.db, &aid, &client).await {
                        Ok(out) => {
                            emit_store(&app2, &aid, &out.changed_threads).await;
                            notify_new(&app2, &state.db, &out.new_inbox).await;
                        }
                        Err(e) => {
                            let _ = app2.emit(
                                "sync:state",
                                serde_json::json!({"account_id": aid, "phase": "error", "done": 0, "total": 0, "last_error": e.to_string()}),
                            );
                        }
                    }
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
                if let Ok(client) = state.client_for(&aid).await {
                    match crate::outbox::drain_one(&state.db, &client, &aid, true).await {
                        Ok(true) => {
                            worked = true;
                            // Fresh server state after every mutation.
                            if let Ok(out) =
                                crate::sync::partial::run_partial_sync(&state.db, &aid, &client)
                                    .await
                            {
                                emit_store(&app3, &aid, &out.changed_threads).await;
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
                let _ = app3.emit(
                    "outbox:state",
                    serde_json::json!({"account_id": aid, "pending": n, "failed": 0}),
                );
            }
            tokio::time::sleep(std::time::Duration::from_secs(if worked { 1 } else { 5 })).await;
        }
    });

    // Body backfill loop (low priority, yields to foreground fetches).
    let (app4, aid) = (app, account_id);
    tauri::async_runtime::spawn(async move {
        loop {
            let state = app4.state::<AppState>();
            if state.is_online() && !crate::demo::is_demo() {
                if let Ok(client) = state.client_for(&aid).await {
                    let settings = state.db.settings_get().await.unwrap_or_default();
                    let horizon_days = match settings.offline_body_cache.as_str() {
                        "6m" => 180,
                        "all" => 365 * 30,
                        _ => 730,
                    };
                    let _ = crate::sync::backfill::run_backfill_once(
                        &state.db,
                        &aid,
                        &client,
                        &state.gate,
                        horizon_days,
                    )
                    .await;
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(20)).await;
        }
    });
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
