use super::Db;
use crate::dto::MessageRef;
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
    /// The exact `List-Unsubscribe-Post` field value (P9.4). The derived
    /// boolean is not enough to decide whether one-click is really offered.
    pub list_unsubscribe_post_value: Option<String>,
    /// `Authentication-Results` exactly as the provider returned it.
    pub auth_results: Option<String>,
    /// True only when the provider itself produced `auth_results`.
    pub auth_results_trusted: bool,
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
            list_unsubscribe_post_value: None,
            auth_results: None,
            auth_results_trusted: false,
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

/// One message upsert on an existing connection/transaction (P4.5), so a
/// partial-sync batch can write every row and its checkpoint atomically.
/// The thread row is NOT recomputed here: the batch recomputes each affected
/// thread once, inside the same transaction (`recompute_thread_conn`).
pub(crate) fn messages_upsert_conn(c: &rusqlite::Connection, m: &MsgUpsert) -> Result<()> {
    let labels_json = serde_json::to_string(&m.label_ids)?;
    // Was this message already known? A re-ingest of an existing row is a
    // metadata refresh, not newly ingested mail, and must never be handed to
    // the rule engine (P8.3).
    let was_known: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM messages WHERE account_id=?1 AND id=?2)",
        params![m.account_id, m.id],
        |r| r.get(0),
    )?;
    c.execute("INSERT INTO messages (id,account_id,thread_id,history_id,internal_date,from_name,from_email,to_json,cc_json,bcc_json,reply_to,subject,snippet,rfc_message_id,in_reply_to,references_json,list_unsubscribe,list_unsubscribe_post,list_unsubscribe_post_value,auth_results,auth_results_trusted,size_estimate,has_attachments,is_unread,is_starred,is_draft,is_sent_by_me,label_ids,body_state) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,COALESCE((SELECT body_state FROM messages WHERE account_id=? AND id=?),'none'))
        ON CONFLICT(account_id,id) DO UPDATE SET account_id=excluded.account_id, thread_id=excluded.thread_id, history_id=excluded.history_id, internal_date=excluded.internal_date, from_name=excluded.from_name, from_email=excluded.from_email, to_json=excluded.to_json, cc_json=excluded.cc_json, bcc_json=excluded.bcc_json, reply_to=excluded.reply_to, subject=excluded.subject, snippet=excluded.snippet, label_ids=excluded.label_ids, is_unread=excluded.is_unread, is_starred=excluded.is_starred, is_draft=excluded.is_draft, has_attachments=MAX(messages.has_attachments, excluded.has_attachments), list_unsubscribe=COALESCE(excluded.list_unsubscribe, messages.list_unsubscribe), list_unsubscribe_post=MAX(messages.list_unsubscribe_post, excluded.list_unsubscribe_post), list_unsubscribe_post_value=COALESCE(excluded.list_unsubscribe_post_value, messages.list_unsubscribe_post_value), auth_results=COALESCE(excluded.auth_results, messages.auth_results), auth_results_trusted=MAX(messages.auth_results_trusted, excluded.auth_results_trusted)",
        params![m.id,m.account_id,m.thread_id,m.history_id,m.internal_date,m.from_name,m.from_email,m.to_json,m.cc_json,m.bcc_json,m.reply_to,m.subject,m.snippet,m.rfc_message_id,m.in_reply_to,m.references_json,m.list_unsubscribe,m.list_unsubscribe_post as i32,m.list_unsubscribe_post_value,m.auth_results,m.auth_results_trusted as i32,m.size_estimate,m.has_attachments as i32,m.is_unread as i32,m.is_starred as i32,m.is_draft as i32,m.is_sent_by_me as i32,labels_json,m.account_id,m.id])?;
    c.execute(
        "DELETE FROM message_labels WHERE account_id=? AND message_id=?",
        params![m.account_id, m.id],
    )?;
    for l in &m.label_ids {
        c.execute("INSERT OR IGNORE INTO message_labels (account_id,message_id,label_id) VALUES (?,?,?)", params![m.account_id, m.id, l])?;
    }
    // FTS subject/from/to. The row is keyed by (account_id, message_id);
    // an existing row keeps its indexed body, a new one starts empty and is
    // filled in by bodies_put.
    let from_t = format!(
        "{} {}",
        m.from_name.clone().unwrap_or_default(),
        m.from_email.clone().unwrap_or_default()
    );
    let updated = c.execute("UPDATE messages_fts SET subject=?3, from_text=?4, to_text=?5 WHERE account_id=?1 AND message_id=?2",
        params![m.account_id, m.id, m.subject, from_t, m.to_json])?;
    if updated == 0 {
        c.execute("INSERT INTO messages_fts (message_id,account_id,subject,from_text,to_text,body) VALUES (?,?,?,?,?,'')",
          params![m.id, m.account_id, m.subject, from_t, m.to_json])?;
    }
    // Rules evaluate newly ingested mail *after* the metadata commit (P8.3):
    // the queue row is written here, in the same transaction, and drained
    // later by the rule worker, outside the first-page path. Only genuinely
    // new mail that arrives after the account's first sync is queued.
    if !was_known {
        crate::db::rules::queue_new_message_conn(c, &m.account_id, &m.id, super::now_ms())?;
    }
    Ok(())
}

impl Db {
    /// Upsert one message keyed by its full identity `(account_id, id)`. The
    /// provider id alone is not unique across accounts (DB-02).
    pub async fn messages_upsert(&self, m: MsgUpsert) -> Result<()> {
        let account_id = m.account_id.clone();
        let thread_id = m.thread_id.clone();
        self.write(move |c| messages_upsert_conn(c, &m)).await?;
        self.recompute_thread(&account_id, &thread_id).await
    }

