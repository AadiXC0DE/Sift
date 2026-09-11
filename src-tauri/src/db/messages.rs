use super::Db;
use anyhow::Result;
use rusqlite::params;

#[derive(Debug, Clone)]
pub struct MsgUpsert {
    pub id: String,
    pub account_id: String,
    pub thread_id: String,
    pub history_id: Option<String>,
    pub internal_date: i64,
    pub from_name: Option<String>,
    pub from_email: Option<String>,
    pub to_json: String,
    pub cc_json: String,
    pub bcc_json: String,
    pub reply_to: Option<String>,
    pub subject: String,
    pub snippet: String,
    pub rfc_message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references_json: String,
    pub list_unsubscribe: Option<String>,
    pub list_unsubscribe_post: bool,
    pub size_estimate: Option<i64>,
    pub has_attachments: bool,
    pub is_unread: bool,
    pub is_starred: bool,
    pub is_draft: bool,
    pub is_sent_by_me: bool,
    pub label_ids: Vec<String>,
}
impl Default for MsgUpsert {
    fn default() -> Self {
        Self {
            id: String::new(),
            account_id: String::new(),
            thread_id: String::new(),
            history_id: None,
            internal_date: 0,
            from_name: None,
            from_email: None,
            to_json: "[]".into(),
            cc_json: "[]".into(),
            bcc_json: "[]".into(),
            reply_to: None,
            subject: String::new(),
            snippet: String::new(),
            rfc_message_id: None,
            in_reply_to: None,
            references_json: "[]".into(),
            list_unsubscribe: None,
            list_unsubscribe_post: false,
            size_estimate: None,
            has_attachments: false,
            is_unread: false,
            is_starred: false,
            is_draft: false,
            is_sent_by_me: false,
            label_ids: vec![],
        }
    }
}

impl Db {
    pub async fn messages_upsert(&self, m: MsgUpsert) -> Result<()> {
        let account_id = m.account_id.clone();
        let thread_id = m.thread_id.clone();
        self.write(move |c| {
      let labels_json = serde_json::to_string(&m.label_ids)?;
      c.execute("INSERT INTO messages (id,account_id,thread_id,history_id,internal_date,from_name,from_email,to_json,cc_json,bcc_json,reply_to,subject,snippet,rfc_message_id,in_reply_to,references_json,list_unsubscribe,list_unsubscribe_post,size_estimate,has_attachments,is_unread,is_starred,is_draft,is_sent_by_me,label_ids,body_state) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,COALESCE((SELECT body_state FROM messages WHERE id=?),'none'))
        ON CONFLICT(id) DO UPDATE SET account_id=excluded.account_id, thread_id=excluded.thread_id, history_id=excluded.history_id, internal_date=excluded.internal_date, from_name=excluded.from_name, from_email=excluded.from_email, to_json=excluded.to_json, cc_json=excluded.cc_json, bcc_json=excluded.bcc_json, reply_to=excluded.reply_to, subject=excluded.subject, snippet=excluded.snippet, label_ids=excluded.label_ids, is_unread=excluded.is_unread, is_starred=excluded.is_starred, is_draft=excluded.is_draft, has_attachments=MAX(messages.has_attachments, excluded.has_attachments)",
        params![m.id,m.account_id,m.thread_id,m.history_id,m.internal_date,m.from_name,m.from_email,m.to_json,m.cc_json,m.bcc_json,m.reply_to,m.subject,m.snippet,m.rfc_message_id,m.in_reply_to,m.references_json,m.list_unsubscribe,m.list_unsubscribe_post as i32,m.size_estimate,m.has_attachments as i32,m.is_unread as i32,m.is_starred as i32,m.is_draft as i32,m.is_sent_by_me as i32,labels_json,m.id])?;
      c.execute("DELETE FROM message_labels WHERE message_id=?", params![m.id])?;
      for l in &m.label_ids {
        c.execute("INSERT OR IGNORE INTO message_labels (message_id,label_id) VALUES (?,?)", params![m.id, l])?;
      }
      // FTS subject/from/to
      let from_t = format!("{} {}", m.from_name.clone().unwrap_or_default(), m.from_email.clone().unwrap_or_default());
      c.execute("INSERT OR IGNORE INTO messages_fts (message_id,account_id,subject,from_text,to_text,body) VALUES (?,?,?,?,?,?)",
        params![m.id, m.account_id, m.subject, from_t, m.to_json, ""])?;
      Ok(())
    }).await?;
        self.recompute_thread(&account_id, &thread_id).await
    }

