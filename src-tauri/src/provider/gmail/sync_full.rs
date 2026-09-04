use super::client::GmailClient;
use crate::db::messages::MsgUpsert;
use crate::dto::Label;
use crate::provider::{Cursor, SyncSink};
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
    sink: &dyn SyncSink,
    account_id: &str,
    client: &GmailClient,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<Cursor> {
    // 1. profile FIRST (historyId before listing)
    let profile = client
        .get_profile()
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let start_hid = profile.history_id.clone();
    let progress = |phase: &str, done: i64, total: i64| {
        sink.progress(crate::dto::SyncStatus {
            account_id: account_id.into(),
            phase: phase.into(),
            done,
            total,
            last_error: None,
        })
    };
    progress("profile", 0, 0);
    // 2. labels
    let labels = client
        .list_labels()
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let dto_labels: Vec<Label> = labels
        .iter()
        .enumerate()
        .map(|(i, l)| Label {
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
        .collect();
    sink.upsert_labels(&dto_labels).await?;
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
        progress("listing", ids.len() as i64, ids.len() as i64);
        if page.is_none() {
            break;
        }
    }
    // insert stubs
    for (id, tid) in &ids {
        sink.insert_stub(id, account_id, tid).await?;
    }
    // 4. metadata newest-first in batches of 50 (Gmail returns newest first already)
    let total = ids.len() as i64;
    let mut done = 0i64;

    // newest-first: reverse? list returns newest first; keep order
    for chunk in ids.chunks(50) {
        if cancel.is_cancelled() {
            return Err(anyhow::anyhow!("cancelled"));
        }
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
                    let parsed = super::mime::parse_addrs(&from_raw);
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
                    let _ = sink.upsert_message(up).await;
                }
                Err(crate::errors::SiftError::NotFound(_)) => {
                    // added then deleted: drop the stub so no ghost thread remains.
                    let _ = sink.delete_message(id, account_id, tid).await;
                }
                Err(_) => {}
            }
        }
        done += chunk.len() as i64;
        progress("metadata", done, total);
    }
    sink.set_history_id(account_id, &start_hid).await?;
    sink.set_sync_state(account_id, "partial").await?;
    sink.log_sync(account_id, "full", "done").await?;
    Ok(Cursor::Gmail {
        history_id: start_hid,
    })
}
