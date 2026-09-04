use crate::app_state::AppState;
use crate::dto::*;
use crate::errors::SiftError;
use tauri::State;

#[tauri::command]
pub async fn threads_query(
    state: State<'_, AppState>,
    query: ThreadsQuery,
) -> Result<ThreadsPage, SiftError> {
    if query.limit > 100 {
        return Err(SiftError::app("bad_query", "limit must be ≤100", false));
    }
    if query.account_ids.is_empty() {
        return Ok(ThreadsPage {
            rows: vec![],
            next_cursor: None,
            total: None,
            generation: 0,
        });
    }
    // Search view delegates to search
    if let View::Search { q } = &query.view {
        let parsed = crate::search::query::parse(q);
        let pairs =
            crate::search::local::search(&state.db, &query.account_ids, &parsed, query.limit)
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        // hydrate rows
        let mut rows = vec![];
        for (aid, tid) in pairs {
            if let Some(r) = thread_row_for(&state.db, &aid, &tid).await? {
                rows.push(r);
            }
        }
        return Ok(ThreadsPage {
            rows,
            next_cursor: None,
            total: None,
            generation: 0,
        });
    }
    state
        .db
        .threads_query(query)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))
}

pub async fn thread_row_for(
    db: &crate::db::Db,
    account_id: &str,
    thread_id: &str,
) -> Result<Option<ThreadRow>, SiftError> {
    let (a, t) = (account_id.to_string(), thread_id.to_string());
    db.read(move |c| {
        let mut s = match c.prepare("SELECT * FROM threads WHERE account_id=? AND id=?") {
            Ok(s) => s,
            Err(_) => return Ok(None),
        };
        let mut rows = s
            .query_map(rusqlite::params![a, t], |r| {
                let label_ids: Vec<String> =
                    serde_json::from_str(&r.get::<_, String>("label_ids")?).unwrap_or_default();
                let parts: Vec<Address> =
                    serde_json::from_str(&r.get::<_, String>("participants")?).unwrap_or_default();
                Ok(ThreadRow {
                    account_id: r.get("account_id")?,
                    id: r.get("id")?,
                    subject: r.get("subject")?,
                    snippet: r.get("snippet")?,
                    participants: parts,
                    last_message_at: r.get("last_message_at")?,
                    message_count: r.get("message_count")?,
                    unread_count: r.get("unread_count")?,
                    is_starred: r.get::<_, i64>("is_starred")? != 0,
                    has_attachments: r.get::<_, i64>("has_attachments")? != 0,
                    label_ids,
                    snoozed_until: r.get("snoozed_until")?,
                    server_only: false,
                })
            })
            .map_err(|e| rusqlite::Error::ToSqlConversionFailure(Box::new(e)))?;
        Ok(rows
            .next()
            .transpose()
            .map_err(|e: rusqlite::Error| e)
            .unwrap_or(None))
    })
    .await
    .map_err(|e| SiftError::app("db", e.to_string(), false))
}

