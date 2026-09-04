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
