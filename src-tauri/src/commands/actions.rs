use crate::app_state::AppState;
use crate::dto::{ActionKind, ThreadAction};
use crate::errors::SiftError;
use tauri::State;

fn label_sets(action: &ActionKind) -> (Vec<String>, Vec<String>, bool, bool) {
    // (add, remove, trash_flag, spam_flag) — local message_labels edit
    match action {
        ActionKind::Archive => (vec![], vec!["INBOX".into()], false, false),
        ActionKind::Unarchive => (vec!["INBOX".into()], vec![], false, false),
        ActionKind::Trash => (vec!["TRASH".into()], vec!["INBOX".into()], true, false),
        ActionKind::Untrash => (vec!["INBOX".into()], vec!["TRASH".into()], false, false),
        ActionKind::Spam => (vec!["SPAM".into()], vec!["INBOX".into()], false, true),
        ActionKind::Unspam => (vec!["INBOX".into()], vec!["SPAM".into()], false, false),
        ActionKind::DeleteForever => (vec![], vec![], false, false),
        ActionKind::Star { on } => (
            if *on { vec!["STARRED".into()] } else { vec![] },
            if *on { vec![] } else { vec!["STARRED".into()] },
            false,
            false,
        ),
        ActionKind::Read { on } => (
            if *on { vec![] } else { vec!["UNREAD".into()] },
            if *on { vec!["UNREAD".into()] } else { vec![] },
            false,
            false,
        ),
        ActionKind::AddLabel { label_id } => (vec![label_id.clone()], vec![], false, false),
        ActionKind::RemoveLabel { label_id } => (vec![], vec![label_id.clone()], false, false),
        ActionKind::MoveTo { label_id } => {
            (vec![label_id.clone()], vec!["INBOX".into()], false, false)
        }
    }
}

#[tauri::command]
pub async fn threads_action(
    state: State<'_, AppState>,
    req: ThreadAction,
) -> Result<serde_json::Value, SiftError> {
    if matches!(req.action, ActionKind::DeleteForever) {
        // only from trash/spam
        for tid in &req.thread_ids {
            let in_trash: bool = state
                .db
                .read({
                    let (a, t) = (req.account_id.clone(), tid.clone());
                    move |c| {
                        Ok(c.query_row(
                            "SELECT in_trash FROM threads WHERE account_id=? AND id=?",
                            rusqlite::params![a, t],
                            |r| r.get::<_, i64>(0),
                        )
                        .map(|v| v != 0)
                        .unwrap_or(false))
                    }
                })
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            let in_spam: bool = state
                .db
                .read({
                    let (a, t) = (req.account_id.clone(), tid.clone());
                    move |c| {
                        Ok(c.query_row(
                            "SELECT in_spam FROM threads WHERE account_id=? AND id=?",
                            rusqlite::params![a, t],
                            |r| r.get::<_, i64>(0),
                        )
                        .map(|v| v != 0)
                        .unwrap_or(false))
                    }
                })
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            if !in_trash && !in_spam {
                return Err(SiftError::not_in_trash());
            }
        }
    }
    let undo_group = uuid::Uuid::now_v7().to_string();
    let (add, remove, _trash, _spam) = label_sets(&req.action);
    // group by account (req is single-account in v1 IPC; mixed selection splits client-side into 2 calls sharing undo_group — see P6-T17)
    // Local mutation in one write transaction per thread
    let mut all_ids: Vec<String> = vec![];
    // star semantics: latest message only
    let star_only_latest = matches!(req.action, ActionKind::Star { .. });
    for tid in &req.thread_ids {
        let mids: Vec<String> = state.db.read({
      let (a, t) = (req.account_id.clone(), tid.clone());
      move |c| -> anyhow::Result<Vec<String>> {
        if star_only_latest {
          let one: String = c.query_row("SELECT id FROM messages WHERE account_id=? AND thread_id=? ORDER BY internal_date DESC LIMIT 1", rusqlite::params![a, t], |r| r.get(0)).unwrap_or_default();
          Ok(vec![one])
        } else {
          let mut s = c.prepare("SELECT id FROM messages WHERE account_id=? AND thread_id=?")?;
          let v: Vec<String> = s.query_map(rusqlite::params![a, t], |r| r.get(0))?.collect::<Result<Vec<String>, rusqlite::Error>>()?;
          Ok(v)
        }
      }
    }).await.map_err(|e| SiftError::app("db", e.to_string(), false))?;
        let mids: Vec<String> = mids.into_iter().filter(|s| !s.is_empty()).collect();
        for mid in &mids {
            state
                .db
                .apply_label_change(mid, &add, &remove)
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            all_ids.push(mid.clone());
        }
        // snooze clear on archive etc? keep snoozed_until unless unsnoozed explicitly
    }
    if matches!(req.action, ActionKind::DeleteForever) {
        for tid in &req.thread_ids {
            let (a, t) = (req.account_id.clone(), tid.clone());
            state
                .db
                .write(move |c| {
                    c.execute(
                        "DELETE FROM messages WHERE account_id=? AND thread_id=?",
                        rusqlite::params![a, t],
                    )?;
                    c.execute(
                        "DELETE FROM threads WHERE account_id=? AND id=?",
                        rusqlite::params![a, t],
                    )?;
                    Ok(())
                })
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        }
        state
            .db
            .outbox_enqueue(
                &req.account_id,
                "delete",
                &serde_json::json!({"threads": req.thread_ids}).to_string(),
                Some(&undo_group),
                0,
            )
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    } else if !all_ids.is_empty() {
        // enqueue modify_labels (coalesced by drain)
        let payload = serde_json::json!({"ids": all_ids, "add": add, "remove": remove}).to_string();
        // coalesce: if last pending op for account within 200ms has same add/remove, merge ids
        state
            .db
            .outbox_enqueue(
                &req.account_id,
                "modify_labels",
                &payload,
                Some(&undo_group),
                0,
            )
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    }
    // trash/untrash use thread endpoints (still enqueue modify for simplicity + correct reconcile)
    Ok(serde_json::json!({ "undo_group": undo_group }))
}

