use crate::app_state::AppState;
use crate::db::threads::page_identity;
use crate::dto::{
    SavedSearch, SavedSearchCount, SavedSearchInput, SearchPage, UnsubscribeResult, View,
};
use crate::errors::SiftError;
use crate::search::{cursor, local, query};
use crate::unsubscribe::{self, UnsubPlan};
use tauri::State;

/// Largest page any list or search will return.
const MAX_LIMIT: i64 = 100;

fn db_error(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

/// One page of search results (P7.1/P7.3).
///
/// `scope = "local"` runs the compiled query against the local store;
/// `scope = "server"` asks each account's provider and hydrates what it
/// returns. A provider failure is reported per account instead of being
/// swallowed, so a unified search can say its result is partial.
#[tauri::command]
pub async fn search(
    state: State<'_, AppState>,
    account_ids: Vec<String>,
    q: String,
    scope: String,
    cursor: Option<String>,
    limit: Option<i64>,
) -> Result<SearchPage, SiftError> {
    let limit = limit.unwrap_or(MAX_LIMIT).clamp(1, MAX_LIMIT);
    let parsed = query::parse(&q);
    let view = View::Search { q: q.clone() };
    let (sort, scope_key, query_fingerprint) = page_identity(&view, &account_ids);
    let decoded = match cursor.as_deref() {
        Some(raw) => Some(cursor::expect(raw, sort, &scope_key, &query_fingerprint)?),
        None => None,
    };
    if scope == "server" {
        return server_search(&state, &account_ids, &q, limit).await;
    }
    let page = local::search_page(
        &state.db,
        &account_ids,
        &parsed,
        decoded.as_ref(),
        &scope_key,
        limit,
    )
    .await
    .map_err(db_error)?;
    Ok(SearchPage {
        rows: page.rows,
        next_cursor: page.next_cursor,
        total: None,
        generation: 0,
        hints: parsed.hints,
        query: q,
        failed_accounts: Vec::new(),
        local_only: true,
    })
}

/// Server search: ask each provider, hydrate everything in one statement, and
/// report the accounts that failed. It is a single page — the provider decides
/// how much it returns — so a client must not keep paging it with a cursor.
async fn server_search(
    state: &AppState,
    account_ids: &[String],
    q: &str,
    limit: i64,
) -> Result<SearchPage, SiftError> {
    let mut pairs: Vec<(String, String, i64)> = Vec::new();
    let mut failed_accounts: Vec<String> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let sink = crate::provider::DbSink::new(state.db.clone());
    for account_id in account_ids {
        let provider = match state.provider_for(account_id).await {
            Ok(provider) => provider,
            Err(_) => {
                failed_accounts.push(account_id.clone());
                continue;
            }
        };
        match provider.server_search(q, limit as u32, &sink).await {
            Ok(refs) => {
                for reference in refs {
                    if seen.insert((account_id.clone(), reference.thread_id.clone())) {
                        pairs.push((account_id.clone(), reference.thread_id.clone(), 0));
                    }
                }
            }
            Err(_) => failed_accounts.push(account_id.clone()),
        }
    }
    let rows = state
        .db
        .read(move |conn| local::hydrate(conn, &pairs))
        .await
        .map_err(db_error)?;
    Ok(SearchPage {
        rows,
        next_cursor: None,
        total: None,
        generation: 0,
        hints: Vec::new(),
        query: q.to_string(),
        failed_accounts,
        local_only: false,
    })
}

// ---------------------------------------------------------------------------
// Saved searches (P7.4)
// ---------------------------------------------------------------------------

/// Create or update a saved search. The query is validated with the same
/// compiler the ad hoc search uses, so a saved mailbox and a typed query
/// always agree.
#[tauri::command]
pub async fn saved_search_upsert(
    state: State<'_, AppState>,
    search: SavedSearchInput,
) -> Result<SavedSearch, SiftError> {
    state
        .db
        .saved_search_upsert(search)
        .await
        .map_err(|e| SiftError::typed("invalid_saved_search", e.to_string(), serde_json::json!({})))
}

/// Delete a saved search. Only the saved query is removed; no message, thread,
/// label or queued operation is touched.
#[tauri::command]
pub async fn saved_search_delete(state: State<'_, AppState>, id: String) -> Result<(), SiftError> {
    state
        .db
        .saved_search_delete(&id)
        .await
        .map_err(db_error)
}

/// List saved searches with their scope, plus the entries of that scope whose
/// account no longer exists. Nothing is counted here: a count is a separate,
/// explicit request.
#[tauri::command]
pub async fn saved_search_list(state: State<'_, AppState>) -> Result<Vec<SavedSearch>, SiftError> {
    state.db.saved_search_list().await.map_err(db_error)
}

/// Count a saved search on demand, when its mailbox becomes visible.
#[tauri::command]
pub async fn saved_search_count(
    state: State<'_, AppState>,
    id: String,
) -> Result<SavedSearchCount, SiftError> {
    state
        .db
        .saved_search_count(&id)
        .await
        .map_err(db_error)
}

/// Run a saved search. It expands to the stored query and scope only — there
/// is no provider call and no background work.
#[tauri::command]
pub async fn saved_search_open(
    state: State<'_, AppState>,
    id: String,
    cursor: Option<String>,
    limit: Option<i64>,
) -> Result<SearchPage, SiftError> {
    let saved = state
        .db
        .saved_search_get(&id)
        .await
        .map_err(db_error)?
        .ok_or_else(|| SiftError::NotFound("saved search".into()))?;
    search(
        state,
        saved.account_scope,
        saved.query,
        "local".into(),
        cursor,
        limit,
    )
    .await
}

// ---------------------------------------------------------------------------
// Unsubscribe (P9.4)
// ---------------------------------------------------------------------------

/// `(list_unsubscribe, list_unsubscribe_post value, auth_results, auth
/// trusted, from_email)` for one message.
type UnsubRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    Option<String>,
);

