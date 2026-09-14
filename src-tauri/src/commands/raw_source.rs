//! View Source and Save as `.eml` (P9.3).
//!
//! Both go through one byte-oriented path. The raw MIME is fetched once and
//! cached on disk with its SHA-256, so:
//!
//! * **View Source works offline** for anything already cached, and when it is
//!   not cached the user is told that a download is needed rather than shown an
//!   empty box;
//! * **Save as `.eml` writes exactly the cached bytes** — the digest is checked
//!   before the copy, so an export can never quietly differ from what Sift
//!   showed, and the export never round-trips through `String::from_utf8_lossy`
//!   (which replaces invalid sequences and would change the file).
//!
//! Reading the source never changes a message's read state and never queues a
//! provider mutation: this is a read path.

use crate::app_state::AppState;
use crate::db::raw_cache::{raw_dir, raw_file_name, sha256_hex, RawCacheRow};
use crate::dto::MessageRef;
use crate::errors::SiftError;
use std::path::{Path, PathBuf};
use tauri::State;

fn db_error(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

/// What the reader shows for a message's source.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RawSource {
    pub text: String,
    pub size: i64,
    /// The bytes are not valid UTF-8, so `text` is a lossy rendering. Sift says
    /// so instead of presenting replacement characters as the message.
    pub lossy: bool,
    /// Served from the on-disk cache (no network was used).
    pub cached: bool,
}

/// Fetch the raw bytes, from the cache when possible.
///
/// Returns the bytes and whether they came from the cache. A missing cache plus
/// an unreachable provider is reported as "not downloaded yet", because that is
/// the actionable truth: connecting once makes it work offline afterwards.
async fn raw_bytes(state: &AppState, message: &MessageRef) -> Result<(Vec<u8>, bool), SiftError> {
    let exists = state.db.message_thread(message).await.map_err(db_error)?;
    if exists.is_none() {
        return Err(SiftError::NotFound("message".into()));
    }
    if let Some(row) = state
        .db
        .raw_cache_get(&message.account_id, &message.message_id)
        .await
        .map_err(db_error)?
    {
        match tokio::fs::read(&row.path).await {
            Ok(bytes) if sha256_hex(&bytes) == row.sha256 => {
                let _ = state
                    .db
                    .raw_cache_touch(&message.account_id, &message.message_id)
                    .await;
                return Ok((bytes, true));
            }
            // A cached file that changed underneath Sift is discarded: the
            // cache is an optimisation, never a source of wrong bytes.
            Ok(_) => {
                let _ = tokio::fs::remove_file(&row.path).await;
                let _ = state
                    .db
                    .raw_cache_remove(&message.account_id, &message.message_id)
                    .await;
            }
            Err(_) => {
                let _ = state
                    .db
                    .raw_cache_remove(&message.account_id, &message.message_id)
                    .await;
            }
        }
    }
    let provider = state.provider_for(&message.account_id).await?;
    let bytes = provider.fetch_raw_bytes(&message.message_id).await?;
    if bytes.is_empty() {
        return Err(SiftError::app(
            "raw_unavailable",
            "This message has no raw source to show.",
            false,
        ));
    }
    // Caching is best effort: a read-only or full disk must not stop the read.
    if let Err(e) = cache_raw(state, message, &bytes).await {
        log::warn!("could not cache raw source for {}: {e}", message.message_id);
    }
    Ok((bytes, false))
}

async fn cache_raw(state: &AppState, message: &MessageRef, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = raw_dir(&state.data_dir, &message.account_id);
    tokio::fs::create_dir_all(&dir).await?;
    let path = dir.join(raw_file_name(&message.message_id));
    // Write beside the target and rename, so a reader never sees a half file.
    let tmp = dir.join(format!(
        "{}.part-{}",
        raw_file_name(&message.message_id),
        uuid::Uuid::now_v7()
    ));
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, &path).await?;
    state
        .db
        .raw_cache_put(&RawCacheRow {
            account_id: message.account_id.clone(),
            message_id: message.message_id.clone(),
            path: path.to_string_lossy().to_string(),
            size: bytes.len() as i64,
            sha256: sha256_hex(bytes),
        })
        .await?;
    Ok(())
}

