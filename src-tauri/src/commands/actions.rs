//! Label creation and the legacy single-account entry points.
//!
//! The gesture path (P6.3) lives in [`crate::commands::outbox`]: it takes
//! account-qualified targets and one gesture id, so a mixed-account selection
//! is a single undoable group. What remains here is the label service, which
//! the picker and the sidebar share.

use crate::app_state::AppState;
use crate::errors::SiftError;
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
pub async fn labels_create(
    state: State<'_, AppState>,
    account_id: String,
    name: String,
    color: Option<String>,
) -> Result<crate::dto::Label, SiftError> {
    let provider = state.provider_for(&account_id).await?;
    let mut l = provider.create_label(&name).await?;
    l.color_bg = color.clone();
    state
        .db
        .labels_upsert(&l)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    Ok(l)
}

#[cfg(test)]
mod tests {

    /// The star gesture must touch the newest message only: starring a
    /// conversation in Gmail sets the flag on the latest message, and the
    /// action service derives that from the same table.
    #[test]
    fn p6_t02_star_latest_only_shape() {
        let sets = crate::actions::label_sets(&crate::dto::ActionKind::Star { on: true });
        assert_eq!(sets.add, vec!["STARRED"]);
        assert!(sets.remove.is_empty());
        assert!(!sets.delete_forever);
    }
}

/// The one label service the picker and the sidebar share (P8.5).
///
/// It answers with display names, nesting and colours; a provider id never
/// reaches a surface as a label.
#[tauri::command]
pub async fn labels_with_hierarchy(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<Vec<crate::dto::Label>, SiftError> {
    crate::labels::list(&state.db, &account_id).await
}

/// Rename a label, locally and durably (P8.5).
#[tauri::command]
pub async fn label_rename(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    label_id: String,
    name: String,
) -> Result<crate::dto::Label, SiftError> {
    let gesture = format!("label-rename-{}", uuid::Uuid::now_v7());
    let label =
        crate::labels::rename(&state.db, &account_id, &label_id, &name, &gesture).await?;
    state.kick_outbox(&account_id).await;
    let _ = app.emit(
        "store:labels",
        serde_json::json!({ "account_id": account_id, "label_ids": [label.id] }),
    );
    Ok(label)
}

/// Delete a label: organisation only, never the mail (P8.5).
#[tauri::command]
pub async fn label_delete(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    label_id: String,
) -> Result<(), SiftError> {
    crate::labels::delete(&state.db, &account_id, &label_id).await?;
    state.kick_outbox(&account_id).await;
    let _ = app.emit(
        "store:labels",
        serde_json::json!({ "account_id": account_id, "label_ids": [label_id] }),
    );
    Ok(())
}

/// How many messages Empty Trash would permanently delete.
///
/// The count is what the confirmation shows, and it counts messages rather
/// than threads because messages are what the operation names.
#[tauri::command]
pub async fn trash_empty_preview(
    state: State<'_, AppState>,
    account_ids: Vec<String>,
) -> Result<serde_json::Value, SiftError> {
    let mut messages = 0i64;
    for account_id in &account_ids {
        let account = account_id.clone();
        let n: i64 = state
            .db
            .read(move |c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM messages m \
                     JOIN message_labels ml ON ml.account_id=m.account_id AND ml.message_id=m.id \
                     WHERE m.account_id=? AND ml.label_id='TRASH' AND m.is_draft=0",
                    rusqlite::params![account],
                    |r| r.get(0),
                )?)
            })
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        messages += n;
    }
    Ok(serde_json::json!({ "count": messages }))
}

/// Permanently delete everything in Trash, by naming every message (P8.5).
///
/// Nothing here issues a folder-wide EXPUNGE: the messages that are really in
/// Trash are enumerated and queued through the P6.4 path, so the operation is
/// explicit, bounded per chunk and recoverable while it is unacknowledged.
#[tauri::command]
pub async fn trash_empty(
    app: AppHandle,
    state: State<'_, AppState>,
    account_ids: Vec<String>,
    gesture_id: Option<String>,
) -> Result<crate::dto::GestureResponse, SiftError> {
    let gesture = gesture_id.unwrap_or_else(|| format!("empty-trash-{}", uuid::Uuid::now_v7()));
    let (outcome, failures) =
        crate::actions::empty_trash(&state.db, &gesture, &account_ids).await?;
    let mut by_account: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for op in &outcome.operations {
        by_account.entry(op.account_id.clone()).or_default();
    }
    for account_id in by_account.keys() {
        state.kick_outbox(account_id).await;
        let _ = app.emit(
            "store:threads",
            serde_json::json!({ "account_id": account_id, "thread_ids": [] }),
        );
    }
    Ok(crate::dto::GestureResponse {
        gesture_id: gesture,
        operations: outcome
            .operations
            .iter()
            .map(crate::dto::OutboxOpSummary::from)
            .collect(),
        failures,
    })
}
