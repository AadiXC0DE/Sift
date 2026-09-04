use super::client::GmailClient;
use crate::db::messages::MsgUpsert;
use crate::provider::{PartialOutcome, SyncSink};
use anyhow::Result;

pub async fn run_partial_sync(
    sink: &dyn SyncSink,
    account_id: &str,
    client: &GmailClient,
    start_history: &str,
) -> Result<PartialOutcome> {
    let mut start = start_history.to_string();
    if start.is_empty() {
        // No cursor: the caller reconciles, then continues from the new id.
        return Ok(PartialOutcome::NeedsFull);
    }
    let mut changed: Vec<(String, String)> = vec![];
    let mut new_inbox = vec![];
    let mut page: Option<String> = None;
    loop {
        let resp = match client.history_list(&start, page.as_deref()).await {
            Ok(r) => r,
            Err(crate::errors::SiftError::NotFound(_)) => {
                // historyId too old (~1 week): caller reconciles and retries.
                return Ok(PartialOutcome::NeedsFull);
            }
            Err(e) => return Err(anyhow::anyhow!(e.to_string())),
        };
        for rec in resp.history.unwrap_or_default() {
            if let Some(added) = rec.added {
                for a in added {
                    // fetch metadata
                    match client.get_message_meta(&a.message.id).await {
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
                            let internal_date = m
                                .internal_date
                                .as_deref()
                                .and_then(|s| s.parse::<i64>().ok())
                                .unwrap_or(0);
                            let up = MsgUpsert {
                                id: a.message.id.clone(),
                                account_id: account_id.into(),
                                thread_id: a.message.thread_id.clone(),
                                history_id: m.history_id.clone(),
                                internal_date,
                                from_name: parsed.first().and_then(|(n, _)| n.clone()),
                                from_email: parsed.first().map(|(_, e)| e.clone()),
                                to_json: "[]".into(),
                                cc_json: "[]".into(),
                                bcc_json: "[]".into(),
                                reply_to: None,
                                subject: get("subject"),
                                snippet: m.snippet.clone().unwrap_or_default(),
                                rfc_message_id: None,
                                in_reply_to: None,
                                references_json: "[]".into(),
                                list_unsubscribe: None,
                                list_unsubscribe_post: false,
                                size_estimate: m.size_estimate,
                                has_attachments: false,
                                is_unread: labels.contains(&"UNREAD".into()),
                                is_starred: labels.contains(&"STARRED".into()),
                                is_draft: labels.contains(&"DRAFT".into()),
                                is_sent_by_me: false,
                                label_ids: labels.clone(),
                            };
                            let tid = a.message.thread_id.clone();
                            let _ = sink.upsert_message(up).await;
                            changed.push((account_id.to_string(), tid.clone()));
                            if labels.contains(&"INBOX".into()) && labels.contains(&"UNREAD".into())
                            {
                                new_inbox.push((
                                    account_id.to_string(),
                                    tid,
                                    from_raw,
                                    get("subject"),
                                ));
                            }
                        }
                        Err(crate::errors::SiftError::NotFound(_)) => { /* added then deleted: skip */
                        }
                        Err(e) => return Err(anyhow::anyhow!(e.to_string())),
                    }
                }
            }
            if let Some(deleted) = rec.deleted {
                for d in deleted {
                    // find thread then delete
                    if let Some((a, t)) = sink.message_thread(&d.message.id).await? {
                        sink.delete_message(&d.message.id, &a, &t).await?;
                        changed.push((a, t));
                    }
                }
            }
            for chg in rec
                .labels_added
                .into_iter()
                .flatten()
                .chain(rec.labels_removed.into_iter().flatten())
            {
                // determine add vs remove by which list it came from — simplified: apply both directions via message_labels diff
                // Here we just re-fetch minimal labels
                if let Ok(m) = client.get_message_meta(&chg.message.id).await {
                    let labels = m.label_ids.clone().unwrap_or_default();
                    // compute add/remove vs DB
                    let cur: Vec<String> = sink
                        .message_labels(&chg.message.id)
                        .await
                        .unwrap_or_default();
                    let add: Vec<String> = labels
                        .iter()
                        .filter(|l| !cur.contains(l))
                        .cloned()
                        .collect();
                    let remove: Vec<String> = cur
                        .iter()
                        .filter(|l| !labels.contains(l))
                        .cloned()
                        .collect();
                    if !add.is_empty() || !remove.is_empty() {
                        if let Ok((a, t)) = sink
                            .apply_label_change(&chg.message.id, &add, &remove)
                            .await
                        {
                            changed.push((a, t));
                        }
                    }
                }
            }
        }
        start = resp.history_id.clone();
        page = resp.next_page_token;
        if page.is_none() {
            break;
        }
    }
    sink.set_history_id(account_id, &start).await?;
    Ok(PartialOutcome::Synced {
        changed_threads: changed,
        new_inbox,
    })
}
