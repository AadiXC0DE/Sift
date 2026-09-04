use crate::db::{messages::MsgUpsert, Db};
use crate::gmail::client::GmailClient;
use anyhow::Result;

pub struct PartialOut {
    pub changed_threads: Vec<(String, String)>,
    pub new_inbox: Vec<(String, String, String, String)>,
}

pub async fn run_partial_sync(
    db: &Db,
    account_id: &str,
    client: &GmailClient,
) -> Result<PartialOut> {
    let acc = db
        .accounts_get(account_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("no account"))?;
    let mut start = acc.history_id.clone().unwrap_or_default();
    if start.is_empty() {
        // no cursor: fall back to reconcile
        super::reconcile::run_reconcile(db, account_id, client).await?;
        return Ok(PartialOut {
            changed_threads: vec![],
            new_inbox: vec![],
        });
    }
    let mut changed: Vec<(String, String)> = vec![];
    let mut new_inbox = vec![];
    let mut page: Option<String> = None;
    loop {
        let resp = match client.history_list(&start, page.as_deref()).await {
            Ok(r) => r,
            Err(crate::errors::SiftError::NotFound(_)) => {
                super::reconcile::run_reconcile(db, account_id, client).await?;
                return Ok(PartialOut {
                    changed_threads: changed,
                    new_inbox,
                });
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
                            let parsed = crate::gmail::mime::parse_addrs(&from_raw);
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
                            let _ = db.messages_upsert(up).await;
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
                    if let Some((a, t)) = db.message_thread(&d.message.id).await? {
                        db.messages_delete(&d.message.id, &a, &t).await?;
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
                    let cur: Vec<String> = db
                        .read({
                            let mid = chg.message.id.clone();
                            move |c| {
                                Ok(c.query_row(
                                    "SELECT label_ids FROM messages WHERE id=?",
                                    rusqlite::params![mid],
                                    |r| r.get::<_, String>(0),
                                )
                                .unwrap_or("[]".into()))
                            }
                        })
                        .await
                        .map(|j| serde_json::from_str(&j).unwrap_or_default())
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
                        if let Ok((a, t)) =
                            db.apply_label_change(&chg.message.id, &add, &remove).await
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
    db.accounts_set_history(account_id, &start, crate::db::now_ms())
        .await?;
    Ok(PartialOut {
        changed_threads: changed,
        new_inbox,
    })
}
