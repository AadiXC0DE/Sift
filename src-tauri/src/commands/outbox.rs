//! Triage commands: one gesture in, one durable operation group out (P6.3),
//! plus the compact Outbox surface the sidebar and panel read (P6.6).
//!
//! Every command here follows the same shape: the service commits, and only
//! then does the command emit store events. A UI that reacted to an event
//! that preceded the commit could show a state the database does not have.

use crate::actions;
use crate::app_state::AppState;
use crate::db::outbox::OutboxList;
use crate::dto::{GestureResponse, OutboxOpSummary, OutboxPage};
use crate::errors::SiftError;
use tauri::{AppHandle, Emitter, State};

fn db_error(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

fn resolve_gesture_id(requested: Option<String>) -> String {
    requested
        .filter(|g| !g.trim().is_empty())
        .unwrap_or_else(|| uuid::Uuid::now_v7().to_string())
}

fn summarize(ops: &[crate::db::outbox::Op]) -> Vec<OutboxOpSummary> {
    ops.iter().map(OutboxOpSummary::from).collect()
}

/// Account-scoped store events for the threads a gesture touched.
fn emit_gesture(app: &AppHandle, thread_ids: &[String], account_ids: &[String]) {
    for account_id in account_ids {
        let scoped: Vec<String> = thread_ids.to_vec();
        let _ = app.emit(
            "store:threads",
            serde_json::json!({ "account_id": account_id, "thread_ids": scoped }),
        );
        let _ = app.emit(
            "store:labels",
            serde_json::json!({ "account_id": account_id }),
        );
    }
}

async fn emit_outbox_state(state: &AppState, app: &AppHandle, account_id: &str) {
    let counts = state
        .db
        .outbox_state_counts(account_id)
        .await
        .unwrap_or_default();
    let summary = state
        .db
        .outbox_summary(account_id)
        .await
        .unwrap_or_default();
    let _ = app.emit(
        "outbox:state",
        serde_json::json!({
            "account_id": account_id,
            "pending": counts.pending + counts.inflight,
            "inflight": counts.inflight,
            "failed": counts.failed,
            "uncertain": counts.uncertain,
            "summary": summary
                .into_iter()
                .map(|(label, count)| serde_json::json!({"label": label, "count": count}))
                .collect::<Vec<_>>(),
        }),
    );
}

/// One UI gesture over account-qualified targets.
#[tauri::command]
pub async fn threads_action(
    app: AppHandle,
    state: State<'_, AppState>,
    gesture_id: Option<String>,
    targets: Vec<crate::dto::GestureTarget>,
    action: crate::dto::ActionKind,
) -> Result<GestureResponse, SiftError> {
    let gesture = resolve_gesture_id(gesture_id);
    let (outcome, failures) =
        actions::apply_gesture(&state.db, &gesture, &targets, &action).await?;
    let account_ids: Vec<String> = targets.iter().map(|t| t.account_id.clone()).collect();
    emit_gesture(&app, &outcome.thread_ids, &account_ids);
    for account_id in account_ids.iter() {
        state.kick_outbox(account_id).await;
        emit_outbox_state(&state, &app, account_id).await;
    }
    Ok(GestureResponse {
        gesture_id: gesture,
        operations: summarize(&outcome.operations),
        failures,
    })
}

/// Undo one gesture, across every account it touched.
#[tauri::command]
pub async fn action_undo(
    app: AppHandle,
    state: State<'_, AppState>,
    gesture_id: String,
) -> Result<GestureResponse, SiftError> {
    let (operations, failures) = actions::undo_gesture(&state.db, &gesture_id).await?;
    let mut account_ids: Vec<String> = operations.iter().map(|o| o.account_id.clone()).collect();
    for failure in &failures {
        account_ids.push(failure.account_id.clone());
    }
    account_ids.sort();
    account_ids.dedup();
    let thread_ids: Vec<String> = vec![];
    emit_gesture(&app, &thread_ids, &account_ids);
    for account_id in account_ids.iter() {
        state.kick_outbox(account_id).await;
        emit_outbox_state(&state, &app, account_id).await;
    }
    Ok(GestureResponse {
        gesture_id,
        operations: summarize(&operations),
        failures,
    })
}

/// Snooze: local scheduling with an optional matching remote label (P6.5).
#[tauri::command]
pub async fn snooze_set(
    app: AppHandle,
    state: State<'_, AppState>,
    gesture_id: Option<String>,
    targets: Vec<crate::dto::GestureTarget>,
    wake_at: i64,
    wake_unread: Option<bool>,
) -> Result<GestureResponse, SiftError> {
    let gesture = resolve_gesture_id(gesture_id);
    let (outcome, failures) = crate::snooze::snooze_set(
        &state.db,
        &gesture,
        &targets,
        wake_at,
        wake_unread.unwrap_or(false),
    )
    .await?;
    let account_ids: Vec<String> = targets.iter().map(|t| t.account_id.clone()).collect();
    emit_gesture(&app, &outcome.thread_ids, &account_ids);
    for account_id in account_ids.iter() {
        state.kick_outbox(account_id).await;
        emit_outbox_state(&state, &app, account_id).await;
    }
    Ok(GestureResponse {
        gesture_id: gesture,
        operations: summarize(&outcome.operations),
        failures,
    })
}

/// Unsnooze: the timer goes away *and* the saved Inbox policy comes back.
#[tauri::command]
pub async fn snooze_clear(
    app: AppHandle,
    state: State<'_, AppState>,
    gesture_id: Option<String>,
    targets: Vec<crate::dto::GestureTarget>,
) -> Result<GestureResponse, SiftError> {
    let gesture = resolve_gesture_id(gesture_id);
    let (outcome, failures) = crate::snooze::snooze_clear(&state.db, &gesture, &targets).await?;
    let account_ids: Vec<String> = targets.iter().map(|t| t.account_id.clone()).collect();
    emit_gesture(&app, &outcome.thread_ids, &account_ids);
    for account_id in account_ids.iter() {
        state.kick_outbox(account_id).await;
        emit_outbox_state(&state, &app, account_id).await;
    }
    Ok(GestureResponse {
        gesture_id: gesture,
        operations: summarize(&outcome.operations),
        failures,
    })
}

/// The compact Outbox page (P6.6): lightweight summaries, never a payload.
#[tauri::command]
pub async fn outbox_list(
    state: State<'_, AppState>,
    account_ids: Vec<String>,
    states: Option<Vec<String>>,
    cursor: Option<String>,
    limit: Option<i64>,
) -> Result<OutboxPage, SiftError> {
    let states = states.unwrap_or_default();
    let cursor = cursor.and_then(|c| c.parse::<i64>().ok());
    let OutboxList {
        operations,
        total,
        counts,
    } = state
        .db
        .outbox_list(&account_ids, &states, cursor, limit.unwrap_or(50))
        .await
        .map_err(db_error)?;
    let next_cursor = if (operations.len() as i64) < limit.unwrap_or(50).clamp(1, 200) {
        None
    } else {
        operations.last().map(|op| op.id.to_string())
    };
    Ok(OutboxPage {
        operations: operations.iter().map(OutboxOpSummary::from).collect(),
        next_cursor,
        total,
        counts,
    })
}

#[tauri::command]
pub async fn outbox_get(
    state: State<'_, AppState>,
    op_id: i64,
) -> Result<OutboxOpSummary, SiftError> {
    let op = state
        .db
        .outbox_get(op_id)
        .await
        .map_err(db_error)?
        .ok_or_else(|| SiftError::NotFound(format!("operation {op_id}")))?;
    Ok(OutboxOpSummary::from(&op))
}

/// Retry an operation the user asked for.
///
/// A `failed` operation goes back to `pending` with its identity intact. An
/// `uncertain` send only moves when the user acknowledges that Sift cannot
/// prove it was not delivered (P6.1) — the caller renders that choice.
#[tauri::command]
pub async fn outbox_retry(
    app: AppHandle,
    state: State<'_, AppState>,
    op_id: i64,
    acknowledge_duplicate_risk: Option<bool>,
) -> Result<OutboxOpSummary, SiftError> {
    let op = state
        .db
        .outbox_retry(op_id, acknowledge_duplicate_risk.unwrap_or(false))
        .await
        .map_err(unwrap_sift)?;
    let account_id = op.account_id.clone();
    state.kick_outbox(&account_id).await;
    emit_outbox_state(&state, &app, &account_id).await;
    Ok(OutboxOpSummary::from(&op))
}

/// A handled error keeps its code and detail; anything else is a database
/// failure, which is what it is.
fn unwrap_sift(e: anyhow::Error) -> SiftError {
    match e.downcast::<SiftError>() {
        Ok(err) => err,
        Err(e) => SiftError::app("db", e.to_string(), false),
    }
}
