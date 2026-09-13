//! Compose commands (P5.1, P5.3, P5.4, P5.5 backend half).
//!
//! Drafts are saved locally and immediately; remote draft synchronization is
//! debounced through the outbox and never blocks a keystroke. Sending freezes
//! the draft into a prepared MIME file and enqueues one operation, after which
//! the draft still exists: only an acknowledged send may clean it up (P6.2).

use crate::app_state::AppState;
use crate::dto::{Contact, Draft, DraftPage, SendHandle};
use crate::errors::SiftError;
use crate::outgoing::{self, PreparedSend};
use tauri::State;

fn db_error(e: anyhow::Error) -> SiftError {
    // A failed local save is reported as a storage failure, never as
    // "offline": the user must know the text was not written to disk.
    SiftError::app("storage", format!("could not save the draft: {e}"), true)
}

/// Sending identity for a draft: the account's own address unless the user
/// picked one of its verified aliases.
async fn identity_for(state: &AppState, draft: &Draft) -> Result<outgoing::Identity, SiftError> {
    let account = state
        .db
        .accounts_get(&draft.account_id)
        .await
        .map_err(db_error)?
        .ok_or_else(|| SiftError::NotFound("account".into()))?;
    let selected = draft
        .from_email
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty());
    match selected {
        Some(from) if !from.eq_ignore_ascii_case(&account.email) => Err(SiftError::app(
            "from_not_authorized",
            format!("{from} is not a sending identity of {}", account.email),
            false,
        )),
        _ => Ok(outgoing::Identity {
            email: account.email.clone(),
            display_name: account.display_name.clone(),
        }),
    }
}

#[tauri::command]
pub async fn drafts_get(state: State<'_, AppState>, local_id: String) -> Result<Draft, SiftError> {
    state
        .db
        .drafts_get(&local_id)
        .await
        .map_err(db_error)?
        .ok_or_else(|| SiftError::NotFound("draft".into()))
}

#[tauri::command]
pub async fn drafts_list(
    state: State<'_, AppState>,
    account_ids: Vec<String>,
    cursor: Option<String>,
    limit: Option<i64>,
) -> Result<DraftPage, SiftError> {
    state
        .db
        .drafts_list(&account_ids, cursor.as_deref(), limit.unwrap_or(50))
        .await
        .map_err(db_error)
}

/// Save a draft. Local commit is immediate; the remote copy is queued behind a
/// 2 second idle debounce so a burst of keystrokes produces one Gmail draft.
#[tauri::command]
pub async fn drafts_upsert(
    state: State<'_, AppState>,
    draft: Draft,
    expected_revision: Option<i64>,
) -> Result<Draft, SiftError> {
    let saved = state
        .db
        .drafts_upsert(&draft, expected_revision)
        .await
        .map_err(db_error)?;
    // A locally-unsaved revision is what schedules remote synchronization; a
    // no-op save (same content) leaves the queue alone.
    if saved.has_unsaved_revision() {
        if let Err(e) = state
            .db
            .outbox_enqueue_draft_sync(&saved.account_id, &saved.local_id, saved.revision)
            .await
        {
            // The draft is safely on disk; a queue failure must not fail the
            // save, but it must be visible.
            log::warn!("draft {} could not be queued for remote sync: {e}", saved.local_id);
        }
    }
    Ok(saved)
}

#[tauri::command]
pub async fn drafts_delete(state: State<'_, AppState>, local_id: String) -> Result<(), SiftError> {
    // Explicit user discard: the local row and its staged files go away, and
    // the remote copy is removed best effort. The send path never calls this.
    let draft = state.db.drafts_get(&local_id).await.map_err(db_error)?;
    if let Some(d) = &draft {
        if let Some(rid) = d.remote_draft_id.clone() {
            if let Ok(p) = state.provider_for(&d.account_id).await {
                let _ = p.draft_delete(&rid).await;
            }
        }
        let _ = state
            .db
            .outbox_cancel_draft_sync(&d.account_id, &local_id)
            .await;
    }
    state.db.drafts_delete(&local_id).await.map_err(db_error)?;
    // The staged files are draft-owned, but a recovered copy shares them: only
    // remove the directory when nothing else points into it.
    let dir = outgoing::draft_send_dir(&state.data_dir, &local_id);
    let shared = state
        .db
        .drafts_share_staging_dir(&dir.to_string_lossy(), &local_id)
        .await
        .unwrap_or(true);
    if !shared {
        let _ = tokio::fs::remove_dir_all(dir).await;
    }
    Ok(())
}