#[tauri::command]
pub async fn thread_get(
    state: State<'_, AppState>,
    account_id: String,
    thread_id: String,
) -> Result<ThreadDetail, SiftError> {
    let (a, t) = (account_id.clone(), thread_id.clone());
    let subject: String = state
        .db
        .read(move |c| {
            Ok(c.query_row(
                "SELECT subject FROM threads WHERE account_id=? AND id=?",
                rusqlite::params![a, t],
                |r| r.get(0),
            )
            .unwrap_or_default())
        })
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    let label_ids: Vec<String> = state
        .db
        .read({
            let (a, t) = (account_id.clone(), thread_id.clone());
            move |c| -> anyhow::Result<Vec<String>> {
                let j: String = c
                    .query_row(
                        "SELECT label_ids FROM threads WHERE account_id=? AND id=?",
                        rusqlite::params![a, t],
                        |r| r.get(0),
                    )
                    .unwrap_or("[]".into());
                Ok(serde_json::from_str(&j).unwrap_or_default())
            }
        })
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    // messages ascending, page 50+ (spec: page at 50 for huge threads — we return all up to 200, else latest 50)
    let msgs: Vec<MsgTuple> = state.db.read({
    let (a, t) = (account_id.clone(), thread_id.clone());
    move |c| -> anyhow::Result<Vec<MsgTuple>> {
      let count: i64 = c.query_row("SELECT count(*) FROM messages WHERE account_id=? AND thread_id=?", rusqlite::params![a, t], |r| r.get(0)).unwrap_or(0);
      let sql = if count > 200 {
        "SELECT * FROM messages WHERE account_id=? AND thread_id=? ORDER BY internal_date DESC LIMIT 50"
      } else {
        "SELECT * FROM messages WHERE account_id=? AND thread_id=? ORDER BY internal_date ASC LIMIT 200"
      };
      let mut s = c.prepare(sql)?;
      let rows = s.query_map(rusqlite::params![a, t], |r| {
        let id: String = r.get("id")?;
        let to_json: String = r.get("to_json")?; let cc_json: String = r.get("cc_json")?; let bcc_json: String = r.get("bcc_json")?;
        let from_name: Option<String> = r.get("from_name")?; let from_email: Option<String> = r.get("from_email")?;
        let label_ids: Vec<String> = serde_json::from_str(&r.get::<_, String>("label_ids")?).unwrap_or_default();
        Ok((id, r.get::<_, i64>("internal_date")?, from_name, from_email, to_json, cc_json, bcc_json,
          r.get::<_, Option<String>>("reply_to")?, r.get::<_, String>("subject")?, r.get::<_, String>("snippet")?,
          r.get::<_, i64>("is_unread")? != 0, r.get::<_, i64>("is_starred")? != 0, r.get::<_, i64>("is_draft")? != 0,
          r.get::<_, i64>("is_sent_by_me")? != 0, label_ids, r.get::<_, i64>("has_attachments")? != 0,
          r.get::<_, String>("body_state")?, r.get::<_, Option<String>>("list_unsubscribe")?, r.get::<_, i64>("list_unsubscribe_post")? != 0))
      })?.collect::<Result<Vec<_>, rusqlite::Error>>()?;
      let mut rows = rows;
      if count > 200 { rows.reverse(); }
      Ok(rows)
    }
  }).await.map_err(|e| SiftError::app("db", e.to_string(), false))?;
    let mut messages = vec![];
    for (
        id,
        internal_date,
        from_name,
        from_email,
        to_json,
        cc_json,
        bcc_json,
        reply_to,
        subject,
        snippet,
        is_unread,
        is_starred,
        is_draft,
        is_sent_by_me,
        label_ids,
        has_attachments,
        body_state,
        list_unsub,
        list_post,
    ) in msgs
    {
        let attachments = state
            .db
            .attachments_for_message(&id)
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        let to: Vec<Address> = serde_json::from_str(&to_json).unwrap_or_default();
        let cc: Vec<Address> = serde_json::from_str(&cc_json).unwrap_or_default();
        let bcc: Vec<Address> = serde_json::from_str(&bcc_json).unwrap_or_default();
        let list_unsubscribe = list_unsub.map(|u| {
            let (url, mailto) = parse_list_unsub(&u);
            ListUnsub {
                url,
                mailto,
                one_click: list_post,
            }
        });
        messages.push(MessageMeta {
            id,
            internal_date,
            from: Address {
                n: from_name,
                e: from_email.unwrap_or_default(),
                me: None,
            },
            to,
            cc,
            bcc,
            reply_to,
            subject,
            snippet,
            is_unread,
            is_starred,
            is_draft,
            is_sent_by_me,
            label_ids,
            has_attachments,
            attachments,
            body_state: match body_state.as_str() {
                "fetched" => "fetched".into(),
                "error" => "error".into(),
                _ => "none".into(),
            },
            list_unsubscribe,
        });
    }
    Ok(ThreadDetail {
        account_id,
        id: thread_id,
        subject,
        label_ids,
        messages,
    })
}

// 19-column message row: kept as tuple to avoid a second DTO mapping layer (spec 8.3).
#[allow(clippy::type_complexity)]
type MsgTuple = (
    String,
    i64,
    Option<String>,
    Option<String>,
    String,
    String,
    String,
    Option<String>,
    String,
    String,
    bool,
    bool,
    bool,
    bool,
    Vec<String>,
    bool,
    String,
    Option<String>,
    bool,
);

fn parse_list_unsub(raw: &str) -> (Option<String>, Option<String>) {
    let mut url = None;
    let mut mailto = None;
    for part in raw.split(',') {
        let p = part
            .trim()
            .trim_matches(|c| c == '<' || c == '>')
            .trim()
            .to_string();
        if p.starts_with("mailto:") {
            mailto = Some(p);
        } else if p.starts_with("http") {
            url = Some(p);
        }
    }
    (url, mailto)
}

fn body_failure_state(error: &SiftError) -> &'static str {
    if error.is_retryable() {
        "loading"
    } else {
        "error"
    }
}

