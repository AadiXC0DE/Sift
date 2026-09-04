use super::{now_ms, Db};
use crate::dto::Account;
use anyhow::Result;
use rusqlite::params;

pub fn row_to_account(r: &rusqlite::Row) -> rusqlite::Result<Account> {
    Ok(Account {
        id: r.get("id")?,
        provider: r.get("provider")?,
        email: r.get("email")?,
        display_name: r.get("display_name")?,
        avatar_url: r.get("avatar_url")?,
        color: r.get("color")?,
        history_id: r.get("history_id")?,
        sync_state: r.get("sync_state")?,
        last_sync_at: r.get("last_sync_at")?,
        created_at: r.get("created_at")?,
        sort_order: r.get("sort_order")?,
        signature_html: r.get("signature_html")?,
    })
}

impl Db {
    pub async fn accounts_list(&self) -> Result<Vec<Account>> {
        self.read(|c| {
            let mut s = c.prepare("SELECT * FROM accounts ORDER BY sort_order, email")?;
            let rows = s
                .query_map([], row_to_account)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }
    pub async fn accounts_insert(&self, a: &Account) -> Result<()> {
        let a = a.clone();
        self.write(move |c| {
      c.execute("INSERT OR REPLACE INTO accounts (id,provider,email,display_name,avatar_url,color,history_id,sync_state,last_sync_at,created_at,sort_order,signature_html) VALUES (?,?,?,?,?,?,?,?,?,?,?,?)",
        params![a.id,a.provider,a.email,a.display_name,a.avatar_url,a.color,a.history_id,a.sync_state,a.last_sync_at,a.created_at,a.sort_order,a.signature_html])?;
      Ok(())
    }).await
    }
    pub async fn accounts_get(&self, id: &str) -> Result<Option<Account>> {
        let id = id.to_string();
        self.read(move |c| {
            let mut s = c.prepare("SELECT * FROM accounts WHERE id=?")?;
            let mut rows = s.query_map(params![id], row_to_account)?;
            let __out = rows.next().transpose()?;
            Ok(__out)
        })
        .await
    }
    pub async fn accounts_remove(&self, id: &str) -> Result<()> {
        let id = id.to_string();
        self.write(move |c| {
            c.execute("DELETE FROM accounts WHERE id=?", params![id])?;
            Ok(())
        })
        .await
    }
    pub async fn accounts_update_meta(
        &self,
        id: &str,
        color: Option<String>,
        display_name: Option<String>,
        sig: Option<String>,
        order: Option<i64>,
    ) -> Result<()> {
        let (id, color, display_name, sig, order) =
            (id.to_string(), color, display_name, sig, order);
        self.write(move |c| {
            if let Some(v) = color {
                c.execute("UPDATE accounts SET color=? WHERE id=?", params![v, id])?;
            }
            if let Some(v) = display_name {
                c.execute(
                    "UPDATE accounts SET display_name=? WHERE id=?",
                    params![v, id],
                )?;
            }
            if let Some(v) = sig {
                c.execute(
                    "UPDATE accounts SET signature_html=? WHERE id=?",
                    params![v, id],
                )?;
            }
            if let Some(v) = order {
                c.execute(
                    "UPDATE accounts SET sort_order=? WHERE id=?",
                    params![v, id],
                )?;
            }
            Ok(())
        })
        .await
    }
    pub async fn accounts_set_state(&self, id: &str, state: &str) -> Result<()> {
        let (id, state) = (id.to_string(), state.to_string());
        self.write(move |c| {
            c.execute(
                "UPDATE accounts SET sync_state=? WHERE id=?",
                params![state, id],
            )?;
            Ok(())
        })
        .await
    }
    pub async fn accounts_set_history(&self, id: &str, hid: &str, at: i64) -> Result<()> {
        let (id, hid) = (id.to_string(), hid.to_string());
        self.write(move |c| {
            c.execute(
                "UPDATE accounts SET history_id=?, last_sync_at=? WHERE id=?",
                params![hid, at, id],
            )?;
            Ok(())
        })
        .await
    }
    pub async fn new_account(
        &self,
        email: &str,
        name: Option<String>,
        avatar: Option<String>,
    ) -> Result<Account> {
        let a = Account {
            id: uuid::Uuid::now_v7().to_string(),
            provider: "gmail".into(),
            email: email.into(),
            display_name: name,
            avatar_url: avatar,
            color: "blue".into(),
            history_id: None,
            sync_state: "new".into(),
            last_sync_at: None,
            created_at: now_ms(),
            sort_order: 0,
            signature_html: None,
        };
        self.accounts_insert(&a).await?;
        Ok(a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn p2_t07_remove_cascades() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let a = db.new_account("a@x.com", None, None).await.unwrap();
        let b = db.new_account("b@x.com", None, None).await.unwrap();
        db.write({
      let (aid, bid) = (a.id.clone(), b.id.clone());
      move |c| {
        c.execute("INSERT INTO labels (account_id,id,name,kind) VALUES (?,'INBOX','Inbox','system')", params![aid])?;
        c.execute("INSERT INTO labels (account_id,id,name,kind) VALUES (?,'INBOX','Inbox','system')", params![bid])?;
        c.execute("INSERT INTO threads (account_id,id,subject,last_message_at,first_message_at) VALUES (?, 't1','s',1,1)", params![aid])?;
        c.execute("INSERT INTO threads (account_id,id,subject,last_message_at,first_message_at) VALUES (?, 't1','s',1,1)", params![bid])?;
        Ok(())
      }
    }).await.unwrap();
        db.accounts_remove(&a.id).await.unwrap();
        let bid = b.id.clone();
        let n: i64 = db
            .read(move |c| -> anyhow::Result<i64> {
                Ok(c.query_row(
                    "SELECT count(*) FROM threads WHERE account_id=?",
                    params![bid],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap_or(0);
        // b untouched
        let bthreads: i64 = db
            .read(move |c| {
                let x: i64 = c.query_row("SELECT count(*) FROM threads", [], |r| r.get(0))?;
                Ok(x)
            })
            .await
            .unwrap();
        assert_eq!(bthreads, 1);
        let _ = n;
    }
}