    /// Delete one message and its account-qualified children. The FTS row, the
    /// label rows, the body and the attachments all go through their foreign
    /// keys, which now include the account half.
    pub async fn messages_delete(&self, r: &MessageRef, thread_id: &str) -> Result<()> {
        let (aid, id, t) = (
            r.account_id.clone(),
            r.message_id.clone(),
            thread_id.to_string(),
        );
        self.write(move |c| {
            c.execute(
                "DELETE FROM messages WHERE account_id=? AND id=?",
                params![aid, id],
            )?;
            c.execute(
                "DELETE FROM messages_fts WHERE account_id=? AND message_id=?",
                params![aid, id],
            )?;
            Ok(())
        })
        .await?;
        self.recompute_thread(&r.account_id, &t).await
    }

    pub async fn apply_label_change(
        &self,
        r: &MessageRef,
        add: &[String],
        remove: &[String],
    ) -> Result<(String, String)> {
        let (aid, mid, add, remove) = (
            r.account_id.clone(),
            r.message_id.clone(),
            add.to_vec(),
            remove.to_vec(),
        );
        let tid: String = self
            .read({
                let (aid, mid) = (aid.clone(), mid.clone());
                move |c| {
                    Ok(c.query_row(
                        "SELECT thread_id FROM messages WHERE account_id=? AND id=?",
                        params![aid, mid],
                        |r| r.get(0),
                    )?)
                }
            })
            .await?;
        let (aid2, tid2) = (aid.clone(), tid.clone());
        self.write(move |c| {
            for l in &add {
                c.execute(
                    "INSERT OR IGNORE INTO message_labels (account_id,message_id,label_id) VALUES (?,?,?)",
                    params![aid, mid, l],
                )?;
            }
            for l in &remove {
                c.execute(
                    "DELETE FROM message_labels WHERE account_id=? AND message_id=? AND label_id=?",
                    params![aid, mid, l],
                )?;
            }
            // refresh flags
            let has = |lid: &str| -> rusqlite::Result<bool> {
                c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM message_labels WHERE account_id=? AND message_id=? AND label_id=?)",
                    params![aid, mid, lid],
                    |r| r.get(0),
                )
            };
            let unread = has("UNREAD")?;
            let starred = has("STARRED")?;
            let draft = has("DRAFT")?;
            let labels: Vec<String> = c
                .prepare("SELECT label_id FROM message_labels WHERE account_id=? AND message_id=?")?
                .query_map(params![aid, mid], |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let lj = serde_json::to_string(&labels).unwrap_or("[]".into());
            c.execute(
                "UPDATE messages SET is_unread=?, is_starred=?, is_draft=?, label_ids=? WHERE account_id=? AND id=?",
                params![unread as i32, starred as i32, draft as i32, lj, aid, mid],
            )?;
            Ok(())
        })
        .await?;
        self.recompute_thread(&aid2, &tid2).await?;
        Ok((aid2, tid2))
    }

    pub async fn next_bodies_to_fetch(
        &self,
        account: &str,
        limit: i64,
        min_date: i64,
    ) -> Result<Vec<MessageRef>> {
        let (a, l, m) = (account.to_string(), limit, min_date);
        self.read(move |c| -> anyhow::Result<Vec<MessageRef>> {
      let mut s = c.prepare("SELECT id FROM messages WHERE account_id=? AND body_state='none' AND internal_date>=? ORDER BY internal_date DESC LIMIT ?")?;
      let v: Vec<String> = s.query_map(params![a,m,l], |r| r.get(0))?.collect::<Result<Vec<String>, rusqlite::Error>>()?;
      Ok(v.into_iter().map(|message_id| MessageRef::new(a.clone(), message_id)).collect())
    }).await
    }

    pub async fn message_flags(&self, r: &MessageRef) -> Result<(bool, bool)> {
        let (aid, mid) = (r.account_id.clone(), r.message_id.clone());
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT is_unread, is_starred FROM messages WHERE account_id=? AND id=?",
                params![aid, mid],
                |r| Ok((r.get::<_, i64>(0)? != 0, r.get::<_, i64>(1)? != 0)),
            )
            .unwrap_or((false, false)))
        })
        .await
    }
    pub async fn message_snippet(&self, r: &MessageRef) -> Result<String> {
        let (aid, mid) = (r.account_id.clone(), r.message_id.clone());
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT snippet FROM messages WHERE account_id=? AND id=?",
                params![aid, mid],
                |r| r.get::<_, String>(0),
            )
            .unwrap_or_default())
        })
        .await
    }
    pub async fn set_snippet(&self, r: &MessageRef, snippet: &str) -> Result<()> {
        let (aid, mid, sn) = (r.account_id.clone(), r.message_id.clone(), snippet.to_string());
        self.write(move |c| {
            c.execute(
                "UPDATE messages SET snippet=? WHERE account_id=? AND id=?",
                params![sn, aid, mid],
            )?;
            Ok(())
        })
        .await
    }
    /// Thread id of the addressed message, if it exists in *this* account.
    pub async fn message_thread(&self, r: &MessageRef) -> Result<Option<String>> {
        let (aid, mid) = (r.account_id.clone(), r.message_id.clone());
        self.read(move |c| {
            let mut s = c.prepare("SELECT thread_id FROM messages WHERE account_id=? AND id=?")?;
            let mut rows = s.query_map(params![aid, mid], |r| r.get(0))?;
            let __out = rows.next().transpose()?;
            Ok(__out)
        })
        .await
    }
}