/// Queue a send for an exact revision of a draft.
///
/// The MIME snapshot is built and validated here, written to a draft-owned
/// file, and only its path/size/envelope go into the outbox — never megabytes
/// of base64 inside SQLite.
#[tauri::command]
pub async fn drafts_send(
    state: State<'_, AppState>,
    local_id: String,
    revision: i64,
    not_before: Option<i64>,
    archive_after_send: Option<bool>,
) -> Result<SendHandle, SiftError> {
    let draft = state
        .db
        .drafts_get(&local_id)
        .await
        .map_err(db_error)?
        .ok_or_else(|| SiftError::NotFound("draft".into()))?;
    if draft.revision != revision {
        return Err(SiftError::app(
            "draft_conflict",
            "This draft changed since you hit send. Reopen it and send again.",
            false,
        ));
    }
    let identity = identity_for(&state, &draft).await?;
    let data_dir = state.data_dir.clone();
    let snapshot = draft.clone();
    // Reading attachment files, encoding MIME and writing the raw file are all
    // blocking: keep them off the async runtime.
    let prepared: PreparedSend = tokio::task::spawn_blocking(move || {
        outgoing::prepare(&data_dir, &snapshot, &identity, crate::db::now_ms() / 1000)
    })
    .await
    .map_err(|e| SiftError::app("storage", format!("send preparation failed: {e}"), true))??;

    let deadline = not_before.unwrap_or(0).max(crate::db::now_ms());
    state
        .db
        .drafts_enqueue_send(&prepared, deadline, archive_after_send.unwrap_or(false))
        .await
        .map_err(db_error)
}

/// Cancel a send that has not been claimed yet and return the reopened draft.
#[tauri::command]
pub async fn send_cancel(state: State<'_, AppState>, op_id: i64) -> Result<Draft, SiftError> {
    state
        .db
        .drafts_cancel_send(op_id)
        .await
        .map_err(db_error)?
        .ok_or_else(|| {
            SiftError::app(
                "too_late",
                "This message is already on its way and can't be undone.",
                false,
            )
        })
}

/// Sift's current send limit, so the composer can show the real number
/// instead of a hardcoded claim about Gmail.
#[tauri::command]
pub async fn compose_limits() -> Result<serde_json::Value, SiftError> {
    Ok(serde_json::json!({
        "rawMimeLimitBytes": outgoing::RAW_MIME_LIMIT_BYTES,
    }))
}

#[tauri::command]
pub async fn contacts_suggest(
    state: State<'_, AppState>,
    account_id: String,
    q: String,
    limit: i64,
) -> Result<Vec<Contact>, SiftError> {
    state
        .db
        .contacts_suggest(&account_id, &q, limit.min(20))
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))
}

/// Stage files into a draft-owned directory.
///
/// The whole batch is staged before anything is reported: on failure the files
/// added by this call are removed again, and the caller keeps the attachments
/// it already had.
#[tauri::command]
pub async fn attachments_add_from_paths(
    state: State<'_, AppState>,
    account_id: String,
    draft_id: String,
    paths: Vec<String>,
) -> Result<Vec<crate::dto::AttachmentRef>, SiftError> {
    if draft_id.trim().is_empty() {
        return Err(SiftError::app(
            "bad_id",
            "a staged attachment needs a draft to belong to",
            false,
        ));
    }
    // Drafts belong to an account; a caller that names the wrong one is a bug
    // we refuse rather than file the file under a foreign draft.
    let owner = state
        .db
        .drafts_get(&draft_id)
        .await
        .map_err(db_error)?;
    if let Some(d) = &owner {
        if !account_id.is_empty() && d.account_id != account_id {
            return Err(SiftError::app(
                "bad_id",
                "that draft belongs to a different account",
                false,
            ));
        }
    }
    let dir = outgoing::draft_send_dir(&state.data_dir, &draft_id);
    tokio::task::spawn_blocking(move || stage_paths(&dir, &paths))
        .await
        .map_err(|e| SiftError::app("storage", format!("staging failed: {e}"), true))?
}

fn stage_paths(
    dir: &std::path::Path,
    paths: &[String],
) -> Result<Vec<crate::dto::AttachmentRef>, SiftError> {
    std::fs::create_dir_all(dir).map_err(|e| {
        SiftError::app("storage", format!("could not create the draft folder: {e}"), false)
    })?;
    let mut staged: Vec<std::path::PathBuf> = Vec::new();
    let mut out = Vec::with_capacity(paths.len());
    let result = (|| -> Result<(), SiftError> {
        for p in paths {
            let src = std::path::Path::new(p);
            let meta = std::fs::metadata(src).map_err(|e| {
                SiftError::app("attachment_missing", format!("{p} could not be read: {e}"), false)
            })?;
            if !meta.is_file() {
                return Err(SiftError::app(
                    "attachment_missing",
                    format!("{p} is not a regular file"),
                    false,
                ));
            }
            let name = src
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or_else(|| SiftError::app("bad_id", "attachment has no file name", false))?
                .to_string();
            let dest = crate::attachments::service::unique_destination(dir, &name);
            std::fs::copy(src, &dest).map_err(|e| {
                SiftError::app(
                    "storage",
                    format!("{name} could not be staged: {e}"),
                    false,
                )
            })?;
            staged.push(dest.clone());
            out.push(crate::dto::AttachmentRef {
                mime: mime_guess(&name),
                name,
                size: meta.len() as i64,
                path: dest.to_string_lossy().into_owned(),
            });
        }
        Ok(())
    })();
    if let Err(e) = result {
        // Roll back only what this call staged; earlier attachments stay.
        for f in staged {
            let _ = std::fs::remove_file(f);
        }
        return Err(e);
    }
    Ok(out)
}

