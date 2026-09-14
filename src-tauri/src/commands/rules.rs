//! Rule commands (P8.3).
//!
//! Every command here is explicit about what it does before it does it:
//! `rules_preview` counts and samples without changing anything, and
//! `rules_apply_existing` requires the user to have seen that count. A rule
//! that was never enabled evaluates nothing, and enabling one never silently
//! rewrites the mailbox.

use crate::app_state::AppState;
use crate::dto::{MailRule, RulePreview};
use crate::errors::SiftError;
use tauri::{AppHandle, Emitter, State};

fn db_error(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

#[tauri::command]
pub async fn rules_list(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<Vec<MailRule>, SiftError> {
    state.db.rules_list(&account_id).await.map_err(db_error)
}

/// Create or edit one rule.
///
/// The whole rule is validated against the closed vocabulary before anything is
/// stored, so a rule that cannot be evaluated can never exist — a stored rule
/// that silently never matches would be worse than a refusal.
#[tauri::command]
pub async fn rules_upsert(
    state: State<'_, AppState>,
    rule: MailRule,
    expected_revision: Option<i64>,
) -> Result<MailRule, SiftError> {
    crate::db::rules::validate(&rule).map_err(|m| SiftError::app("rule_invalid", m, false))?;
    let mut rule = rule;
    if rule.id.trim().is_empty() {
        rule.id = format!("rule-{}", uuid::Uuid::now_v7());
    }
    state
        .db
        .rule_upsert(rule, expected_revision)
        .await
        .map_err(|e| match e.downcast::<SiftError>() {
            Ok(sift) => sift,
            Err(other) => db_error(other),
        })
}

/// Delete a rule. Its application records go with it: the rule no longer
/// exists, so "already applied by revision 3 of a rule that is gone" has no
/// meaning.
#[tauri::command]
pub async fn rules_delete(
    state: State<'_, AppState>,
    account_id: String,
    rule_id: String,
) -> Result<(), SiftError> {
    state
        .db
        .rule_delete(&account_id, &rule_id)
        .await
        .map_err(db_error)
}

/// Count what a rule would do, and show a few examples. Nothing changes.
#[tauri::command]
pub async fn rules_preview(
    state: State<'_, AppState>,
    account_id: String,
    rule: MailRule,
    limit: Option<i64>,
) -> Result<RulePreview, SiftError> {
    crate::db::rules::validate(&rule).map_err(|m| SiftError::app("rule_invalid", m, false))?;
    crate::rules::preview(&state.db, &account_id, &rule, limit.unwrap_or(20)).await
}

/// Apply a saved rule to mail the user already has.
///
/// The revision is required so the run is bound to the rule the preview
/// described: if the rule was edited in between, the run refuses rather than
/// applying something the user never saw.
#[tauri::command]
pub async fn rules_apply_existing(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    rule_id: String,
    revision: i64,
) -> Result<crate::rules::ApplyReport, SiftError> {
    let rule = state
        .db
        .rule_get(&account_id, &rule_id)
        .await
        .map_err(db_error)?
        .ok_or_else(|| SiftError::NotFound("rule".into()))?;
    if rule.revision != revision {
        return Err(SiftError::app(
            "rule_conflict",
            "This rule changed since the preview. Preview it again before applying.",
            false,
        ));
    }
    let report = crate::rules::apply_existing(&state.db, &account_id, &rule).await?;
    if report.applied > 0 {
        let _ = app.emit(
            "store:threads",
            serde_json::json!({ "account_id": account_id, "thread_ids": [] }),
        );
        state.kick_outbox(&account_id).await;
    }
    Ok(report)
}

/// "Block sender in Sift" (P8.3).
///
/// This is a rule, not Gmail's account-wide block: it moves future *downloaded*
/// mail from that address to Junk when Sift syncs. The UI copy says so, because
/// a user who also uses Gmail's own block button would otherwise assume the
/// block travels with their account.
#[tauri::command]
pub async fn rule_block_sender(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    email: String,
    name: Option<String>,
) -> Result<MailRule, SiftError> {
    let rule =
        crate::rules::block_sender(&state.db, &account_id, &email, name.as_deref()).await?;
    let _ = app.emit(
        "rules:state",
        serde_json::json!({ "account_id": account_id }),
    );
    Ok(rule)
}

/// Why a rule disabled itself, for the settings surface.
#[tauri::command]
pub async fn rules_diagnostics(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<Vec<MailRule>, SiftError> {
    let rules = state.db.rules_list(&account_id).await.map_err(db_error)?;
    Ok(rules.into_iter().filter(|r| r.last_error.is_some()).collect())
}
