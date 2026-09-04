use crate::app_state::AppState;
use crate::dto::{ThreadRow, ThreadsPage};
use crate::errors::SiftError;
use tauri::State;

#[tauri::command]
pub async fn search(
    state: State<'_, AppState>,
    account_ids: Vec<String>,
    q: String,
    scope: String,
    cursor: Option<String>,
) -> Result<ThreadsPage, SiftError> {
    let parsed = crate::search::query::parse(&q);
    if scope == "local" {
        let pairs = crate::search::local::search(&state.db, &account_ids, &parsed, 100)
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        let mut rows = vec![];
        for (aid, tid) in pairs {
            if let Some(r) = super::threads::thread_row_for(&state.db, &aid, &tid).await? {
                rows.push(r);
            }
        }
        return Ok(ThreadsPage {
            rows,
            next_cursor: cursor,
            total: None,
            generation: 0,
        });
    }
    // server scope: provider search hydrates + indexes hits (second run local)
    let mut rows: Vec<ThreadRow> = vec![];
    let mut seen = std::collections::HashSet::new();
    let sink = crate::provider::DbSink::new(state.db.clone());
    for aid in &account_ids {
        let provider = state.provider_for(aid).await?;
        let refs = provider.server_search(&q, 100, &sink).await?;
        for r in refs {
            if seen.insert((aid.clone(), r.thread_id.clone())) {
                if let Some(mut row) =
                    super::threads::thread_row_for(&state.db, aid, &r.thread_id).await?
                {
                    // mark serverOnly if it was just hydrated? simplified: false
                    row.server_only = false;
                    rows.push(row);
                }
            }
        }
    }
    rows.sort_by_key(|a| std::cmp::Reverse(a.last_message_at));
    rows.truncate(100);
    Ok(ThreadsPage {
        rows,
        next_cursor: None,
        total: None,
        generation: 0,
    })
}

#[tauri::command]
pub async fn unsubscribe(
    state: State<'_, AppState>,
    message_id: String,
) -> Result<serde_json::Value, SiftError> {
    let (list_url, mailto, one_click): (Option<String>, Option<String>, bool) = state
        .db
        .read({
            let mid = message_id.clone();
            move |c| {
                let (lu, post): (Option<String>, i64) = c
                    .query_row(
                        "SELECT list_unsubscribe, list_unsubscribe_post FROM messages WHERE id=?",
                        rusqlite::params![mid],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )
                    .unwrap_or((None, 0));
                Ok((
                    lu.as_deref().and_then(|s| {
                        s.split(',').find(|p| p.contains("http")).map(|p| {
                            p.trim()
                                .trim_matches(|cc| cc == '<' || cc == '>')
                                .to_string()
                        })
                    }),
                    lu.as_deref().and_then(|s| {
                        s.split(',').find(|p| p.contains("mailto:")).map(|p| {
                            p.trim()
                                .trim_matches(|cc| cc == '<' || cc == '>')
                                .to_string()
                        })
                    }),
                    post != 0,
                ))
            }
        })
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    if one_click {
        if let Some(url) = list_url.clone() {
            let r = state
                .http
                .post(url)
                .body("List-Unsubscribe=One-Click")
                .send()
                .await;
            if r.map(|x| x.status().is_success()).unwrap_or(false) {
                return Ok(serde_json::json!({"method":"post","done":true}));
            }
        }
    }
    if let Some(m) = mailto {
        return Ok(serde_json::json!({"method":"mailto","done":false, "to": m}));
    }
    if let Some(u) = list_url {
        return Ok(serde_json::json!({"method":"url","done":false, "url": u}));
    }
    Ok(serde_json::json!({"method":"url","done":false}))
}
