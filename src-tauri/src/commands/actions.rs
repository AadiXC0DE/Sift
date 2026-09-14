//! Label creation and the legacy single-account entry points.
//!
//! The gesture path (P6.3) lives in [`crate::commands::outbox`]: it takes
//! account-qualified targets and one gesture id, so a mixed-account selection
//! is a single undoable group. What remains here is the label service, which
//! the picker and the sidebar share.

use crate::app_state::AppState;
use crate::errors::SiftError;
use tauri::State;

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
