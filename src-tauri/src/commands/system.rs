use crate::app_state::AppState;
use crate::dto::{Label, SyncStatus};
use crate::errors::SiftError;
use tauri::State;

#[tauri::command]
pub async fn sync_now(
    state: State<'_, AppState>,
    account_id: Option<String>,
) -> Result<(), SiftError> {
    // Trigger partial sync for one or all accounts
    let ids: Vec<String> = match account_id {
        Some(a) => vec![a],
        None => state
            .db
            .accounts_list()
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?
            .into_iter()
            .map(|a| a.id)
            .collect(),
    };
    for aid in ids {
        let client = match state.client_for(&aid).await {
            Ok(c) => c,
            Err(_) => continue,
        };
        let db = state.db.clone();
        let aid2 = aid.clone();
        tokio::spawn(async move {
            let _ = crate::sync::partial::run_partial_sync(&db, &aid2, &client).await;
        });
    }
    Ok(())
}

#[tauri::command]
pub async fn sync_status(state: State<'_, AppState>) -> Result<Vec<SyncStatus>, SiftError> {
    let accs = state
        .db
        .accounts_list()
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    Ok(accs
        .into_iter()
        .map(|a| SyncStatus {
            account_id: a.id,
            phase: a.sync_state,
            done: 0,
            total: 0,
            last_error: None,
        })
        .collect())
}

#[tauri::command]
pub async fn labels_list(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<Vec<Label>, SiftError> {
    state
        .db
        .labels_list(&account_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))
}

#[tauri::command]
pub async fn app_set_badge(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
    count: i64,
) -> Result<(), SiftError> {
    #[cfg(target_os = "macos")]
    {
        let _ = app;
        let _ = count;
        // badge via dock plugin would go here; use private API through tauri? keep as no-op with log
    }
    let _ = state;
    Ok(())
}

#[tauri::command]
pub async fn app_open_url(app: tauri::AppHandle, url: String) -> Result<(), SiftError> {
    let lower = url.to_lowercase();
    if !(lower.starts_with("http://")
        || lower.starts_with("https://")
        || lower.starts_with("mailto:"))
    {
        return Err(SiftError::app("bad_url", "scheme not allowed", false));
    }
    crate::opener::open(&app, &url).map_err(|e| SiftError::app("open", e.to_string(), false))
}

#[tauri::command]
pub async fn diagnostics_export(
    state: State<'_, AppState>,
) -> Result<serde_json::Value, SiftError> {
    use std::io::Write;
    let dir = state.data_dir.join("diagnostics");
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(format!("sift-diag-{}.zip", crate::db::now_ms()));
    let file = std::fs::File::create(&path).map_err(SiftError::from)?;
    let mut zip = zip::ZipWriter::new(file);
    // sync_log + counts (no content)
    let log: Vec<(i64, String, String)> = state
        .db
        .read(|c| -> anyhow::Result<Vec<(i64, String, String)>> {
            let mut s = c.prepare(
                "SELECT at, kind, COALESCE(detail,'') FROM sync_log ORDER BY id DESC LIMIT 200",
            )?;
            let v: Vec<(i64, String, String)> = s
                .query_map([], |r| {
                    Ok::<(i64, String, String), rusqlite::Error>((r.get(0)?, r.get(1)?, r.get(2)?))
                })?
                .collect::<Result<Vec<(i64, String, String)>, rusqlite::Error>>()?;
            Ok(v)
        })
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    let counts: String = state
        .db
        .read(|c| -> anyhow::Result<String> {
            let mut out = String::new();
            for t in ["accounts", "threads", "messages", "outbox_ops"] {
                let n: i64 = c
                    .query_row(&format!("SELECT count(*) FROM {t}"), [], |r| r.get(0))
                    .unwrap_or(0);
                out.push_str(&format!("{t}={n}\n"));
            }
            Ok(out)
        })
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    // redaction check: ensure no emails in export (we only export counts + kinds)
    let body = format!(
        "counts:\n{counts}\nsync_log:\n{}",
        log.iter()
            .map(|(a, k, d)| format!("{a} {k} {d}"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    // scan for email pattern
    assert_no_pii(&body)?;
    zip.start_file("diag.txt", zip::write::SimpleFileOptions::default())
        .map_err(|e| SiftError::app("zip", e.to_string(), false))?;
    zip.write_all(body.as_bytes()).map_err(SiftError::from)?;
    zip.finish()
        .map_err(|e| SiftError::app("zip", e.to_string(), false))?;
    Ok(serde_json::json!({"path": path.to_string_lossy()}))
}

fn assert_no_pii(s: &str) -> Result<(), SiftError> {
    // crude email regex + token check
    if s.contains("ya29.") || s.contains("1//") {
        return Err(SiftError::app("pii", "tokens in diagnostics", false));
    }
    // allow sync kinds but not addresses: fail if contains @ with dot
    for tok in s.split_whitespace() {
        if tok.contains('@') && tok.contains('.') && !tok.starts_with("kind") {
            // sync detail should never contain addresses; our detail strings don't
            return Err(SiftError::app(
                "pii",
                format!("address in diagnostics: {tok}"),
                false,
            ));
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn perf_mark(state: State<'_, AppState>, name: String) -> Result<(), SiftError> {
    eprintln!("perf:{name} {}", crate::db::now_ms());
    if name == "first-paint" {
        state
            .first_paint_at
            .store(crate::db::now_ms(), std::sync::atomic::Ordering::SeqCst);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn p9_t05_badge_counts() {
        // badge = unread inbox threads across included accounts
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(dir.path()).unwrap();
        let a = db.new_account("a@x.com", None, None).await.unwrap();
        db.messages_upsert(crate::db::messages::MsgUpsert {
            id: "m1".into(),
            account_id: a.id.clone(),
            thread_id: "t1".into(),
            internal_date: 1,
            is_unread: true,
            label_ids: vec!["INBOX".into(), "UNREAD".into()],
            ..Default::default()
        })
        .await
        .unwrap();
        let unread: i64 = db.read({
      let aid = a.id.clone();
      move |c| Ok(c.query_row("SELECT count(*) FROM threads WHERE account_id=? AND in_inbox=1 AND unread_count>0", rusqlite::params![aid], |r| r.get(0))?)
    }).await.unwrap();
        assert_eq!(unread, 1);
    }
    #[test]
    fn p10_t05_diag_no_pii() {
        assert!(assert_no_pii("counts:\nthreads=3\nsync_log:\n1 full done").is_ok());
        assert!(assert_no_pii("to ada@x.com").is_err());
    }
}
