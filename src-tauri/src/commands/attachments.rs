//! Attachment commands (P2.1–P2.4, P2.6).
//!
//! Thin wrappers: the attachment service owns identity, cache, streaming,
//! progress, saving and open policy. These commands only translate UI shapes
//! into service calls and run the native dialogs.
//!
//! Every command takes the account-qualified key from appendix A: an
//! attachment id or message id alone is not unique across accounts (P4.2).

use crate::attachments::service;
use crate::attachments::AttachmentRuntime;
use crate::app_state::AppState;
use crate::dto::{AttachmentRefKey, MessageRef, SaveAllResult, SaveAsResult};
use crate::errors::SiftError;
use std::path::PathBuf;
use tauri::State;

fn runtime(state: &AppState, app: &tauri::AppHandle) -> AttachmentRuntime {
    crate::attachments::runtime_for(state, app)
}

/// Offer a Save As destination. Returns `None` when the user cancels.
async fn prompt_save_path(app: &tauri::AppHandle, name: &str) -> Option<PathBuf> {
    use tauri_plugin_dialog::DialogExt;
    // Use the callback dialog, not `blocking_save_file`: the blocking variant
    // parks a runtime thread on a synchronous channel, which can deadlock the
    // command and leave the UI spinning forever.
    let (tx, rx) = tokio::sync::oneshot::channel();
    let name = name.to_string();
    app.dialog()
        .file()
        .add_filter("All", &["*"])
        .set_file_name(&name)
        .save_file(move |dest| {
            let _ = tx.send(dest);
        });
    let dest = rx.await.ok().flatten()?;
    dest.into_path().ok()
}

/// One folder prompt for Save All.
async fn prompt_folder(app: &tauri::AppHandle) -> Option<PathBuf> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog().file().pick_folder(move |folder| {
        let _ = tx.send(folder);
    });
    let folder = rx.await.ok().flatten()?;
    folder.into_path().ok()
}

/// Open an attachment with the system handler.
///
/// `confirmed_executable` is the caller's confirmation result for downloaded
/// app/script/executable types; without it those never reach the opener. The
/// check is enforced here, in the backend.
#[tauri::command]
pub async fn attachments_open(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    account_id: String,
    attachment_id: String,
    confirmed_executable: Option<bool>,
) -> Result<(), SiftError> {
    let rt = runtime(&state, &app);
    let key = AttachmentRefKey {
        account_id,
        attachment_id,
    };
    let rec = service::owned_record(&rt.db, &key).await?;
    service::open(&rt, &*state, &rec, confirmed_executable.unwrap_or(false), |path| {
        crate::opener::open_path(&app, &path.to_string_lossy())
    })
    .await
}

/// Save As. The destination is chosen *before* any uncached download, so a
/// cancelled dialog costs no network work and a failed copy leaves nothing
/// half-written at the destination.
#[tauri::command]
pub async fn attachments_save_as(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    account_id: String,
    attachment_id: String,
) -> Result<SaveAsResult, SiftError> {
    let rt = runtime(&state, &app);
    let key = AttachmentRefKey {
        account_id,
        attachment_id,
    };
    let rec = service::owned_record(&rt.db, &key).await?;
    let name = crate::attachments::naming::basename(
        rec.filename.as_deref(),
        &rec.mime,
        &rec.id,
    );
    let Some(dest) = prompt_save_path(&app, &name).await else {
        return Ok(SaveAsResult {
            path: None,
            cancelled: true,
        });
    };
    service::copy_to(&rt, &*state, &rec, &dest).await?;
    Ok(SaveAsResult {
        path: Some(dest.to_string_lossy().into_owned()),
        cancelled: false,
    })
}

/// Save All for a message with two or more non-inline attachments (P2.6).
#[tauri::command]
pub async fn attachments_save_all(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    account_id: String,
    message_id: String,
) -> Result<SaveAllResult, SiftError> {
    let rt = runtime(&state, &app);
    let Some(dir) = prompt_folder(&app).await else {
        return Ok(SaveAllResult {
            saved: 0,
            failed: vec![],
        });
    };
    service::save_all_into(&rt, &*state, &MessageRef::new(account_id, message_id), &dir).await
}

/// Cancel an in-flight transfer by the `requestId` the progress event carries
/// (an account-qualified attachment id). Immediate, and idempotent.
#[tauri::command]
pub async fn attachments_cancel(
    account_id: String,
    request_id: String,
) -> Result<(), SiftError> {
    service::cancel(account_id.trim(), request_id.trim());
    Ok(())
}
