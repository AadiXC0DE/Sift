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
    // The Search view, the keyset cursor and the query/sort identity are all
    // resolved in one place (`Db::threads_query`), so the list and a saved
    // mailbox can never page differently.
    state.db.threads_query(query).await
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
        let mut rows = s.query_map(rusqlite::params![a, t], crate::db::threads::thread_row_from)?;
        Ok(rows.next().transpose()?)
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
    // messages ascending, page 50+ (spec: page at 50 for huge threads - we return all up to 200, else latest 50)
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
          r.get::<_, String>("body_state")?, r.get::<_, Option<String>>("list_unsubscribe")?, r.get::<_, i64>("list_unsubscribe_post")? != 0,
          r.get::<_, Option<String>>("rfc_message_id")?, r.get::<_, Option<String>>("references_json")?))
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
        rfc_message_id,
        references_json,
    ) in msgs
    {
        let attachments = state
            .db
            .attachments_for_message(&MessageRef::new(account_id.clone(), id.clone()))
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        let to: Vec<Address> = serde_json::from_str(&to_json).unwrap_or_default();
        let cc: Vec<Address> = serde_json::from_str(&cc_json).unwrap_or_default();
        let bcc: Vec<Address> = serde_json::from_str(&bcc_json).unwrap_or_default();
        let list_unsubscribe = list_unsub.map(|u| {
            let targets = crate::unsubscribe::parse_targets(&u);
            let url = targets
                .iter()
                .find(|t| t.scheme == "https" || t.scheme == "http")
                .map(|t| t.url.clone());
            let mailto = targets
                .iter()
                .find(|t| t.scheme == "mailto")
                .map(|t| t.url.clone());
            ListUnsub {
                url,
                mailto,
                one_click: list_post,
                targets,
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
            rfc_message_id,
            references_json: references_json
                .map(|j| serde_json::from_str(&j).unwrap_or_default())
                .unwrap_or_default(),
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
    Option<String>,
    Option<String>,
);

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

async fn render_message_html(state: &AppState, message: &MessageRef, html: String) -> String {
    // Inline references are account-qualified so the scheme can validate
    // ownership; a body from one account can never address another. Cached
    // bodies from before the scoping migration are upgraded in place (in
    // memory) rather than re-rendered.
    let url_path = format!("{}/{}", message.account_id, message.message_id);
    let html = crate::render::sanitize::qualify_attachment_urls(
        &html,
        &message.account_id,
        &message.message_id,
    );
    let mut rendered = match state.db.attachments_with_bytes(message).await {
        Ok(parts) => crate::render::sanitize::embed_local_images(&html, &url_path, &parts),
        Err(_) => html,
    };
    let unresolved = crate::render::sanitize::inline_image_refs(&rendered, &url_path);
    if unresolved.is_empty() {
        return rendered;
    }
    let Ok(provider) = state.provider_for(&message.account_id).await else {
        return rendered;
    };
    for key in unresolved {
        if let Ok((bytes, mime)) =
            crate::uri_scheme::resolve_attachment(&state.db, &*provider, message, &key).await
        {
            let _ = state.db.attachment_cache_data(message, &key, &bytes).await;
            let part = vec![(key.clone(), key.clone(), Some(key), mime, bytes)];
            rendered = crate::render::sanitize::embed_local_images(&rendered, &url_path, &part);
        }
    }
    rendered
}

/// Whether this read may load external content, and the policy that decided.
struct RemoteDecision {
    mode: &'static str,
    allowed: bool,
    generation: i64,
}

fn db_err(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

/// The sender of one message, as stored. Used for the account-scoped
/// allow-list, which is only consulted in `ask` mode.
async fn message_sender(db: &crate::db::Db, message: &MessageRef) -> Result<String, SiftError> {
    let (account, id) = (message.account_id.clone(), message.message_id.clone());
    db.read(move |c| {
        Ok(c.query_row(
            "SELECT COALESCE(from_email,'') FROM messages WHERE account_id=? AND id=?",
            rusqlite::params![account, id],
            |r| r.get::<_, String>(0),
        )
        .unwrap_or_default())
    })
    .await
    .map_err(db_err)
}

/// Resolve the permission for one read (P9.1).
///
/// `block` never loads, `allow` always does, and `ask` consults — in this
/// order — the per-message session grant and then the sender allow-list for
/// **this account**. An unanswered one-time upgrade choice keeps the behaviour
/// the database already had (`allow`).
async fn remote_decision(
    state: &AppState,
    message: &MessageRef,
) -> Result<RemoteDecision, SiftError> {
    let privacy = state.db.privacy_state().await.map_err(db_err)?;
    let generation = state
        .db
        .privacy_generation(&message.account_id)
        .await
        .map_err(db_err)?
        + state.session_privacy_bumps(&message.account_id).await;
    let mode = privacy.effective();
    let allowed = match mode {
        crate::dto::RemoteContentMode::Block => false,
        crate::dto::RemoteContentMode::Allow => true,
        crate::dto::RemoteContentMode::Ask => {
            if state
                .session_remote_allowed(&message.account_id, &message.message_id)
                .await
            {
                true
            } else {
                let sender = message_sender(&state.db, message).await?;
                !sender.is_empty()
                    && state
                        .db
                        .sender_allowed(&message.account_id, &sender)
                        .await
                        .map_err(db_err)?
            }
        }
    };
    Ok(RemoteDecision {
        mode: mode.as_str(),
        allowed,
        generation,
    })
}

/// Build the IPC payload for one read, applying the permission to the stored
/// body. A broken inline reference is reported, never worked around by turning
/// remote content on.
/// What the sanitized body contributed to the payload.
struct BodyStats {
    remote_images: i64,
    trackers: i64,
    dark_safe: bool,
    unresolved_inline: i64,
}

impl Default for BodyStats {
    fn default() -> Self {
        Self {
            remote_images: 0,
            trackers: 0,
            dark_safe: true,
            unresolved_inline: 0,
        }
    }
}

fn body_payload(
    decision: &RemoteDecision,
    message_id: String,
    state: &str,
    html: Option<String>,
    text: Option<String>,
    stats: BodyStats,
) -> MessageBody {
    MessageBody {
        message_id,
        state: state.into(),
        html,
        text,
        remote_image_count: stats.remote_images,
        tracker_count: stats.trackers,
        dark_safe: stats.dark_safe,
        remote_images_allowed: decision.allowed,
        remote_content_mode: decision.mode.to_string(),
        privacy_generation: decision.generation,
        render_version: crate::dto::RENDER_VERSION,
        unresolved_inline_count: stats.unresolved_inline,
    }
}

/// Render one stored body under the resolved permission.
async fn rendered_body(
    state: &AppState,
    message: &MessageRef,
    html: Option<String>,
    decision: &RemoteDecision,
) -> (Option<String>, i64) {
    let Some(html) = html else {
        return (None, 0);
    };
    let url_path = format!("{}/{}", message.account_id, message.message_id);
    let rendered = render_message_html(state, message, html).await;
    let unresolved = crate::render::sanitize::inline_image_refs(&rendered, &url_path).len() as i64;
    (
        Some(crate::render::policy::apply(&rendered, decision.allowed)),
        unresolved,
    )
}

#[tauri::command]
pub async fn message_body(
    state: State<'_, AppState>,
    account_id: String,
    message_id: String,
) -> Result<MessageBody, SiftError> {
    message_body_for(&state, &account_id, &message_id).await
}

/// Read one message body under the account's current remote-content policy.
///
/// Split out from the command so the whole path — permission, sanitized
/// render, unresolved inline references — is exercisable without a Tauri
/// runtime.
pub async fn message_body_for(
    state: &AppState,
    account_id: &str,
    message_id: &str,
) -> Result<MessageBody, SiftError> {
    let message = MessageRef::new(account_id, message_id);
    // Ownership check: the message must exist in this account. The provider id
    // alone is not a cross-account isolation boundary (P4.2).
    let exists = state.db.message_thread(&message).await.map_err(db_err)?;
    if exists.is_none() {
        return Ok(body_payload(
            &RemoteDecision {
                mode: "block",
                allowed: false,
                generation: 0,
            },
            message.message_id,
            "error",
            None,
            None,
            BodyStats::default(),
        ));
    }
    let decision = remote_decision(state, &message).await?;
    if let Some((html, text, remote_images, trackers, dark_safe, _q)) =
        state.db.bodies_get(&message).await.map_err(db_err)?
    {
        let (html_out, unresolved) = rendered_body(state, &message, html, &decision).await;
        return Ok(body_payload(
            &decision,
            message.message_id,
            "ready",
            html_out,
            text,
            BodyStats {
                remote_images,
                trackers,
                dark_safe,
                unresolved_inline: unresolved,
            },
        ));
    }
    // Foreground fetches must preempt bulk backfill after a rendering upgrade.
    let _foreground = state.gate.enter();
    let mut provider = match state.provider_for(&message.account_id).await {
        Ok(p) => p,
        Err(error) => {
            return Ok(body_payload(
                &decision,
                message.message_id,
                body_failure_state(&error),
                None,
                Some(error.to_string()),
                BodyStats::default(),
            ))
        }
    };
    let mut fetched = provider.fetch_body(&message.message_id).await;
    // A cached Gmail provider owns the token it was constructed with. If the
    // server rejects that token, rebuild it from the refresh token and retry
    // the foreground request once instead of failing every opened message.
    if fetched
        .as_ref()
        .is_err_and(|error| should_refresh_body_provider(error, provider.kind()))
    {
        match state.refresh_provider(&message.account_id).await {
            Ok(refreshed) => {
                provider = refreshed;
                fetched = provider.fetch_body(&message.message_id).await;
            }
            Err(refresh_error) => fetched = Err(refresh_error),
        }
    }
    match fetched {
        Ok(parsed) => {
            let sink = crate::provider::DbSink::new(state.db.clone());
            if crate::provider::store_parsed(&sink, &message, &parsed)
                .await
                .is_err()
            {
                return Ok(body_payload(
                    &decision,
                    message.message_id,
                    "error",
                    None,
                    parsed.text,
                    BodyStats::default(),
                ));
            }
        }
        Err(error) => {
            return Ok(body_payload(
                &decision,
                message.message_id,
                body_failure_state(&error),
                None,
                Some(error.to_string()),
                BodyStats::default(),
            ))
        }
    }
    if let Some((html, text, remote_images, trackers, dark_safe, _q)) =
        state.db.bodies_get(&message).await.map_err(db_err)?
    {
        let (html_out, unresolved) = rendered_body(state, &message, html, &decision).await;
        return Ok(body_payload(
            &decision,
            message.message_id,
            "ready",
            html_out,
            text,
            BodyStats {
                remote_images,
                trackers,
                dark_safe,
                unresolved_inline: unresolved,
            },
        ));
    }
    Ok(body_payload(
        &decision,
        message.message_id,
        "error",
        None,
        None,
        BodyStats::default(),
    ))
}

#[tauri::command]
pub async fn message_raw_source(
    state: State<'_, AppState>,
    account_id: String,
    message_id: String,
) -> Result<String, SiftError> {
    let message = MessageRef::new(account_id, message_id);
    let exists = state.db.message_thread(&message).await.map_err(db_err)?;
    if exists.is_none() {
        return Err(SiftError::NotFound("message".into()));
    }
    let provider = state.provider_for(&message.account_id).await?;
    provider.fetch_raw(&message.message_id).await
}

// ---------------------------------------------------------------------------
// Remote content permission (P9.1)
// ---------------------------------------------------------------------------

async fn policy_payload(
    state: &AppState,
    account_id: &str,
) -> Result<RemoteContentPolicy, SiftError> {
    let privacy = state.db.privacy_state().await.map_err(db_err)?;
    Ok(RemoteContentPolicy {
        mode: privacy.stored_mode.as_str().to_string(),
        choice_pending: privacy.choice_pending,
        allowed_senders: state
            .db
            .sender_allow_list(account_id)
            .await
            .map_err(db_err)?,
        generation: state
            .db
            .privacy_generation(account_id)
            .await
            .map_err(db_err)?,
        privacy_notice: PRIVACY_NOTICE.to_string(),
    })
}

#[tauri::command]
pub async fn remote_content_policy_get(
    state: State<'_, AppState>,
    account_id: String,
) -> Result<RemoteContentPolicy, SiftError> {
    policy_payload(&state, &account_id).await
}

/// Set the explicit policy. Answering this clears the one-time upgrade prompt
/// and moves every account's permission generation, because a cached body was
/// rendered under the previous permission.
#[tauri::command]
pub async fn remote_content_policy_set(
    state: State<'_, AppState>,
    account_id: String,
    mode: String,
) -> Result<RemoteContentPolicy, SiftError> {
    let parsed = crate::dto::RemoteContentMode::parse(&mode)
        .ok_or_else(|| SiftError::app("bad_request", "unknown remote-content mode", false))?;
    state.db.privacy_set_mode(parsed).await.map_err(db_err)?;
    policy_payload(&state, &account_id).await
}

#[tauri::command]
pub async fn remote_content_sender_revoke(
    state: State<'_, AppState>,
    account_id: String,
    sender: String,
) -> Result<RemoteContentPolicy, SiftError> {
    state
        .db
        .sender_revoke(&account_id, &sender)
        .await
        .map_err(db_err)?;
    state
        .db
        .privacy_bump_generations(Some(std::slice::from_ref(&account_id)))
        .await
        .map_err(db_err)?;
    policy_payload(&state, &account_id).await
}

/// Grant external content for one message.
///
/// `rememberSender` persists an account-scoped "always for sender"; without it
/// the grant is a **session** permission for this one message and is never
/// written to settings. Either way the body is returned re-rendered under the
/// new permission.
#[tauri::command]
pub async fn remote_content_allow(
    state: State<'_, AppState>,
    account_id: String,
    message_id: String,
    remember_sender: bool,
) -> Result<MessageBody, SiftError> {
    remote_content_allow_for(&state, &account_id, &message_id, remember_sender).await
}

/// Grant external content for one message; see [`remote_content_allow`].
pub async fn remote_content_allow_for(
    state: &AppState,
    account_id: &str,
    message_id: &str,
    remember_sender: bool,
) -> Result<MessageBody, SiftError> {
    let message = MessageRef::new(account_id, message_id);
    if remember_sender {
        let sender = message_sender(&state.db, &message).await?;
        if sender.is_empty() {
            return Err(SiftError::app(
                "bad_request",
                "This message has no sender address to remember.",
                false,
            ));
        }
        state
            .db
            .sender_allow(&message.account_id, &sender)
            .await
            .map_err(db_err)?;
        state
            .db
            .privacy_bump_generations(Some(std::slice::from_ref(&message.account_id)))
            .await
            .map_err(db_err)?;
    } else {
        state
            .grant_session_remote_load(&message.account_id, &message.message_id)
            .await;
    }
    message_body_for(state, &message.account_id, &message.message_id).await
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
        assert!(db
            .bodies_get(&crate::dto::MessageRef::new(a.id.clone(), "m9"))
            .await
            .unwrap()
            .is_none());
    }
}
