use crate::db::{messages::MsgUpsert, Db};
use crate::dto::Label;
use crate::gmail::client::GmailClient;
use anyhow::Result;

fn sys_order(name: &str, i: usize) -> i64 {
    match name {
        "INBOX" => 0,
        "STARRED" => 1,
        "SENT" => 2,
        "DRAFT" => 3,
        "SPAM" => 4,
        "TRASH" => 5,
        _ => 100 + i as i64,
    }
}

pub async fn run_full_sync(
    db: &Db,
    account_id: &str,
    client: &GmailClient,
    emit: impl Fn(crate::dto::SyncStatus) + Send + Sync + 'static,
) -> Result<()> {
    // 1. profile FIRST (historyId before listing)
    let profile = client
        .get_profile()
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let start_hid = profile.history_id.clone();
    emit(crate::dto::SyncStatus {
        account_id: account_id.into(),
        phase: "profile".into(),
        done: 0,
        total: 0,
        last_error: None,
    });
    // 2. labels
    let labels = client
        .list_labels()
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    for (i, l) in labels.iter().enumerate() {
        db.labels_upsert(&Label {
            account_id: account_id.into(),
            id: l.id.clone(),
            name: l.name.clone(),
            kind: if l.kind == "system" {
                "system".into()
            } else {
                "user".into()
            },
            color_bg: l.color.as_ref().and_then(|c| c.bg.clone()),
            color_fg: l.color.as_ref().and_then(|c| c.fg.clone()),
            visible: l.list_visibility.as_deref() != Some("labelHide"),
            unread_count: 0,
            total_count: 0,
            sort_order: sys_order(&l.id, i),
        })
        .await?;
    }
    // 3. list ids
    let mut page: Option<String> = None;
    let mut ids: Vec<(String, String)> = vec![];
    loop {
        let resp = client
            .list_messages(page.as_deref(), None, true)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        for m in resp.messages.unwrap_or_default() {
            ids.push((m.id, m.thread_id));
        }
        // insert stubs progressively
        page = resp.next_page_token;
        emit(crate::dto::SyncStatus {
            account_id: account_id.into(),
            phase: "listing".into(),
            done: ids.len() as i64,
            total: ids.len() as i64,
            last_error: None,
        });
        if page.is_none() {
            break;
        }
    }
    // insert stubs
    for (id, tid) in &ids {
        let (id, tid, aid) = (id.clone(), tid.clone(), account_id.to_string());
        db.write(move |c| {
      c.execute("INSERT OR IGNORE INTO messages (id,account_id,thread_id,internal_date,body_state) VALUES (?,?,?,0,'none')", rusqlite::params![id, aid, tid])?;
      Ok(())
    }).await?;
    }
    // 4. metadata newest-first in batches of 50 (Gmail returns newest first already)
    let total = ids.len() as i64;
    let mut done = 0i64;
    // newest-first: reverse? list returns newest first; keep order
    for chunk in ids.chunks(50) {
        let chunk_ids: Vec<String> = chunk.iter().map(|(i, _)| i.clone()).collect();
        let results = client
            .batch_get_messages(&chunk_ids, "metadata")
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        for ((id, tid), res) in chunk.iter().zip(results) {
            match res {
                Ok(m) => {
                    let labels = m.label_ids.clone().unwrap_or_default();
                    let hdrs: std::collections::HashMap<String, String> = m
                        .payload
                        .as_ref()
                        .and_then(|p| p.headers.clone())
                        .unwrap_or_default()
                        .into_iter()
                        .map(|h| (h.name.to_lowercase(), h.value))
                        .collect();
                    let get = |n: &str| hdrs.get(n).cloned().unwrap_or_default();
                    let from_raw = get("from");
                    let parsed = crate::gmail::mime::parse_addrs(&from_raw);
                    let up = MsgUpsert {
                        id: id.clone(),
                        account_id: account_id.into(),
                        thread_id: tid.clone(),
                        history_id: m.history_id.clone(),
                        internal_date: m
                            .internal_date
                            .as_deref()
                            .and_then(|s| s.parse::<i64>().ok())
                            .unwrap_or(0),
                        from_name: parsed.first().and_then(|(n, _)| n.clone()),
                        from_email: parsed.first().map(|(_, e)| e.clone()),
                        to_json: serde_json::to_string(&parsed).unwrap_or("[]".into()),
                        cc_json: "[]".into(),
                        bcc_json: "[]".into(),
                        reply_to: hdrs.get("reply-to").cloned(),
                        subject: get("subject"),
                        snippet: m.snippet.clone().unwrap_or_default(),
                        rfc_message_id: hdrs.get("message-id").cloned(),
                        in_reply_to: hdrs.get("in-reply-to").cloned(),
                        references_json: "[]".into(),
                        list_unsubscribe: hdrs.get("list-unsubscribe").cloned(),
                        list_unsubscribe_post: hdrs
                            .get("list-unsubscribe-post")
                            .map(|s| s.contains("One-Click"))
                            .unwrap_or(false),
                        size_estimate: m.size_estimate,
                        has_attachments: false,
                        is_unread: labels.contains(&"UNREAD".to_string()),
                        is_starred: labels.contains(&"STARRED".to_string()),
                        is_draft: labels.contains(&"DRAFT".to_string()),
                        is_sent_by_me: labels.contains(&"SENT".to_string()),
                        label_ids: labels,
                    };
                    let _ = db.messages_upsert(up).await;
                }
                Err(crate::errors::SiftError::NotFound(_)) => {
                    // added then deleted: skip
                    let (id, aid, tid) = (id.clone(), account_id.to_string(), tid.clone());
                    let _ = db
                        .write(move |c| {
                            c.execute("DELETE FROM messages WHERE id=?", rusqlite::params![id])?;
                            Ok(())
                        })
                        .await;
                    let _ = (aid, tid);
                }
                Err(_) => {}
            }
        }
        done += chunk.len() as i64;
        emit(crate::dto::SyncStatus {
            account_id: account_id.into(),
            phase: "metadata".into(),
            done,
            total,
            last_error: None,
        });
    }
    db.accounts_set_history(account_id, &start_hid, crate::db::now_ms())
        .await?;
    db.accounts_set_state(account_id, "partial").await?;
    db.write({
        let aid = account_id.to_string();
        move |c| {
            c.execute(
                "INSERT INTO sync_log (account_id,at,kind,detail) VALUES (?,?,?,?)",
                rusqlite::params![aid, crate::db::now_ms(), "full", "done"],
            )?;
            Ok(())
        }
    })
    .await?;
    Ok(())
}
