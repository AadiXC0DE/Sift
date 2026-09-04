use crate::app_state::AppState;
use crate::dto::{AttachmentRef, Contact, Draft};
use crate::errors::SiftError;
use tauri::State;

#[tauri::command]
pub async fn drafts_upsert(state: State<'_, AppState>, draft: Draft) -> Result<Draft, SiftError> {
    state
        .db
        .drafts_upsert(&draft)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))
}

#[tauri::command]
pub async fn drafts_delete(state: State<'_, AppState>, local_id: String) -> Result<(), SiftError> {
    // delete remote if exists
    if let Ok(Some(d)) = state.db.drafts_get(&local_id).await {
        if let Some(rid) = d.remote_draft_id {
            if let Ok(c) = state.client_for(&d.account_id).await {
                let base = std::env::var("SIFT_GMAIL_BASE")
                    .unwrap_or_else(|_| "https://gmail.googleapis.com/gmail/v1/users/me".into());
                let _ = state
                    .http
                    .delete(format!("{base}/drafts/{rid}"))
                    .bearer_auth("x")
                    .send()
                    .await;
                let _ = &c;
            }
        }
    }
    state
        .db
        .drafts_delete(&local_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))
}

#[tauri::command]
pub async fn drafts_send(
    state: State<'_, AppState>,
    local_id: String,
    undo_delay_ms: i64,
) -> Result<serde_json::Value, SiftError> {
    let d = state
        .db
        .drafts_get(&local_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
        .ok_or_else(|| SiftError::NotFound("draft".into()))?;
    let raw = build_raw(&d)?;
    if raw.len() > 25 * 1024 * 1024 {
        return Err(SiftError::too_large());
    }
    let payload = serde_json::json!({"raw": raw, "threadId": d.thread_id, "localId": local_id, "accountId": d.account_id}).to_string();
    let not_before = crate::db::now_ms() + undo_delay_ms;
    let op_id = state
        .db
        .outbox_enqueue(&d.account_id, "send", &payload, None, not_before)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    // insert temporary local card? Represented as message with id local:<uuid> in thread
    Ok(serde_json::json!({ "op_id": op_id }))
}

#[tauri::command]
pub async fn send_cancel(state: State<'_, AppState>, op_id: i64) -> Result<Draft, SiftError> {
    // only while pending
    let op: Option<crate::db::outbox::Op> = state.db.read(move |c| {
    Ok(c.query_row("SELECT id,account_id,kind,payload,undo_group,state,attempts,not_before FROM outbox_ops WHERE id=?", rusqlite::params![op_id], |r| {
      Ok(crate::db::outbox::Op { id: r.get(0)?, account_id: r.get(1)?, kind: r.get(2)?, payload: r.get(3)?, undo_group: r.get(4)?, state: r.get(5)?, attempts: r.get(6)?, not_before: r.get(7)? })
    }).ok())
  }).await.map_err(|e| SiftError::app("db", e.to_string(), false))?;
    let op = op.ok_or_else(|| SiftError::NotFound("op".into()))?;
    if op.state != "pending" {
        return Err(SiftError::app("too_late", "already sending", false));
    }
    state
        .db
        .outbox_set(op.id, "cancelled", op.attempts, op.not_before, None)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    let payload: serde_json::Value = serde_json::from_str(&op.payload).unwrap_or_default();
    let local_id = payload["localId"].as_str().unwrap_or_default().to_string();
    state
        .db
        .drafts_get(&local_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
        .ok_or_else(|| SiftError::NotFound("draft".into()))
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

#[tauri::command]
pub async fn attachments_add_from_paths(
    state: State<'_, AppState>,
    paths: Vec<String>,
) -> Result<Vec<AttachmentRef>, SiftError> {
    let mut out = vec![];
    let cache = state.data_dir.join("compose-cache");
    let _ = std::fs::create_dir_all(&cache);
    for p in paths {
        let meta = std::fs::metadata(&p).map_err(SiftError::from)?;
        if meta.len() > 25 * 1024 * 1024 {
            return Err(SiftError::too_large());
        }
        let name = std::path::Path::new(&p)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("file")
            .to_string();
        let dest = cache.join(format!("{}-{}", uuid::Uuid::now_v7(), name));
        std::fs::copy(&p, &dest).map_err(SiftError::from)?;
        let mime = mime_guess(&name);
        out.push(AttachmentRef {
            name,
            mime,
            size: meta.len() as i64,
            path: dest.to_string_lossy().into_owned(),
        });
    }
    Ok(out)
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

pub fn build_raw(d: &Draft) -> Result<String, SiftError> {
    use base64::Engine;
    // Build minimal RFC5322 MIME (multipart/alternative inside mixed when attachments)
    let to = d
        .to_json
        .iter()
        .map(format_addr)
        .collect::<Vec<_>>()
        .join(", ");
    let cc = d
        .cc_json
        .iter()
        .map(format_addr)
        .collect::<Vec<_>>()
        .join(", ");
    let bcc = d
        .bcc_json
        .iter()
        .map(format_addr)
        .collect::<Vec<_>>()
        .join(", ");
    let msgid = format!("<{}@sift.local>", uuid::Uuid::now_v7());
    let date = chrono::Utc::now().to_rfc2822();
    let text = crate::render::text::html_to_text(&d.body_html);
    let boundary_mixed = format!("mixed-{}", uuid::Uuid::now_v7().simple());
    let boundary_alt = format!("alt-{}", uuid::Uuid::now_v7().simple());
    let mut headers = format!("From: me\r\nTo: {to}\r\nSubject: {}\r\nDate: {date}\r\nMessage-ID: {msgid}\r\nMIME-Version: 1.0\r\n", d.subject);
    if !cc.is_empty() {
        headers.push_str(&format!("Cc: {cc}\r\n"));
    }
    if !bcc.is_empty() {
        headers.push_str(&format!("Bcc: {bcc}\r\n"));
    }
    // In-Reply-To/References would be filled from parent thread in full impl (Phase 7 reply path passes them via draft.thread_id lookup)

    let body: String = if d.attachments_json.is_empty() {
        headers.push_str(&format!(
            "Content-Type: multipart/alternative; boundary=\"{boundary_alt}\"\r\n\r\n"
        ));
        format!("--{boundary_alt}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n--{boundary_alt}\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{}\r\n--{boundary_alt}--\r\n", d.body_html)
    } else {
        headers.push_str(&format!(
            "Content-Type: multipart/mixed; boundary=\"{boundary_mixed}\"\r\n\r\n"
        ));
        let mut parts = format!("--{boundary_mixed}\r\nContent-Type: multipart/alternative; boundary=\"{boundary_alt}\"\r\n\r\n--{boundary_alt}\r\nContent-Type: text/plain; charset=utf-8\r\n\r\n{text}\r\n--{boundary_alt}\r\nContent-Type: text/html; charset=utf-8\r\n\r\n{}\r\n--{boundary_alt}--\r\n", d.body_html);
        for a in &d.attachments_json {
            let bytes = std::fs::read(&a.path).unwrap_or_default();
            let enc = base64::engine::general_purpose::STANDARD.encode(&bytes);
            // RFC2231 filename* for non-ascii
            parts.push_str(&format!("--{boundary_mixed}\r\nContent-Type: {}; name=\"{}\"\r\nContent-Disposition: attachment; filename=\"{}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n{enc}\r\n", a.mime, a.name, a.name));
        }
        parts.push_str(&format!("--{boundary_mixed}--\r\n"));
        parts
    };
    let full = format!("{headers}\r\n{body}");
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(full.as_bytes()))
}

fn format_addr(a: &crate::dto::Address) -> String {
    match &a.n {
        Some(n) if !n.is_empty() => format!("\"{n}\" <{}>", a.e),
        _ => a.e.clone(),
    }
}
