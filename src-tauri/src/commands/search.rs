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
    // server scope: messages.list?q= then hydrate
    let mut rows: Vec<ThreadRow> = vec![];
    let mut seen = std::collections::HashSet::new();
    for aid in &account_ids {
        let client = state.client_for(aid).await?;
        let resp = client.list_messages(None, Some(&q), false).await?;
        let ids: Vec<String> = resp
            .messages
            .unwrap_or_default()
            .into_iter()
            .map(|m| m.id)
            .collect();
        // hydrate unknown
        for id in &ids {
            let exists: bool = state
                .db
                .read({
                    let mid = id.clone();
                    move |c| {
                        Ok(c.query_row(
                            "SELECT EXISTS(SELECT 1 FROM messages WHERE id=?)",
                            rusqlite::params![mid],
                            |r| r.get(0),
                        )
                        .unwrap_or(false))
                    }
                })
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            if !exists {
                if let Ok(m) = client.get_message_meta(id).await {
                    let labels = m.label_ids.clone().unwrap_or_default();
                    let up = crate::db::messages::MsgUpsert {
                        id: id.clone(),
                        account_id: aid.clone(),
                        thread_id: m.thread_id.clone(),
                        history_id: m.history_id.clone(),
                        internal_date: m
                            .internal_date
                            .as_deref()
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(0),
                        subject: String::new(),
                        snippet: m.snippet.clone().unwrap_or_default(),
                        is_unread: labels.contains(&"UNREAD".into()),
                        is_starred: labels.contains(&"STARRED".into()),
                        is_draft: labels.contains(&"DRAFT".into()),
                        label_ids: labels,
                        ..Default::default()
                    };
                    let _ = state.db.messages_upsert(up).await;
                }
            }
        }
        // map to threads
        for id in ids {
            if let Some((a, t)) = state
                .db
                .message_thread(&id)
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?
            {
                if seen.insert((a.clone(), t.clone())) {
                    if let Some(mut r) = super::threads::thread_row_for(&state.db, &a, &t).await? {
                        // mark serverOnly if it was just hydrated? simplified: false
                        r.server_only = false;
                        rows.push(r);
                    }
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