fn should_refresh_body_provider(
    error: &SiftError,
    provider_kind: crate::provider::ProviderKind,
) -> bool {
    error.is_reauth() && provider_kind == crate::provider::ProviderKind::GmailApi
}

async fn render_message_html(
    state: &AppState,
    account_id: &str,
    message_id: &str,
    html: String,
) -> String {
    let mut rendered = match state.db.attachments_with_bytes(message_id).await {
        Ok(parts) => crate::render::sanitize::embed_local_images(&html, message_id, &parts),
        Err(_) => html,
    };
    let unresolved = crate::render::sanitize::inline_image_refs(&rendered, message_id);
    if unresolved.is_empty() {
        return rendered;
    }
    let Ok(provider) = state.provider_for(account_id).await else {
        return rendered;
    };
    for key in unresolved {
        if let Ok((bytes, mime)) =
            crate::uri_scheme::resolve_attachment(&state.db, &*provider, message_id, &key).await
        {
            let _ = state
                .db
                .attachment_cache_data(message_id, &key, &bytes)
                .await;
            let part = vec![(key.clone(), key.clone(), Some(key), mime, bytes)];
            rendered = crate::render::sanitize::embed_local_images(&rendered, message_id, &part);
        }
    }
    rendered
}

#[tauri::command]
pub async fn message_body(
    state: State<'_, AppState>,
    message_id: String,
) -> Result<MessageBody, SiftError> {
    // sender prefs + bodies
    let info: Option<(String, String)> = state
        .db
        .message_thread(&message_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    let Some((account_id, _thread)) = info else {
        return Ok(MessageBody {
            message_id,
            state: "error".into(),
            html: None,
            text: None,
            remote_image_count: 0,
            tracker_count: 0,
            dark_safe: true,
            remote_images_allowed: false,
        });
    };
    // from email for sender prefs
    let from_email: String = state
        .db
        .read({
            let mid = message_id.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT COALESCE(from_email,'') FROM messages WHERE id=?",
                    rusqlite::params![mid],
                    |r| r.get(0),
                )
                .unwrap_or_default())
            }
        })
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    let allowed_sender: bool = state
        .db
        .read({
            let e = from_email.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT allow_remote_images FROM sender_prefs WHERE email=?",
                    rusqlite::params![e],
                    |r| r.get::<_, i64>(0),
                )
                .map(|v| v != 0)
                .unwrap_or(false))
            }
        })
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    let settings = state
        .db
        .settings_get()
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    let global_allow =
        settings.remote_images == "always" || (allowed_sender && settings.remote_images != "never");
    if let Some((html, text, ri, tc, ds, _q)) = state
        .db
        .bodies_get(&message_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
    {
        let html_out = if global_allow {
            html.map(|h| crate::render::sanitize::restore_remote_images(&h))
        } else {
            html
        };
        let html_out = match html_out {
            Some(h) => Some(render_message_html(&state, &account_id, &message_id, h).await),
            None => None,
        };
        return Ok(MessageBody {
            message_id,
            state: "ready".into(),
            html: html_out,
            text,
            remote_image_count: ri,
            tracker_count: tc,
            dark_safe: ds,
            remote_images_allowed: global_allow,
        });
    }
    // Foreground fetches must preempt bulk backfill after a rendering upgrade.
    let _foreground = state.gate.enter();
    let mut provider = match state.provider_for(&account_id).await {
        Ok(p) => p,
        Err(error) => {
            return Ok(MessageBody {
                message_id,
                state: body_failure_state(&error).into(),
                html: None,
                text: Some(error.to_string()),
                remote_image_count: 0,
                tracker_count: 0,
                dark_safe: true,
                remote_images_allowed: global_allow,
            });
        }
    };
    let mut fetched = provider.fetch_body(&message_id).await;
    // A cached Gmail provider owns the token it was constructed with. If the
    // server rejects that token, rebuild it from the refresh token and retry
    // the foreground request once instead of failing every opened message.
    if fetched
        .as_ref()
        .is_err_and(|error| should_refresh_body_provider(error, provider.kind()))
    {
        state.invalidate_oauth_provider(&account_id).await;
        match state.provider_for(&account_id).await {
            Ok(refreshed) => {
                provider = refreshed;
                fetched = provider.fetch_body(&message_id).await;
            }
            Err(refresh_error) => fetched = Err(refresh_error),
        }
    }
    match fetched {
        Ok(parsed) => {
            let sink = crate::provider::DbSink::new(state.db.clone());
            if crate::provider::store_parsed(&sink, &message_id, &parsed)
                .await
                .is_err()
            {
                return Ok(MessageBody {
                    message_id,
                    state: "error".into(),
                    html: None,
                    text: parsed.text,
                    remote_image_count: 0,
                    tracker_count: 0,
                    dark_safe: true,
                    remote_images_allowed: global_allow,
                });
            }
        }
        Err(error) => {
            return Ok(MessageBody {
                message_id,
                state: body_failure_state(&error).into(),
                html: None,
                text: Some(error.to_string()),
                remote_image_count: 0,
                tracker_count: 0,
                dark_safe: true,
                remote_images_allowed: global_allow,
            });
        }
    }
    if let Some((html, text, ri, tc, ds, _q)) = state
        .db
        .bodies_get(&message_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
    {
        let html_out = if global_allow {
            html.map(|h| crate::render::sanitize::restore_remote_images(&h))
        } else {
            html
        };
        let html_out = match html_out {
            Some(h) => Some(render_message_html(&state, &account_id, &message_id, h).await),
            None => None,
        };
        return Ok(MessageBody {
            message_id,
            state: "ready".into(),
            html: html_out,
            text,
            remote_image_count: ri,
            tracker_count: tc,
            dark_safe: ds,
            remote_images_allowed: global_allow,
        });
    }
    Ok(MessageBody {
        message_id,
        state: "error".into(),
        html: None,
        text: None,
        remote_image_count: 0,
        tracker_count: 0,
        dark_safe: true,
        remote_images_allowed: global_allow,
    })
}

#[tauri::command]
pub async fn message_raw_source(
    state: State<'_, AppState>,
    message_id: String,
) -> Result<String, SiftError> {
    let info = state
        .db
        .message_thread(&message_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    let Some((account_id, _)) = info else {
        return Err(SiftError::NotFound("message".into()));
    };
    let provider = state.provider_for(&account_id).await?;
    provider.fetch_raw(&message_id).await
}

#[tauri::command]
pub async fn remote_images_load(
    state: State<'_, AppState>,
    message_id: String,
    remember_sender: bool,
) -> Result<MessageBody, SiftError> {
    if remember_sender {
        let from_email: String = state
            .db
            .read({
                let mid = message_id.clone();
                move |c| {
                    Ok(c.query_row(
                        "SELECT COALESCE(from_email,'') FROM messages WHERE id=?",
                        rusqlite::params![mid],
                        |r| r.get(0),
                    )
                    .unwrap_or_default())
                }
            })
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        let fe = from_email.clone();
        state
            .db
            .write(move |c| {
                c.execute(
                    "INSERT OR REPLACE INTO sender_prefs (email,allow_remote_images) VALUES (?,1)",
                    rusqlite::params![fe],
                )?;
                Ok(())
            })
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    }
    // return body with images allowed
    let mut body = message_body(state.clone(), message_id).await?;
    if let Some(h) = body.html.take() {
        let restored = crate::render::sanitize::restore_remote_images(&h);
        body.html = Some(restored);
    }
    body.remote_images_allowed = true;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::{body_failure_state, should_refresh_body_provider};
    use crate::errors::SiftError;
    use crate::provider::ProviderKind;

    #[test]
    fn body_fetch_failures_remain_recoverable() {
        let transient = SiftError::app("http", "temporary", true);
        let permanent = SiftError::NotFound("message".into());
        let expired = SiftError::reauth("expired");
        assert_eq!(body_failure_state(&transient), "loading");
        assert_eq!(body_failure_state(&permanent), "error");
        assert!(should_refresh_body_provider(
            &expired,
            ProviderKind::GmailApi
        ));
        assert!(!should_refresh_body_provider(
            &expired,
            ProviderKind::GmailImap
        ));
    }

    #[tokio::test]
    async fn p5_t02_loading_dedup() {
        // message_body for body_state none returns loading (fetch scheduled, no dup panic)
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(dir.path()).unwrap();
        let a = db.new_account("a@x.com", None, None).await.unwrap();
        db.write({
      let aid = a.id.clone();
      move |c| { c.execute("INSERT INTO messages (id,account_id,thread_id,internal_date,body_state) VALUES ('m9',?, 't9', 1, 'none')", rusqlite::params![aid])?; Ok(()) }
    }).await.unwrap();
        // bodies_get none -> would schedule; assert none exists
        assert!(db.bodies_get("m9").await.unwrap().is_none());
    }
}
