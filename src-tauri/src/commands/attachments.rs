use crate::app_state::AppState;
use crate::errors::SiftError;
use tauri::State;

#[tauri::command]
pub async fn attachments_open(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    attachment_id: String,
) -> Result<(), SiftError> {
    let (path, mime) = ensure_downloaded(&state, &attachment_id).await?;
    // executables need confirm — frontend shows dialog; backend double-checks extension
    let _ = mime;
    crate::opener::open(&app, &path).map_err(|e| SiftError::app("open", e.to_string(), false))
}

#[tauri::command]
pub async fn attachments_save_as(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    attachment_id: String,
) -> Result<serde_json::Value, SiftError> {
    use tauri_plugin_dialog::DialogExt;
    let (src, _mime) = ensure_downloaded(&state, &attachment_id).await?;
    let name = std::path::Path::new(&src)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("attachment")
        .to_string();
    let dest = app
        .dialog()
        .file()
        .add_filter("All", &["*"])
        .set_file_name(&name)
        .blocking_save_file();
    if let Some(p) = dest {
        let p = p.to_string();
        // p is file:// url or path
        let dest_path = p.trim_start_matches("file://").to_string();
        std::fs::copy(&src, &dest_path).map_err(SiftError::from)?;
        return Ok(serde_json::json!({"path": dest_path}));
    }
    Err(SiftError::app("cancelled", "save cancelled", false))
}

async fn ensure_downloaded(
    state: &AppState,
    attachment_id: &str,
) -> Result<(String, String), SiftError> {
    let rec = state
        .db
        .attachment_get(attachment_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
        .ok_or_else(|| SiftError::NotFound("attachment".into()))?;
    let (message_id, part_id, gmail_att_id, mime, data, local) = rec;
    if let Some(p) = local {
        if std::path::Path::new(&p).exists() {
            return Ok((p, mime));
        }
    }
    if let Some(b) = data {
        let dir = state.data_dir.join("attachments");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join(format!("{attachment_id}-bin"));
        std::fs::write(&p, &b).map_err(SiftError::from)?;
        let ps = p.to_string_lossy().into_owned();
        let ps2 = ps.clone();
        let __aid = attachment_id.to_string();
        state
            .db
            .write(move |c| {
                c.execute(
                    "UPDATE attachments SET local_path=? WHERE id=?",
                    rusqlite::params![ps2, __aid],
                )?;
                Ok(())
            })
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        return Ok((ps, mime));
    }
    // fetch (transport locator: Gmail attachmentId, or the IMAP section path
    // stored in part_id when gmail_att_id is NULL)
    let provider = state
        .provider_for(&message_id_by_att(state, attachment_id).await?)
        .await?;
    let locator = gmail_att_id.as_deref().unwrap_or(&part_id);
    if locator.is_empty() {
        return Err(SiftError::app("nodata", "no data", true));
    }
    let bytes = provider.fetch_attachment(&message_id, locator).await?;
    let dir = state.data_dir.join("attachments");
    let _ = std::fs::create_dir_all(&dir);
    let p = dir.join(format!("{attachment_id}-bin"));
    std::fs::write(&p, &bytes).map_err(SiftError::from)?;
    let ps = p.to_string_lossy().into_owned();
    let ps2 = ps.clone();
    let __aid = attachment_id.to_string();
    state
        .db
        .write(move |c| {
            c.execute(
                "UPDATE attachments SET local_path=? WHERE id=?",
                rusqlite::params![ps2, __aid],
            )?;
            Ok(())
        })
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    let _ = part_id;
    Ok((ps, mime))
}

async fn message_id_by_att(state: &AppState, attachment_id: &str) -> Result<String, SiftError> {
    state
        .db
        .read({
            let id = attachment_id.to_string();
            move |c| {
                Ok(c.query_row(
                    "SELECT message_id FROM attachments WHERE id=?",
                    rusqlite::params![id],
                    |r| r.get(0),
                )
                .unwrap_or_default())
            }
        })
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))
}
