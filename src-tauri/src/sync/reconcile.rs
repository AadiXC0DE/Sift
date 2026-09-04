use crate::db::{messages::MsgUpsert, Db};
use crate::gmail::client::GmailClient;
use anyhow::Result;

pub async fn run_reconcile(db: &Db, account_id: &str, client: &GmailClient) -> Result<()> {
    let profile = client
        .get_profile()
        .await
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    // list all server ids
    let mut page: Option<String> = None;
    let mut server: std::collections::HashSet<String> = Default::default();
    let mut thread_of: std::collections::HashMap<String, String> = Default::default();
    loop {
        let resp = client
            .list_messages(page.as_deref(), None, true)
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        for m in resp.messages.unwrap_or_default() {
            server.insert(m.id.clone());
            thread_of.insert(m.id.clone(), m.thread_id.clone());
        }
        page = resp.next_page_token;
        if page.is_none() {
            break;
        }
    }
    let local: Vec<(String, String)> = db
        .read({
            let aid = account_id.to_string();
            move |c| -> anyhow::Result<Vec<(String, String)>> {
                let mut s = c.prepare("SELECT id, thread_id FROM messages WHERE account_id=?")?;
                let v: Vec<(String, String)> = s
                    .query_map(rusqlite::params![aid], |r| {
                        Ok::<(String, String), rusqlite::Error>((r.get(0)?, r.get(1)?))
                    })?
                    .collect::<Result<Vec<(String, String)>, rusqlite::Error>>()?;
                Ok(v)
            }
        })
        .await?;
    let local_set: std::collections::HashSet<String> =
        local.iter().map(|(i, _)| i.clone()).collect();
    // delete L - S
    for (id, tid) in &local {
        if !server.contains(id) {
            db.messages_delete(id, account_id, tid).await?;
        }
    }
    // insert S - L
    let missing: Vec<String> = server.difference(&local_set).cloned().collect();
    for chunk in missing.chunks(50) {
        let res = client
            .batch_get_messages(chunk, "metadata")
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        for (id, r) in chunk.iter().zip(res) {
            if let Ok(m) = r {
                let labels = m.label_ids.clone().unwrap_or_default();
                let tid = thread_of
                    .get(id)
                    .cloned()
                    .unwrap_or_else(|| m.thread_id.clone());
                let up = MsgUpsert {
                    id: id.clone(),
                    account_id: account_id.into(),
                    thread_id: tid,
                    history_id: m.history_id.clone(),
                    internal_date: m
                        .internal_date
                        .as_deref()
                        .and_then(|s| s.parse::<i64>().ok())
                        .unwrap_or(0),
                    subject: String::new(),
                    snippet: m.snippet.clone().unwrap_or_default(),
                    is_unread: labels.contains(&"UNREAD".into()),
                    is_starred: labels.contains(&"STARRED".into()),
                    is_draft: labels.contains(&"DRAFT".into()),
                    label_ids: labels,
                    ..Default::default()
                };
                let _ = db.messages_upsert(up).await;
            }
        }
    }
    // refresh S ∩ L labels (minimal)
    let both: Vec<String> = server.intersection(&local_set).cloned().collect();
    for chunk in both.chunks(50) {
        let res = client
            .batch_get_messages(chunk, "minimal")
            .await
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        for (id, r) in chunk.iter().zip(res) {
            if let Ok(m) = r {
                let labels = m.label_ids.clone().unwrap_or_default();
                let cur: String = db
                    .read({
                        let mid = id.clone();
                        move |c| {
                            Ok(c.query_row(
                                "SELECT label_ids FROM messages WHERE id=?",
                                rusqlite::params![mid],
                                |x| x.get(0),
                            )
                            .unwrap_or("[]".into()))
                        }
                    })
                    .await?;
                let cur: Vec<String> = serde_json::from_str(&cur).unwrap_or_default();
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
                    let _ = db.apply_label_change(id, &add, &remove).await;
                }
            }
        }
    }
    db.accounts_set_history(account_id, &profile.history_id, crate::db::now_ms())
        .await?;
    Ok(())
}