/// Read what the message knows about unsubscribing, account-scoped.
async fn unsub_headers(
    db: &crate::db::Db,
    account_id: &str,
    message_id: &str,
) -> Result<unsubscribe::UnsubHeaders, SiftError> {
    let (account, message) = (account_id.to_string(), message_id.to_string());
    let row: Option<UnsubRow> = db
        .read(move |c| {
            let mut statement = c.prepare(
                "SELECT list_unsubscribe, list_unsubscribe_post_value, auth_results, \
                 auth_results_trusted, from_email FROM messages WHERE account_id=? AND id=?",
            )?;
            let mut rows = statement.query(rusqlite::params![account, message])?;
            match rows.next()? {
                Some(row) => Ok(Some((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))),
                None => Ok(None),
            }
        })
        .await
        .map_err(db_error)?;
    let Some((header, post_value, auth_results, trusted, from_email)) = row else {
        return Err(SiftError::NotFound("message".into()));
    };
    Ok(unsubscribe::UnsubHeaders {
        targets: header
            .as_deref()
            .map(unsubscribe::parse_targets)
            .unwrap_or_default(),
        post_value,
        auth_results,
        trusted: trusted != 0,
        from_email: from_email.unwrap_or_default(),
    })
}

/// Unsubscribe from one message (P9.4).
///
/// This is the user's initiation — it only runs from a click — and it only
/// POSTs when [`unsubscribe::plan`] says the message really advertises
/// one-click with provider-backed authentication evidence. Every other case
/// returns a labelled link or `mailto:` instead, so Sift never makes an
/// unintended request on the user's behalf.
pub async fn unsubscribe_for(
    db: &crate::db::Db,
    account_id: &str,
    message_id: &str,
) -> Result<UnsubscribeResult, SiftError> {
    let headers = unsub_headers(db, account_id, message_id).await?;
    let targets = headers.targets.clone();
    let (url, host) = match unsubscribe::plan(&headers) {
        UnsubPlan::Fallback(result) => return Ok(result),
        UnsubPlan::OneClick { url, host } => (url, host),
    };
    let parsed = match url::Url::parse(&url) {
        Ok(parsed) => parsed,
        Err(_) => {
            return Ok(unsubscribe::fallback_result(
                &targets,
                Some("malformed".into()),
                None,
                Some("This unsubscribe link is not a URL Sift can use.".into()),
            ))
        }
    };
    let host = unsubscribe::host_key(&parsed).unwrap_or(host);
    let address = match unsubscribe::resolve_public(&parsed).await {
        Ok(address) => address,
        Err(error) => {
            return Ok(unsubscribe::fallback_result(
                &targets,
                Some(error.reason().into()),
                None,
                Some(error.detail()),
            ))
        }
    };
    let client = match unsubscribe::one_click_client(&host, address) {
        Ok(client) => client,
        Err(error) => {
            return Ok(unsubscribe::fallback_result(
                &targets,
                Some("post_failed".into()),
                None,
                Some(format!("Sift could not prepare the request: {error}")),
            ))
        }
    };
    let outcome = unsubscribe::perform_one_click(&client, &parsed).await;
    if outcome.delivered {
        return Ok(UnsubscribeResult {
            method: "one_click".into(),
            done: true,
            url: None,
            mailto: None,
            reason: None,
            status: outcome.status,
            detail: Some(outcome.detail),
            targets,
        });
    }
    Ok(unsubscribe::fallback_result(
        &targets,
        outcome.reason,
        outcome.status,
        Some(outcome.detail),
    ))
}

#[tauri::command]
pub async fn unsubscribe(
    state: State<'_, AppState>,
    account_id: String,
    message_id: String,
) -> Result<UnsubscribeResult, SiftError> {
    unsubscribe_for(&state.db, &account_id, &message_id).await
}