    pub async fn messages_delete(&self, id: &str, account_id: &str, thread_id: &str) -> Result<()> {
        let (id, a, t) = (
            id.to_string(),
            account_id.to_string(),
            thread_id.to_string(),
        );
        self.write(move |c| {
            c.execute("DELETE FROM messages WHERE id=?", params![id])?;
            c.execute("DELETE FROM messages_fts WHERE message_id=?", params![id])?;
            Ok(())
        })
        .await?;
        self.recompute_thread(&a, &t).await
    }

    pub async fn apply_label_change(
        &self,
        message_id: &str,
        add: &[String],
        remove: &[String],
    ) -> Result<(String, String)> {
        let (mid, add, remove) = (message_id.to_string(), add.to_vec(), remove.to_vec());
        let info: (String, String) = self
            .read({
                let mid = mid.clone();
                move |c| {
                    Ok(c.query_row(
                        "SELECT account_id, thread_id FROM messages WHERE id=?",
                        params![mid],
                        |r| Ok((r.get(0)?, r.get(1)?)),
                    )?)
                }
            })
            .await?;
        let (aid, tid) = info.clone();
        self.write(move |c| {
            for l in &add {
                c.execute(
                    "INSERT OR IGNORE INTO message_labels (message_id,label_id) VALUES (?,?)",
                    params![mid, l],
                )?;
            }
            for l in &remove {
                c.execute(
                    "DELETE FROM message_labels WHERE message_id=? AND label_id=?",
                    params![mid, l],
                )?;
            }
            // refresh flags
            let has = |lid: &str| -> rusqlite::Result<bool> {
                c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM message_labels WHERE message_id=? AND label_id=?)",
                    params![mid, lid],
                    |r| r.get(0),
                )
            };
            let unread = has("UNREAD")?;
            let starred = has("STARRED")?;
            let draft = has("DRAFT")?;
            let labels: Vec<String> = c
                .prepare("SELECT label_id FROM message_labels WHERE message_id=?")?
                .query_map(params![mid], |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let lj = serde_json::to_string(&labels).unwrap_or("[]".into());
            c.execute(
                "UPDATE messages SET is_unread=?, is_starred=?, is_draft=?, label_ids=? WHERE id=?",
                params![unread as i32, starred as i32, draft as i32, lj, mid],
            )?;
            Ok(())
        })
        .await?;
        // update FTS? no need
        self.recompute_thread(&aid, &tid).await?;
        Ok((aid, tid))
    }

    pub async fn next_bodies_to_fetch(
        &self,
        account: &str,
        limit: i64,
        min_date: i64,
    ) -> Result<Vec<String>> {
        let (a, l, m) = (account.to_string(), limit, min_date);
        self.read(move |c| -> anyhow::Result<Vec<String>> {
      let mut s = c.prepare("SELECT id FROM messages WHERE account_id=? AND body_state='none' AND internal_date>=? ORDER BY internal_date DESC LIMIT ?")?;
      let v: Vec<String> = s.query_map(params![a,m,l], |r| r.get(0))?.collect::<Result<Vec<String>, rusqlite::Error>>()?;
      Ok(v)
    }).await
    }

    pub async fn message_flags(&self, message_id: &str) -> Result<(bool, bool)> {
        let mid = message_id.to_string();
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT is_unread, is_starred FROM messages WHERE id=?",
                rusqlite::params![mid],
                |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0)),
            )
            .unwrap_or((false, false)))
        })
        .await
    }
    pub async fn message_snippet(&self, message_id: &str) -> Result<String> {
        let mid = message_id.to_string();
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT snippet FROM messages WHERE id=?",
                rusqlite::params![mid],
                |r| r.get::<_, String>(0),
            )
            .unwrap_or_default())
        })
        .await
    }
    pub async fn set_snippet(&self, message_id: &str, snippet: &str) -> Result<()> {
        let (mid, sn) = (message_id.to_string(), snippet.to_string());
        self.write(move |c| {
            c.execute(
                "UPDATE messages SET snippet=? WHERE id=?",
                rusqlite::params![sn, mid],
            )?;
            Ok(())
        })
        .await
    }
    pub async fn message_thread(&self, message_id: &str) -> Result<Option<(String, String)>> {
        let mid = message_id.to_string();
        self.read(move |c| {
            let mut s = c.prepare("SELECT account_id, thread_id FROM messages WHERE id=?")?;
            let mut rows = s.query_map(params![mid], |r| {
                Ok::<(String, String), rusqlite::Error>((r.get(0)?, r.get(1)?))
            })?;
            let __out = rows.next().transpose()?;
            Ok(__out)
        })
        .await
    }
}
