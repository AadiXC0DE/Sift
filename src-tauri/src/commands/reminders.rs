//! Reminder commands (P8.2).
//!
//! "Remind me…" is under the message More menu. It leaves the thread exactly
//! where it is: no archive, no read state, no label, no provider operation.
//! What the commands return is the reminder list, so the UI shows the state the
//! database holds rather than the state it hoped for.

use crate::app_state::AppState;
use crate::db::reminders::ReminderRow;
use crate::dto::GestureTarget;
use crate::errors::SiftError;
use tauri::{AppHandle, Emitter, State};

fn db_error(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

/// Every account's reminder list is refreshed, because one gesture can span
/// accounts and the compact Reminders view is account-agnostic.
fn emit_reminder_state(app: &AppHandle, rows: &[ReminderRow]) {
    let mut accounts: Vec<String> = rows.iter().map(|r| r.account_id.clone()).collect();
    accounts.sort();
    accounts.dedup();
    for account_id in accounts {
        let _ = app.emit(
            "store:threads",
            serde_json::json!({
                "account_id": account_id,
                "thread_ids": rows
                    .iter()
                    .filter(|r| r.account_id == account_id)
                    .map(|r| r.thread_id.clone())
                    .collect::<Vec<_>>(),
            }),
        );
    }
    let _ = app.emit(
        "reminders:state",
        serde_json::json!({ "open": rows.iter().filter(|r| r.completed_at.is_none()).count() }),
    );
}

#[tauri::command]
pub async fn reminder_set(
    app: AppHandle,
    state: State<'_, AppState>,
    targets: Vec<GestureTarget>,
    remind_at: i64,
) -> Result<Vec<ReminderRow>, SiftError> {
    let rows = crate::reminders::set(&state.db, &targets, remind_at).await?;
    emit_reminder_state(&app, &rows);
    Ok(rows)
}

#[tauri::command]
pub async fn reminder_clear(
    app: AppHandle,
    state: State<'_, AppState>,
    targets: Vec<GestureTarget>,
) -> Result<(), SiftError> {
    crate::reminders::clear(&state.db, &targets).await?;
    emit_touched(&app, &state, &targets).await;
    Ok(())
}

/// Mark a due reminder done. The row is kept (completed), so a restart or a
/// restore from Trash cannot make it fire again.
#[tauri::command]
pub async fn reminder_complete(
    app: AppHandle,
    state: State<'_, AppState>,
    targets: Vec<GestureTarget>,
) -> Result<(), SiftError> {
    crate::reminders::complete(&state.db, &targets).await?;
    emit_touched(&app, &state, &targets).await;
    Ok(())
}

/// Tell the UI which threads changed, using the reminder state actually stored.
async fn emit_touched(app: &AppHandle, state: &AppState, targets: &[GestureTarget]) {
    let mut accounts: Vec<String> = targets.iter().map(|t| t.account_id.clone()).collect();
    accounts.sort();
    accounts.dedup();
    for account_id in accounts {
        let rows = state
            .db
            .reminders_list(std::slice::from_ref(&account_id), true)
            .await
            .unwrap_or_default();
        let thread_ids: Vec<String> = targets
            .iter()
            .filter(|t| t.account_id == account_id)
            .map(|t| t.thread_id.clone())
            .collect();
        let _ = app.emit(
            "store:threads",
            serde_json::json!({ "account_id": account_id, "thread_ids": thread_ids }),
        );
        let _ = app.emit(
            "reminders:state",
            serde_json::json!({
                "open": rows.iter().filter(|r| r.completed_at.is_none()).count(),
            }),
        );
    }
}

#[tauri::command]
pub async fn reminders_list(
    state: State<'_, AppState>,
    account_ids: Vec<String>,
    include_completed: Option<bool>,
) -> Result<Vec<ReminderRow>, SiftError> {
    crate::reminders::list(&state.db, &account_ids, include_completed.unwrap_or(false)).await
}

/// How many open reminders exist. The compact Reminders saved view is only
/// offered when this is non-zero — a nav item that leads nowhere is worse than
/// no nav item.
#[tauri::command]
pub async fn reminders_open_count(
    state: State<'_, AppState>,
    account_ids: Vec<String>,
) -> Result<i64, SiftError> {
    state
        .db
        .reminders_open_count(&account_ids)
        .await
        .map_err(db_error)
}