/// Stage one attachment of an existing message into a draft (forwarding).
#[tauri::command]
pub async fn attachments_stage_from_message(
    state: State<'_, AppState>,
    account_id: String,
    message_id: String,
    attachment_id: String,
    draft_id: String,
) -> Result<crate::dto::AttachmentRef, SiftError> {
    use crate::attachments::service;
    use crate::dto::{AttachmentRef, AttachmentRefKey, MessageRef};

    let key = AttachmentRefKey {
        account_id: account_id.clone(),
        attachment_id: attachment_id.clone(),
    };
    let rec = service::owned_record(&state.db, &key).await?;
    let message = MessageRef::new(account_id, message_id);
    if rec.message_id != message.message_id {
        return Err(SiftError::app(
            "bad_id",
            "that attachment belongs to a different message",
            false,
        ));
    }
    // `AppState` is the transport source: it resolves the account's provider,
    // so staging a forward uses the same cache/verified-file path as Save As.
    let source: &dyn service::TransportSource = &*state;
    let rt = service::AttachmentRuntime::new(state.db.clone(), state.data_dir.clone());
    let info = service::ensure_local(&rt, source, &rec).await?;
    let path = info
        .path
        .ok_or_else(|| SiftError::app("attachment_failed", "no verified local copy", true))?;
    let dir = outgoing::draft_send_dir(&state.data_dir, &draft_id);
    tokio::fs::create_dir_all(&dir).await.map_err(|e| {
        SiftError::app(
            "storage",
            format!("could not create the draft folder: {e}"),
            false,
        )
    })?;
    let name = info
        .display_name
        .clone()
        .unwrap_or_else(|| rec.id.clone());
    let dest = service::unique_destination(&dir, &name);
    crate::attachments::cache::copy_atomic(std::path::Path::new(&path), &dest)
        .await
        .map_err(|e| {
            SiftError::app(
                "storage",
                format!("{name} could not be staged: {e}"),
                true,
            )
        })?;
    let size = tokio::fs::metadata(&dest)
        .await
        .map_err(|e| SiftError::app("storage", format!("{name} vanished: {e}"), true))?
        .len() as i64;
    Ok(AttachmentRef {
        name,
        mime: info.mime,
        size,
        path: dest.to_string_lossy().into_owned(),
    })
}

fn mime_guess(name: &str) -> String {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "txt" => "text/plain",
        "html" => "text/html",
        "ics" => "text/calendar",
        "eml" => "message/rfc822",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p53_staging_rolls_back_a_failed_batch() {
        let dir = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let good = src.path().join("invoice.pdf");
        std::fs::write(&good, b"pdf").unwrap();
        let dest = dir.path().join("draft-1");
        let err = stage_paths(
            &dest,
            &[
                good.to_string_lossy().into_owned(),
                src.path().join("missing.pdf").to_string_lossy().into_owned(),
            ],
        )
        .unwrap_err();
        assert_eq!(
            serde_json::to_value(&err).unwrap()["code"],
            "attachment_missing"
        );
        // Nothing from the failed batch is left behind.
        assert!(!dest.join("invoice.pdf").exists());
    }

    #[test]
    fn p53_staging_copies_real_bytes_and_rejects_directories() {
        let dir = tempfile::tempdir().unwrap();
        let src = tempfile::tempdir().unwrap();
        let good = src.path().join("note.txt");
        std::fs::write(&good, b"hello").unwrap();
        let dest = dir.path().join("draft-1");
        let staged = stage_paths(&dest, &[good.to_string_lossy().into_owned()]).unwrap();
        assert_eq!(staged[0].name, "note.txt");
        assert_eq!(staged[0].mime, "text/plain");
        assert_eq!(staged[0].size, 5);
        assert_eq!(std::fs::read(&staged[0].path).unwrap(), b"hello");

        let err = stage_paths(&dest, &[src.path().to_string_lossy().into_owned()]).unwrap_err();
        assert_eq!(
            serde_json::to_value(&err).unwrap()["code"],
            "attachment_missing"
        );
    }
}
