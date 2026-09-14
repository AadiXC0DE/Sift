//! Notification and VIP commands (P8.4).
//!
//! The settings surface needs to describe behaviour truthfully, including the
//! case where the user turned notifications on and the OS said no. Permission
//! is requested here — when the user enables notifications — and never from the
//! delivery path, which is what stops a denied user from being prompted on
//! every incoming message.

use crate::app_state::AppState;
use crate::dto::{Contact, NotificationState, Settings};
use crate::errors::SiftError;
use tauri::{AppHandle, State};

fn db_error(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

fn state_of(settings: &Settings, permission: &str) -> NotificationState {
    NotificationState {
        enabled: settings.notifications != crate::notify::FILTER_OFF,
        permission: permission.to_string(),
        filter: settings.notifications.clone(),
        hide_subject: settings.notifications_hide_subject,
        sound: if settings.sound == "off" {
            "none".into()
        } else {
            "native".into()
        },
        muted_accounts: settings.notifications_muted_accounts.clone(),
    }
}

#[tauri::command]
pub async fn notifications_state(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<NotificationState, SiftError> {
    let settings = state.db.settings_get().await.map_err(db_error)?;
    Ok(state_of(&settings, crate::notify::permission_state(&app)))
}

/// Turn notifications on or off. Enabling asks the OS for permission once; the
/// answer (including a refusal) is what the returned state reports.
#[tauri::command]
pub async fn notifications_enable(
    app: AppHandle,
    state: State<'_, AppState>,
    enabled: bool,
) -> Result<NotificationState, SiftError> {
    let filter = if enabled {
        // Turning them back on returns to the Inbox default rather than to
        // whatever was stored, which could have been a VIP-only mode the user
        // forgot about.
        crate::notify::FILTER_INBOX.to_string()
    } else {
        crate::notify::FILTER_OFF.to_string()
    };
    state
        .db
        .settings_set(serde_json::json!({ "notifications": filter }))
        .await
        .map_err(db_error)?;
    if enabled {
        crate::notify::request_permission(&app);
    }
    let settings = state.db.settings_get().await.map_err(db_error)?;
    Ok(state_of(&settings, crate::notify::permission_state(&app)))
}

/// Change the filter, the hidden-subject mode or which accounts are quiet.
#[tauri::command]
pub async fn notifications_update(
    app: AppHandle,
    state: State<'_, AppState>,
    filter: Option<String>,
    hide_subject: Option<bool>,
    sound: Option<String>,
    muted_accounts: Option<Vec<String>>,
) -> Result<NotificationState, SiftError> {
    let mut patch = serde_json::Map::new();
    if let Some(filter) = filter {
        if ![
            crate::notify::FILTER_OFF,
            crate::notify::FILTER_INBOX,
            crate::notify::FILTER_VIP,
        ]
        .contains(&filter.as_str())
        {
            return Err(SiftError::app(
                "bad_filter",
                "Notifications can be off, for the Inbox, or for VIPs only.",
                false,
            ));
        }
        patch.insert("notifications".into(), serde_json::Value::String(filter));
    }
    if let Some(hide) = hide_subject {
        patch.insert(
            "notificationsHideSubject".into(),
            serde_json::Value::Bool(hide),
        );
    }
    if let Some(sound) = sound {
        if sound != "native" && sound != "none" {
            return Err(SiftError::app(
                "bad_sound",
                "Sift plays the system notification sound or nothing at all.",
                false,
            ));
        }
        patch.insert(
            "sound".into(),
            serde_json::Value::String(if sound == "none" { "off" } else { "subtle" }.into()),
        );
    }
    if let Some(muted) = muted_accounts {
        patch.insert(
            "notificationsMutedAccounts".into(),
            serde_json::Value::Array(
                muted.into_iter().map(serde_json::Value::String).collect(),
            ),
        );
    }
    if !patch.is_empty() {
        state
            .db
            .settings_set(serde_json::Value::Object(patch))
            .await
            .map_err(db_error)?;
    }
    let settings = state.db.settings_get().await.map_err(db_error)?;
    Ok(state_of(&settings, crate::notify::permission_state(&app)))
}

#[tauri::command]
pub async fn vip_list(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<Vec<String>, SiftError> {
    state.db.vip_list(&account_id).await.map_err(db_error)
}

#[tauri::command]
pub async fn vip_set(
    state: State<'_, AppState>,
    account_id: String,
    email: String,
    vip: bool,
) -> Result<Vec<String>, SiftError> {
    state
        .db
        .vip_set(&account_id, &email, vip)
        .await
        .map_err(db_error)
}

/// Recent correspondents, for choosing a VIP without any address-book
/// permission: these are the addresses Sift has already seen in the mailbox.
#[tauri::command]
pub async fn vip_candidates(
    state: State<'_, AppState>,
    account_id: String,
    limit: Option<i64>,
) -> Result<Vec<Contact>, SiftError> {
    state
        .db
        .contacts_suggest(&account_id, "", limit.unwrap_or(20).clamp(1, 50))
        .await
        .map_err(db_error)
}

/// Consume the ref of a notification the user just came back to.
///
/// The OS reports focus rather than which banner was clicked, so the runtime
/// remembers the last ref it notified about; the frontend may either listen for
/// the `nav:open-thread` event or pull it here, and the second reader gets
/// nothing rather than a stale navigation.
#[tauri::command]
pub async fn notification_pending_open() -> Result<Option<serde_json::Value>, SiftError> {
    Ok(crate::runtime::take_pending_nav().map(|(account_id, thread_id)| {
        serde_json::json!({ "accountId": account_id, "threadId": thread_id })
    }))
}
