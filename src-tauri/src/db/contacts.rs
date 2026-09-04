use super::Db;
use crate::dto::Contact;
use anyhow::Result;
use rusqlite::params;

impl Db {
    pub async fn contacts_upsert(
        &self,
        account_id: &str,
        email: &str,
        name: Option<&str>,
    ) -> Result<()> {
        let (a, e, n) = (
            account_id.to_string(),
            email.to_lowercase(),
            name.map(|s| s.to_string()),
        );
        let now = super::now_ms();
        self.write(move |c| {
      c.execute("INSERT INTO contacts (account_id,email,name,last_used_at,use_count) VALUES (?,?,?, ?,1) ON CONFLICT(account_id,email) DO UPDATE SET name=COALESCE(excluded.name, contacts.name), last_used_at=excluded.last_used_at, use_count=contacts.use_count+1",
        params![a,e,n,now])?;
      Ok(())
    }).await
    }
    pub async fn contacts_suggest(
        &self,
        account_id: &str,
        q: &str,
        limit: i64,
    ) -> Result<Vec<Contact>> {
        let (a, qq) = (account_id.to_string(), format!("%{}%", q.to_lowercase()));
        // trigram FTS when available, fallback to LIKE
        self.read(move |c| {
      let mut out = vec![];
      // try FTS trigram match first
      let fts: Result<Vec<(String, Option<String>)>, rusqlite::Error> = (|| -> Result<Vec<(String, Option<String>)>, rusqlite::Error> {
        let mut s = c.prepare("SELECT email, name FROM contacts_fts WHERE contacts_fts MATCH ? LIMIT ?")?;
        { let __v: Vec<(String, Option<String>)> = s.query_map(params![qq.clone(), limit], |r| Ok::<(String, Option<String>), rusqlite::Error>((r.get(0)?, r.get(1)?)))?.collect::<Result<Vec<(String, Option<String>)>, rusqlite::Error>>()?; Ok(__v) }
      })();
      if let Ok(rows) = fts {
        for (email, name) in rows {
          let (lu, uc): (i64, i64) = c.query_row("SELECT last_used_at, use_count FROM contacts WHERE account_id=? AND email=?", params![a, email], |r| Ok((r.get(0)?, r.get(1)?))).unwrap_or((0,1));
          out.push(Contact { email, name, last_used_at: lu, use_count: uc });
        }
        if !out.is_empty() { return Ok(out); }
      }
      let mut s = c.prepare("SELECT email,name,last_used_at,use_count FROM contacts WHERE account_id=? AND (lower(email) LIKE ? OR lower(COALESCE(name,'')) LIKE ?) ORDER BY use_count DESC, last_used_at DESC LIMIT ?")?;
      { let __v = s.query_map(params![a,qq,qq,limit], |r| Ok(Contact { email: r.get(0)?, name: r.get(1)?, last_used_at: r.get(2)?, use_count: r.get(3)? }))?.collect::<Result<Vec<_>,_>>()?; Ok(__v) }
    }).await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn p7_t07_suggest_orders() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        db.contacts_upsert("a", "ada@x.com", Some("Ada Lovelace"))
            .await
            .unwrap();
        db.contacts_upsert("a", "ada@x.com", Some("Ada Lovelace"))
            .await
            .unwrap();
        db.contacts_upsert("a", "ben@y.org", Some("Ben"))
            .await
            .unwrap();
        let r = db.contacts_suggest("a", "ada", 10).await.unwrap();
        assert!(!r.is_empty());
        assert_eq!(r[0].email, "ada@x.com");
    }
}
