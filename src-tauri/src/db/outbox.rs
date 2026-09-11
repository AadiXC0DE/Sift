use super::Db;
use anyhow::Result;
use rusqlite::params;

#[derive(Debug, Clone)]
pub struct Op {
    pub id: i64,
    pub account_id: String,
    pub kind: String,
    pub payload: String,
    pub undo_group: Option<String>,
    pub state: String,
    pub attempts: i64,
    pub not_before: i64,
}

impl Db {
    pub async fn outbox_enqueue(
        &self,
        account_id: &str,
        kind: &str,
        payload: &str,
        undo_group: Option<&str>,
        not_before: i64,
    ) -> Result<i64> {
        let (a, k, p, u, n) = (
            account_id.to_string(),
            kind.to_string(),
            payload.to_string(),
            undo_group.map(|s| s.to_string()),
            not_before,
        );
        self.write(move |c| {
      c.execute("INSERT INTO outbox_ops (account_id,kind,payload,undo_group,not_before,created_at) VALUES (?,?,?,?,?,?)",
        params![a,k,p,u,n,super::now_ms()])?;
      Ok(c.last_insert_rowid())
    }).await
    }
    pub async fn outbox_next(&self, account_id: &str) -> Result<Option<Op>> {
        let a = account_id.to_string();
        let now = super::now_ms();
        self.read(move |c| {
      let mut s = c.prepare("SELECT id,account_id,kind,payload,undo_group,state,attempts,not_before FROM outbox_ops WHERE account_id=? AND state='pending' AND not_before<=? ORDER BY id LIMIT 1")?;
      let mut rows = s.query_map(params![a,now], |r| Ok(Op { id: r.get(0)?, account_id: r.get(1)?, kind: r.get(2)?, payload: r.get(3)?, undo_group: r.get(4)?, state: r.get(5)?, attempts: r.get(6)?, not_before: r.get(7)? }))?;
      let __out = rows.next().transpose()?; Ok(__out)
    }).await
    }
    pub async fn outbox_set(
        &self,
        id: i64,
        state: &str,
        attempts: i64,
        not_before: i64,
        err: Option<String>,
    ) -> Result<()> {
        let state = state.to_string();
        self.write(move |c| {
            c.execute(
                "UPDATE outbox_ops SET state=?, attempts=?, not_before=?, last_error=? WHERE id=?",
                params![state, attempts, not_before, err, id],
            )?;
            Ok(())
        })
        .await
    }
    pub async fn outbox_cancel_group(&self, undo_group: &str) -> Result<Vec<Op>> {
        let u = undo_group.to_string();
        let ops: Vec<Op> = self.read({
      let u = u.clone();
      move |c| {
        let mut s = c.prepare("SELECT id,account_id,kind,payload,undo_group,state,attempts,not_before FROM outbox_ops WHERE undo_group=? AND state='pending'")?;
        { let __v = s.query_map(params![u], |r| Ok(Op { id: r.get(0)?, account_id: r.get(1)?, kind: r.get(2)?, payload: r.get(3)?, undo_group: r.get(4)?, state: r.get(5)?, attempts: r.get(6)?, not_before: r.get(7)? }))?.collect::<Result<Vec<_>,_>>()?; Ok(__v) }
      }
    }).await?;
        for op in &ops {
            self.outbox_set(op.id, "cancelled", op.attempts, op.not_before, None)
                .await?;
        }
        Ok(ops)
    }
    pub async fn outbox_done_group(&self, undo_group: &str) -> Result<Vec<Op>> {
        let u = undo_group.to_string();
        self.read(move |c| {
      let mut s = c.prepare("SELECT id,account_id,kind,payload,undo_group,state,attempts,not_before FROM outbox_ops WHERE undo_group=? AND state='done'")?;
      { let __v = s.query_map(params![u], |r| Ok(Op { id: r.get(0)?, account_id: r.get(1)?, kind: r.get(2)?, payload: r.get(3)?, undo_group: r.get(4)?, state: r.get(5)?, attempts: r.get(6)?, not_before: r.get(7)? }))?.collect::<Result<Vec<_>,_>>()?; Ok(__v) }
    }).await
    }
    pub async fn outbox_pending_count(&self, account_id: &str) -> Result<i64> {
        let a = account_id.to_string();
        self.read(move |c| Ok(c.query_row("SELECT count(*) FROM outbox_ops WHERE account_id=? AND state IN ('pending','inflight')", params![a], |r| r.get(0))?)).await
    }

    /// Pending ops grouped into human labels, so the UI can say what is still
    /// being sent instead of a bare number.
    pub async fn outbox_summary(&self, account_id: &str) -> Result<Vec<(String, i64)>> {
        let a = account_id.to_string();
        self.read(move |c| {
            let mut s = c.prepare(
                "SELECT kind, payload, count(*) FROM outbox_ops \
                 WHERE account_id=? AND state IN ('pending','inflight') GROUP BY kind, payload",
            )?;
            let rows = s
                .query_map(params![a], |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            let mut map: std::collections::BTreeMap<String, i64> =
                std::collections::BTreeMap::new();
            for (kind, payload, n) in rows {
                *map.entry(op_label(&kind, &payload).to_string())
                    .or_insert(0) += n;
            }
            Ok(map.into_iter().collect())
        })
        .await
    }

    /// Drop every op for an account. Called when the account is removed so its
    /// queued work can never resurface as "changes pending".
    pub async fn outbox_delete_account(&self, account_id: &str) -> Result<()> {
        let a = account_id.to_string();
        self.write(move |c| {
            c.execute("DELETE FROM outbox_ops WHERE account_id=?", params![a])?;
            Ok(())
        })
        .await
    }
}

/// Human label for a queued op, derived from its kind and payload.
pub fn op_label(kind: &str, payload: &str) -> &'static str {
    if kind == "modify_labels" {
        let v: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let has = |field: &str, val: &str| {
            v.get(field)
                .and_then(|a| a.as_array())
                .map(|a| a.iter().any(|x| x.as_str() == Some(val)))
                .unwrap_or(false)
        };
        if has("add", "TRASH") {
            return "Moving to trash";
        }
        if has("add", "SPAM") {
            return "Marking as spam";
        }
        if has("add", "INBOX") {
            return "Moving to inbox";
        }
        if has("remove", "UNREAD") {
            return "Marking as read";
        }
        if has("add", "UNREAD") {
            return "Marking as unread";
        }
        if has("add", "STARRED") {
            return "Starring";
        }
        if has("remove", "STARRED") {
            return "Unstarring";
        }
        if has("remove", "INBOX") {
            return "Archiving";
        }
        return "Updating labels";
    }
    match kind {
        "trash" => "Moving to trash",
        "untrash" => "Moving to inbox",
        "spam" => "Marking as spam",
        "unspam" => "Moving to inbox",
        "delete" => "Deleting",
        "send" => "Sending",
        "draft_upsert" => "Saving draft",
        "draft_delete" => "Deleting draft",
        _ => "Applying change",
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn p6_t04_cancel_pending() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        db.outbox_enqueue("a", "modify_labels", "{}", Some("g1"), 0)
            .await
            .unwrap();
        let ops = db.outbox_cancel_group("g1").await.unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(db.outbox_pending_count("a").await.unwrap(), 0);
    }
}
