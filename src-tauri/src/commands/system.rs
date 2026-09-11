use crate::app_state::AppState;
use crate::dto::{Label, SyncStatus};
use crate::errors::SiftError;
use tauri::{AppHandle, Emitter, State};

#[tauri::command]
pub async fn sync_now(
    app: AppHandle,
    state: State<'_, AppState>,
    account_id: Option<String>,
) -> Result<(), SiftError> {
    // Demo mode has no server: stay local.
    if crate::demo::is_demo() {
        return Ok(());
    }
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
        let provider = match state.provider_for(&aid).await {
            Ok(c) => c,
            Err(_) => continue,
        };
        let db = state.db.clone();
        let aid2 = aid.clone();
        let app2 = app.clone();
        tokio::spawn(async move {
            use crate::provider::{Cursor, PartialOutcome};
            let raw: Option<String> = db
                .accounts_get(&aid2)
                .await
                .ok()
                .flatten()
                .and_then(|a| a.history_id);
            let cursor = Cursor::parse(raw.as_deref().unwrap_or(""));
            let sink = crate::provider::DbSink::with_progress(db.clone(), {
                let app = app2.clone();
                move |s| {
                    let _ = app.emit("sync:state", &s);
                }
            });
            let outcome = provider.partial_sync(&cursor, &sink).await;
            // Manual sync recovers like the poll loop (reconcile REST,
            // rebuild IMAP) instead of surfacing cursor internals.
            let outcome = match outcome {
                Ok(PartialOutcome::NeedsFull) => {
                    if provider.kind() == crate::provider::ProviderKind::GmailImap {
                        let cancel = tokio_util::sync::CancellationToken::new();
                        match provider.full_sync(&sink, cancel).await {
                            Ok(c) => {
                                let _ = db
                                    .accounts_set_history(&aid2, &c.render(), crate::db::now_ms())
                                    .await;
                                Ok(PartialOutcome::Synced {
                                    changed_threads: vec![],
                                    new_inbox: vec![],
                                })
                            }
                            Err(e) => Err(e),
                        }
                    } else {
                        // REST providers reconcile internally on an empty
                        // cursor, then continue from the fresh history id.
                        provider
                            .partial_sync(
                                &Cursor::Gmail {
                                    history_id: String::new(),
                                },
                                &sink,
                            )
                            .await
                    }
                }
                other => other,
            };
            match outcome {
                Ok(PartialOutcome::Synced {
                    changed_threads, ..
                }) => {
                    let _ = db.accounts_set_state(&aid2, "partial").await;
                    let tids: Vec<String> = changed_threads
                        .iter()
                        .filter(|(a, _)| a == &aid2)
                        .map(|(_, t)| t.clone())
                        .collect();
                    if !tids.is_empty() {
                        let _ = app2.emit(
                            "store:threads",
                            serde_json::json!({ "account_id": aid2, "thread_ids": tids }),
                        );
                    }
                    let _ = app2.emit("store:labels", serde_json::json!({ "account_id": aid2 }));
                    let _ = app2.emit(
                        "sync:state",
                        serde_json::json!({ "account_id": aid2, "phase": "done", "done": 0, "total": 0, "last_error": null }),
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    let _ = app2.emit(
                        "sync:state",
                        serde_json::json!({ "account_id": aid2, "phase": "error", "done": 0, "total": 0, "last_error": e.to_string() }),
                    );
                }
            }
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
    let mut labels = state
        .db
        .labels_list(&account_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    // Live counts from the threads table (spec 6.2): threads carrying the
    // label, excluding trash/spam; unread counts threads with unread > 0.
    let counts: std::collections::HashMap<String, (i64, i64)> = state
        .db
        .read({
            let aid = account_id.clone();
            move |c| -> anyhow::Result<std::collections::HashMap<String, (i64, i64)>> {
                let mut s = c.prepare(
                    "SELECT value, COUNT(*), SUM(CASE WHEN t.unread_count>0 THEN 1 ELSE 0 END)                      FROM threads t, json_each(t.label_ids)                      WHERE t.account_id=? AND t.in_trash=0 AND t.in_spam=0 GROUP BY value",
                )?;
                let v: Vec<(String, i64, i64)> = s
                    .query_map(rusqlite::params![aid], |r| {
                        Ok::<(String, i64, i64), rusqlite::Error>((r.get(0)?, r.get(1)?, r.get(2)?))
                    })?
                    .collect::<Result<Vec<(String, i64, i64)>, rusqlite::Error>>()?;
                Ok(v.into_iter().map(|(k, total, unread)| (k, (total, unread))).collect())
            }
        })
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    for l in &mut labels {
        let (total, unread) = counts.get(&l.id).copied().unwrap_or((0, 0));
        l.total_count = total;
        l.unread_count = unread;
    }
    Ok(labels)
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

/// Build info for the setup wizard (Step A button + CI P11-T21).
/// `oauth_available` is true only when a Google client ID was present at
/// build time (`SIFT_GOOGLE_CLIENT_ID` runtime or compile-time) - public
/// builds without it show the app-password path only.
#[tauri::command]
pub fn system_info() -> Result<serde_json::Value, SiftError> {
    let oauth_available = std::env::var("SIFT_GOOGLE_CLIENT_ID")
        .ok()
        .filter(|s| !s.is_empty())
        .is_some()
        || option_env!("SIFT_GOOGLE_CLIENT_ID")
            .map(|s| !s.is_empty())
            .unwrap_or(false);
    Ok(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "oauth_available": oauth_available,
        "demo": crate::demo::is_demo(),
    }))
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