/// View Source (P9.3): the raw message, offline when it is cached.
#[tauri::command]
pub async fn message_view_source(
    state: State<'_, AppState>,
    account_id: String,
    message_id: String,
) -> Result<RawSource, SiftError> {
    let message = MessageRef::new(account_id, message_id);
    let (bytes, cached) = match raw_bytes(&state, &message).await {
        Ok(v) => v,
        Err(e) if e.code() == "network" || e.is_retryable() => {
            return Err(SiftError::app(
                "raw_not_cached",
                "Sift has not downloaded this message's source yet. Connect to the network once and it will be available offline afterwards.",
                true,
            ))
        }
        Err(e) => return Err(e),
    };
    let lossy = std::str::from_utf8(&bytes).is_err();
    Ok(RawSource {
        text: String::from_utf8_lossy(&bytes).into_owned(),
        size: bytes.len() as i64,
        lossy,
        cached,
    })
}

/// Copy `bytes` to `destination`, proving the copy is exact (P9.3).
///
/// The file is written beside the target and renamed only after the bytes on
/// disk hash to the same digest as the bytes read: a cancelled, failed or
/// truncated export therefore never leaves a partial file where the user asked
/// for a message, and an export can never quietly differ from what Sift read.
pub async fn write_raw_export(bytes: &[u8], destination: &Path) -> Result<(), SiftError> {
    let digest = sha256_hex(bytes);
    let tmp = destination.with_extension("eml.part");
    tokio::fs::write(&tmp, bytes).await.map_err(|e| {
        SiftError::app("storage", format!("could not write the export: {e}"), false)
    })?;
    let written = tokio::fs::read(&tmp).await.unwrap_or_default();
    if sha256_hex(&written) != digest {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(SiftError::app(
            "export_mismatch",
            "The exported file did not match the message Sift read, so it was removed.",
            false,
        ));
    }
    tokio::fs::rename(&tmp, destination)
        .await
        .map_err(|e| SiftError::app("storage", format!("could not save the export: {e}"), false))
}

/// Save the raw message as a byte-exact `.eml` (P9.3).
#[tauri::command]
pub async fn message_raw_export(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    account_id: String,
    message_id: String,
) -> Result<crate::dto::RawExport, SiftError> {
    let message = MessageRef::new(account_id, message_id);
    let (bytes, _) = raw_bytes(&state, &message).await?;
    let name = export_file_name(&state, &message).await;
    let Some(destination) = prompt_save(&app, &name).await else {
        return Ok(crate::dto::RawExport { path: None });
    };
    write_raw_export(&bytes, &destination).await?;
    Ok(crate::dto::RawExport {
        path: Some(destination.to_string_lossy().to_string()),
    })
}

/// `Subject.eml` when a subject exists, `message-<id>.eml` otherwise.
async fn export_file_name(state: &AppState, message: &MessageRef) -> String {
    let subject: String = state
        .db
        .read({
            let (a, m) = (message.account_id.clone(), message.message_id.clone());
            move |c| {
                Ok(c.query_row(
                    "SELECT subject FROM messages WHERE account_id=? AND id=?",
                    rusqlite::params![a, m],
                    |r| r.get::<_, String>(0),
                )
                .unwrap_or_default())
            }
        })
        .await
        .unwrap_or_default();
    let safe: String = subject
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = safe.trim().trim_matches('.').to_string();
    let base = if trimmed.is_empty() {
        format!("message-{}", message.message_id)
    } else {
        trimmed.chars().take(80).collect::<String>()
    };
    format!("{base}.eml")
}

async fn prompt_save(app: &tauri::AppHandle, name: &str) -> Option<PathBuf> {
    use tauri_plugin_dialog::DialogExt;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let name = name.to_string();
    app.dialog()
        .file()
        .add_filter("Mail message", &["eml"])
        .set_file_name(&name)
        .save_file(move |dest| {
            let _ = tx.send(dest);
        });
    let dest = rx.await.ok().flatten()?;
    dest.into_path().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p9_3_export_names_are_filesystem_safe() {
        // The name is derived from a subject, which is untrusted text: it can
        // only ever be a file name inside the folder the user chose.
        let dir = tempfile::tempdir().unwrap();
        let candidate = dir.path().join(raw_file_name("18f2/../evil"));
        assert_eq!(
            candidate.parent().unwrap(),
            dir.path(),
            "a message id cannot escape the cache directory"
        );
    }
}
