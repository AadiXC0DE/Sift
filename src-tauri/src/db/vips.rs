//! VIP senders and the notification delivery log (P8.4).
//!
//! VIPs are chosen from `contacts` — the addresses Sift has actually seen —
//! so the feature needs no address-book permission and no Gmail contact scope.
//! The delivery log is what makes "one message, one notification" durable: the
//! claim is an `INSERT OR IGNORE` on `(account_id, message_id)`, so a repeat
//! sync, a restart or a racing scheduler tick cannot raise a second banner.

use super::Db;
use anyhow::Result;
use rusqlite::params;

impl Db {
    pub async fn vip_list(&self, account_id: &str) -> Result<Vec<String>> {
        let a = account_id.to_string();
        self.read(move |c| {
            let mut s = c.prepare(
                "SELECT email FROM vip_senders WHERE account_id=? ORDER BY email",
            )?;
            let rows = s
                .query_map(params![a], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Add or remove one VIP. The email is normalised the same way `contacts`
    /// normalises it, so a VIP selected from a recent contact matches the
    /// address sync will see.
    pub async fn vip_set(&self, account_id: &str, email: &str, vip: bool) -> Result<Vec<String>> {
        let (a, e) = (
            account_id.to_string(),
            email.trim().to_ascii_lowercase(),
        );
        let now = super::now_ms();
        self.write(move |c| {
            if vip {
                c.execute(
                    "INSERT OR IGNORE INTO vip_senders (account_id,email,created_at) VALUES (?,?,?)",
                    params![a, e, now],
                )?;
            } else {
                c.execute(
                    "DELETE FROM vip_senders WHERE account_id=? AND email=?",
                    params![a, e],
                )?;
            }
            Ok(())
        })
        .await?;
        self.vip_list(account_id).await
    }

    pub async fn vip_is_vip(&self, account_id: &str, email: &str) -> Result<bool> {
        let (a, e) = (
            account_id.to_string(),
            email.trim().to_ascii_lowercase(),
        );
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM vip_senders WHERE account_id=? AND email=?)",
                params![a, e],
                |r| r.get(0),
            )?)
        })
        .await
    }

    /// The VIP addresses of several accounts at once, keyed by account.
    pub async fn vip_map(&self, account_ids: &[String]) -> Result<Vec<(String, String)>> {
        let accounts = account_ids.to_vec();
        self.read(move |c| {
            if accounts.is_empty() {
                return Ok(vec![]);
            }
            let holes = vec!["?"; accounts.len()].join(",");
            let sql = format!(
                "SELECT account_id, email FROM vip_senders WHERE account_id IN ({holes})"
            );
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![];
            for a in &accounts {
                binds.push(Box::new(a.clone()));
            }
            let rows = c
                .prepare(&sql)?
                .query_map(
                    rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())),
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?
                .collect::<Result<Vec<(String, String)>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Claim the right to notify about one message. `false` means it was
    /// already reported.
    pub async fn notify_claim(
        &self,
        account_id: &str,
        message_id: &str,
        thread_id: &str,
    ) -> Result<bool> {
        let (a, m, t) = (
            account_id.to_string(),
            message_id.to_string(),
            thread_id.to_string(),
        );
        let now = super::now_ms();
        self.write(move |c| {
            let n = c.execute(
                "INSERT OR IGNORE INTO notify_log (account_id,message_id,thread_id,notified_at) \
                 VALUES (?,?,?,?)",
                params![a, m, t, now],
            )?;
            Ok(n == 1)
        })
        .await
    }

    /// Whether this message was already reported. Used by tests and by the
    /// startup badge pass, which must not re-notify after a restart.
    pub async fn notify_seen(&self, account_id: &str, message_id: &str) -> Result<bool> {
        let (a, m) = (account_id.to_string(), message_id.to_string());
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT EXISTS(SELECT 1 FROM notify_log WHERE account_id=? AND message_id=?)",
                params![a, m],
                |r| r.get(0),
            )?)
        })
        .await
    }

    /// Drop delivery records past their retention window. The table only stops
    /// duplicate banners, so a bounded history is enough.
    pub async fn notify_prune(&self, retention_ms: i64) -> Result<usize> {
        self.write(move |c| {
            let n = c.execute(
                "DELETE FROM notify_log WHERE notified_at < ?",
                params![super::now_ms() - retention_ms],
            )?;
            Ok(n)
        })
        .await
    }

    /// The sender and Inbox membership of a message that just arrived.
    ///
    /// Read from the database rather than from the transport's summary, so the
    /// notification filter decides on the state Sift actually stored — the same
    /// state the list shows.
    pub async fn notify_context(
        &self,
        account_id: &str,
        message_id: &str,
    ) -> Result<(String, bool)> {
        let (a, m) = (account_id.to_string(), message_id.to_string());
        self.read(move |c| {
            let sender: String = c
                .query_row(
                    "SELECT COALESCE(from_email,'') FROM messages WHERE account_id=? AND id=?",
                    params![a, m],
                    |r| r.get(0),
                )
                .unwrap_or_default();
            let in_inbox: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM message_labels \
                   WHERE account_id=? AND message_id=? AND label_id='INBOX')",
                params![a, m],
                |r| r.get(0),
            )?;
            Ok((sender, in_inbox))
        })
        .await
    }

    /// The dock badge count: unread threads in the Inbox, computed with the
    /// exact predicate the sidebar's Inbox count uses (`threads.label_ids`
    /// carrying INBOX, excluding Trash and Junk). Rust owns this number so the
    /// OS badge and the UI cannot disagree (P8.4).
    pub async fn unread_inbox_count(
        &self,
        account_ids: &[String],
        excluded_accounts: &[String],
    ) -> Result<i64> {
        let accounts = account_ids.to_vec();
        let excluded = excluded_accounts.to_vec();
        self.read(move |c| {
            let mut where_sql = String::from("1=1");
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![];
            if !accounts.is_empty() {
                let holes = vec!["?"; accounts.len()].join(",");
                where_sql.push_str(&format!(" AND t.account_id IN ({holes})"));
                for a in &accounts {
                    binds.push(Box::new(a.clone()));
                }
            }
            if !excluded.is_empty() {
                let holes = vec!["?"; excluded.len()].join(",");
                where_sql.push_str(&format!(" AND t.account_id NOT IN ({holes})"));
                for a in &excluded {
                    binds.push(Box::new(a.clone()));
                }
            }
            let sql = format!(
                "SELECT COUNT(*) FROM threads t, json_each(t.label_ids) \
                 WHERE value='INBOX' AND t.in_trash=0 AND t.in_spam=0 AND t.unread_count>0 AND {where_sql}"
            );
            let n: i64 = c.query_row(
                &sql,
                rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())),
                |r| r.get(0),
            )?;
            Ok(n)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> Db {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        std::mem::forget(dir);
        db
    }

    async fn account(db: &Db, id: &str) {
        let id = id.to_string();
        db.write(move |c| {
            c.execute(
                "INSERT OR IGNORE INTO accounts (id,email,created_at) VALUES (?1,?1||'@x',0)",
                params![id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn p8_4_vip_set_is_account_scoped_and_normalised() {
        let db = db().await;
        account(&db, "a").await;
        account(&db, "b").await;
        db.vip_set("a", " Ada@Example.com ", true).await.unwrap();
        assert_eq!(db.vip_list("a").await.unwrap(), vec!["ada@example.com"]);
        assert!(db.vip_is_vip("a", "ada@example.com").await.unwrap());
        // Another account is unaffected by the first account's list.
        assert!(!db.vip_is_vip("b", "ada@example.com").await.unwrap());
        db.vip_set("a", "ada@example.com", false).await.unwrap();
        assert!(db.vip_list("a").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn p8_4_one_message_notifies_once_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().to_path_buf();
        {
            let db = Db::open(&path).unwrap();
            account(&db, "a").await;
            assert!(db.notify_claim("a", "m1", "t1").await.unwrap());
            // A second sync of the same message must not raise it again...
            assert!(!db.notify_claim("a", "m1", "t1").await.unwrap());
        }
        // ...and neither must a restart, because the record is on disk.
        let db = Db::open(&path).unwrap();
        assert!(db.notify_seen("a", "m1").await.unwrap());
        assert!(!db.notify_claim("a", "m1", "t1").await.unwrap());
        std::mem::forget(dir);
    }
}
