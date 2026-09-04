use crate::db::Db;
use anyhow::Result;

/// Compute next snooze wake (ms) or None.
pub async fn next_wake(db: &Db) -> Result<Option<i64>> {
    db.read(|c| {
        let mut v: Option<i64> = None;
        let mut s = c.prepare("SELECT MIN(wake_at) FROM snoozes")?;
        let mut rows = s.query([])?;
        if let Some(r) = rows.next()? {
            v = r.get(0)?;
        }
        Ok(v)
    })
    .await
}

pub async fn process_overdue(db: &Db, now: i64) -> Result<Vec<(String, String)>> {
    let rows: Vec<(String, String, i64)> = db
        .read(move |c| {
            let mut s =
                c.prepare("SELECT account_id, thread_id, wake_at FROM snoozes WHERE wake_at <= ?")?;
            {
                let __v = s
                    .query_map(rusqlite::params![now], |r| {
                        Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(__v)
            }
        })
        .await?;
    let mut out = vec![];
    for (a, t, _w) in rows {
        // restore INBOX locally (op enqueued by caller)
        let (a2, t2) = (a.clone(), t.clone());
        db.write(move |c| {
            c.execute(
                "DELETE FROM snoozes WHERE account_id=? AND thread_id=?",
                rusqlite::params![a2, t2],
            )?;
            c.execute(
                "UPDATE threads SET snoozed_until=NULL WHERE account_id=? AND id=?",
                rusqlite::params![a2, t2],
            )?;
            Ok(())
        })
        .await?;
        out.push((a, t));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn p6_t08_wake_restores() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let a = db.new_account("a@x.com", None, None).await.unwrap();
        db.write({
      let aid = a.id.clone();
      move |c| {
        c.execute("INSERT INTO threads (account_id,id,subject,last_message_at,first_message_at,snoozed_until) VALUES (?,?, 's', 1, 1, 1)", rusqlite::params![aid, "t1"])?;
        c.execute("INSERT INTO snoozes (account_id,thread_id,wake_at) VALUES (?,?,?)", rusqlite::params![aid, "t1", 1])?;
        Ok(())
      }
    }).await.unwrap();
        let w = process_overdue(&db, 10).await.unwrap();
        assert_eq!(w.len(), 1);
        let w2 = next_wake(&db).await.unwrap();
        assert_eq!(w2, None);
        // overdue at startup processed once
        let w3 = process_overdue(&db, 10).await.unwrap();
        assert!(w3.is_empty());
    }
}