#[tauri::command]
pub async fn action_undo(state: State<'_, AppState>, undo_group: String) -> Result<(), SiftError> {
    // cancel pending
    let pending = state
        .db
        .outbox_cancel_group(&undo_group)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    // invert done ops
    let done = state
        .db
        .outbox_done_group(&undo_group)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    for op in done {
        let payload: serde_json::Value = serde_json::from_str(&op.payload).unwrap_or_default();
        if op.kind.as_str() == "modify_labels" {
            let add: Vec<String> =
                serde_json::from_value(payload["add"].clone()).unwrap_or_default();
            let remove: Vec<String> =
                serde_json::from_value(payload["remove"].clone()).unwrap_or_default();
            let ids: Vec<String> =
                serde_json::from_value(payload["ids"].clone()).unwrap_or_default();
            // inverse local
            for mid in &ids {
                let _ = state.db.apply_label_change(mid, &remove, &add).await;
            }
            let inv = serde_json::json!({"ids": ids, "add": remove, "remove": add}).to_string();
            let _ = state
                .db
                .outbox_enqueue(&op.account_id, "modify_labels", &inv, None, 0)
                .await;
        }
    }
    // revert pending locals (apply inverse without enqueue)
    for op in pending {
        let payload: serde_json::Value = serde_json::from_str(&op.payload).unwrap_or_default();
        if op.kind == "modify_labels" {
            let add: Vec<String> =
                serde_json::from_value(payload["add"].clone()).unwrap_or_default();
            let remove: Vec<String> =
                serde_json::from_value(payload["remove"].clone()).unwrap_or_default();
            let ids: Vec<String> =
                serde_json::from_value(payload["ids"].clone()).unwrap_or_default();
            for mid in &ids {
                let _ = state.db.apply_label_change(mid, &remove, &add).await;
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn snooze_set(
    state: State<'_, AppState>,
    account_id: String,
    thread_ids: Vec<String>,
    wake_at: i64,
) -> Result<serde_json::Value, SiftError> {
    // ensure Sift/Snoozed label
    let labels = state
        .db
        .labels_list(&account_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    if !labels.iter().any(|l| l.name == "Sift/Snoozed") {
        // create remotely if online, else local stub
        let remote_id = if state.is_online() {
            match state.client_for(&account_id).await {
                Ok(c) => c
                    .create_label("Sift/Snoozed")
                    .await
                    .map(|l| l.id)
                    .unwrap_or_else(|_| "Label_Snoozed".into()),
                Err(_) => "Label_Snoozed".into(),
            }
        } else {
            "Label_Snoozed".into()
        };
        state
            .db
            .labels_upsert(&crate::dto::Label {
                account_id: account_id.clone(),
                id: remote_id,
                name: "Sift/Snoozed".into(),
                kind: "user".into(),
                color_bg: None,
                color_fg: None,
                visible: true,
                unread_count: 0,
                total_count: 0,
                sort_order: 200,
            })
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    }
    let undo_group = uuid::Uuid::now_v7().to_string();
    for tid in &thread_ids {
        let (a, t) = (account_id.clone(), tid.clone());
        state
            .db
            .write(move |c| {
                c.execute(
                    "INSERT OR REPLACE INTO snoozes (account_id,thread_id,wake_at) VALUES (?,?,?)",
                    rusqlite::params![a, t, wake_at],
                )?;
                c.execute(
                    "UPDATE threads SET snoozed_until=? WHERE account_id=? AND id=?",
                    rusqlite::params![wake_at, a, t],
                )?;
                Ok(())
            })
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        // remove INBOX locally
        let mids: Vec<String> = state
            .db
            .read({
                let (a, t) = (account_id.clone(), tid.clone());
                move |c| {
                    Ok(
                        c.prepare("SELECT id FROM messages WHERE account_id=? AND thread_id=?")?
                            .query_map(rusqlite::params![a, t], |r| r.get(0))?
                            .collect::<Result<Vec<String>, _>>()?,
                    )
                }
            })
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        for mid in &mids {
            let _ = state
                .db
                .apply_label_change(mid, &[], &["INBOX".to_string()])
                .await;
        }
        let payload = serde_json::json!({"ids": mids, "add": [], "remove": ["INBOX"]}).to_string();
        state
            .db
            .outbox_enqueue(&account_id, "modify_labels", &payload, Some(&undo_group), 0)
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    }
    Ok(serde_json::json!({ "undo_group": undo_group }))
}

#[tauri::command]
pub async fn snooze_clear(
    state: State<'_, AppState>,
    account_id: String,
    thread_ids: Vec<String>,
) -> Result<(), SiftError> {
    for tid in &thread_ids {
        let (a, t) = (account_id.clone(), tid.clone());
        state
            .db
            .write(move |c| {
                c.execute(
                    "DELETE FROM snoozes WHERE account_id=? AND thread_id=?",
                    rusqlite::params![a, t],
                )?;
                c.execute(
                    "UPDATE threads SET snoozed_until=NULL WHERE account_id=? AND id=?",
                    rusqlite::params![a, t],
                )?;
                Ok(())
            })
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn labels_create(
    state: State<'_, AppState>,
    account_id: String,
    name: String,
    color: Option<String>,
) -> Result<crate::dto::Label, SiftError> {
    let client = state.client_for(&account_id).await?;
    let remote = client.create_label(&name).await?;
    let l = crate::dto::Label {
        account_id: account_id.clone(),
        id: remote.id,
        name: remote.name,
        kind: "user".into(),
        color_bg: color.clone(),
        color_fg: None,
        visible: true,
        unread_count: 0,
        total_count: 0,
        sort_order: 200,
    };
    state
        .db
        .labels_upsert(&l)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    Ok(l)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn p6_t01_archive_recompute() {
        let dir = tempfile::tempdir().unwrap();
        let db = crate::db::Db::open(dir.path()).unwrap();
        let a = db.new_account("a@x.com", None, None).await.unwrap();
        // 3-message thread all INBOX+UNREAD on first
        for (i, unread) in [(1, true), (2, false), (3, false)] {
            db.messages_upsert(crate::db::messages::MsgUpsert {
                id: format!("m{i}"),
                account_id: a.id.clone(),
                thread_id: "t1".into(),
                internal_date: i,
                subject: "s".into(),
                is_unread: unread,
                label_ids: if unread {
                    vec!["INBOX".into(), "UNREAD".into()]
                } else {
                    vec!["INBOX".into()]
                },
                ..Default::default()
            })
            .await
            .unwrap();
        }
        // archive via apply
        for mid in ["m1", "m2", "m3"] {
            db.apply_label_change(mid, &[], &["INBOX".to_string()])
                .await
                .unwrap();
        }
        let inbox: i64 = db
            .read({
                let aid = a.id.clone();
                move |c| {
                    Ok(c.query_row(
                        "SELECT in_inbox FROM threads WHERE account_id=? AND id='t1'",
                        rusqlite::params![aid],
                        |r| r.get(0),
                    )?)
                }
            })
            .await
            .unwrap();
        assert_eq!(inbox, 0);
    }
    #[test]
    fn p6_t02_star_latest_only_shape() {
        let (add, remove, _, _) = label_sets(&ActionKind::Star { on: true });
        assert_eq!(add, vec!["STARRED"]);
        assert!(remove.is_empty());
    }
}
