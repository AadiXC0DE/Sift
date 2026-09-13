use crate::app_state::AppState;
use crate::dto::{Label, SyncStatus};
use crate::errors::SiftError;
use tauri::{AppHandle, Emitter, Manager, State};

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
        // The manual sync belongs to the account's current generation: a
        // removal (or reauth) cancels it, and its late results are dropped
        // instead of being written for an account that is gone.
        let cancel = match state.runtime_state(&aid).await {
            Some((_, cancel)) => cancel,
            None => tokio_util::sync::CancellationToken::new(),
        };
        // A manual refresh is one trigger on the account's coordinator (P4.3):
        // if a sync tick is already running this request is coalesced into its
        // single follow-up pass instead of starting a second concurrent run.
        let coordinator = state.coordinator_for(&aid).await;
        let db = state.db.clone();
        let aid2 = aid.clone();
        let app2 = app.clone();
        tokio::spawn(async move {
            coordinator
                .tick_now(|| manual_refresh(db.clone(), app2.clone(), aid2.clone(), provider.clone(), cancel.clone()))
                .await;
        });
    }
    Ok(())
}

/// The refresh one manual trigger performs; identical to a runtime tick's
/// partial pass, including NeedsFull recovery.
async fn manual_refresh(
    db: crate::db::Db,
    app: AppHandle,
    aid: String,
    provider: std::sync::Arc<dyn crate::provider::Provider>,
    cancel: tokio_util::sync::CancellationToken,
) {
    use crate::provider::{Cursor, PartialOutcome};
    let aid2 = aid.clone();
    let raw: Option<String> = db
        .accounts_get(&aid)
        .await
        .ok()
        .flatten()
        .and_then(|a| a.history_id);
    let cursor = Cursor::parse(raw.as_deref().unwrap_or(""));
    let sink = crate::provider::DbSink::with_progress(db.clone(), {
        let app = app.clone();
        move |s| {
            let _ = app.emit("sync:state", &s);
        }
    });
    let outcome = tokio::select! {
        _ = cancel.cancelled() => return,
        outcome = provider.partial_sync(&cursor, &sink) => outcome,
    };
    // Manual sync recovers like the poll loop (reconcile REST,
    // rebuild IMAP) instead of surfacing cursor internals.
    let outcome = match outcome {
        Ok(PartialOutcome::NeedsFull) => {
            if provider.kind() == crate::provider::ProviderKind::GmailImap {
                match tokio::select! {
                    _ = cancel.cancelled() => return,
                    synced = provider.full_sync(&sink, cancel.clone()) => synced,
                } {
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
                let empty = Cursor::Gmail {
                    history_id: String::new(),
                };
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    reconciled = provider.partial_sync(&empty, &sink) => reconciled,
                }
            }
        }
        other => other,
    };
    if cancel.is_cancelled() {
        return;
    }
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
                let _ = app.emit(
                    "store:threads",
                    serde_json::json!({ "account_id": aid2, "thread_ids": tids }),
                );
            }
            let _ = app.emit("store:labels", serde_json::json!({ "account_id": aid2 }));
            let _ = app.emit(
                "sync:state",
                serde_json::json!({ "account_id": aid2, "phase": "done", "done": 0, "total": 0, "total_known": false, "last_error": null }),
            );
        }
        Ok(_) => {}
        Err(e) => {
            let _ = app.emit(
                "sync:state",
                serde_json::json!({ "account_id": aid2, "phase": "error", "done": 0, "total": 0, "total_known": false, "last_error": e.to_string() }),
            );
        }
    }
}

/// The host's reachability hint (P4.6): the frontend bridges
/// `navigator.onLine`/`online`/`offline` here, and startup reports its
/// current value. A hint alone never proves Gmail is reachable, so recovery
/// re-runs one real sync tick per account instead of trusting it.
#[tauri::command]
pub async fn app_network_hint(
    app: AppHandle,
    state: State<'_, AppState>,
    online: bool,
) -> Result<(), SiftError> {
    let changed = state.connectivity.set_network_reachable(online);
    if let Ok(list) = state.db.accounts_list().await {
        for a in &list {
            state.emit_connectivity(&a.id);
        }
    }
    if online && changed {
        crate::runtime::resume_after_reconnect(app);
    }
    Ok(())
}

/// Per-account connectivity (state, last successful sync, last error).
#[tauri::command]
pub async fn connectivity_state(
    state: State<'_, AppState>,
) -> Result<Vec<crate::connectivity::ConnectivityState>, SiftError> {
    let ids: Vec<String> = state
        .db
        .accounts_list()
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
        .into_iter()
        .map(|a| a.id)
        .collect();
    Ok(state
        .connectivity
        .snapshot(&ids, crate::db::now_ms()))
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
            // The stored state is a phase, not a count: never claim a total.
            total_known: false,
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
    let _ = state;
    // Unread Inbox only, hidden at zero. This is the native macOS dock badge;
    // the OS styles it, so keep the number meaningful rather than loud.
    if let Some(w) = app.get_webview_window("main") {
        let value = if count > 0 { Some(count) } else { None };
        let _ = w.set_badge_count(value);
    }
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
    // A real Google desktop client id looks like
    // "<project-number>-<random>.apps.googleusercontent.com". Placeholders
    // (empty, "replace-me", a bare string) must not enable the OAuth button,
    // otherwise the browser opens Google's "invalid_client" error page.
    fn looks_real(id: &str) -> bool {
        let id = id.trim();
        id.ends_with(".apps.googleusercontent.com")
            && id
                .split('-')
                .next()
                .map(|prefix| !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()))
                .unwrap_or(false)
    }
    let oauth_available = std::env::var("SIFT_GOOGLE_CLIENT_ID")
        .ok()
        .filter(|s| looks_real(s))
        .is_some()
        || option_env!("SIFT_GOOGLE_CLIENT_ID")
            .map(looks_real)
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
