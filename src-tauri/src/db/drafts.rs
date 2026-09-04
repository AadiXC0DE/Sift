use super::Db;
use crate::dto::Draft;
use anyhow::Result;
use rusqlite::params;

impl Db {
    pub async fn drafts_upsert(&self, d: &Draft) -> Result<Draft> {
        let mut d = d.clone();
        if d.local_id.is_none() {
            d.local_id = Some(uuid::Uuid::now_v7().to_string());
        }
        d.updated_at = Some(super::now_ms());
        let dd = d.clone();
        self.write(move |c| {
      c.execute("INSERT INTO drafts (local_id,account_id,remote_draft_id,remote_message_id,thread_id,in_reply_to_message_id,mode,to_json,cc_json,bcc_json,subject,body_html,attachments_json,updated_at,dirty) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,1)
        ON CONFLICT(local_id) DO UPDATE SET remote_draft_id=excluded.remote_draft_id, thread_id=excluded.thread_id, mode=excluded.mode, to_json=excluded.to_json, cc_json=excluded.cc_json, bcc_json=excluded.bcc_json, subject=excluded.subject, body_html=excluded.body_html, attachments_json=excluded.attachments_json, updated_at=excluded.updated_at, dirty=1",
        params![dd.local_id,dd.account_id,dd.remote_draft_id,dd.remote_message_id,dd.thread_id,dd.in_reply_to_message_id,dd.mode,serde_json::to_string(&dd.to_json)?,serde_json::to_string(&dd.cc_json)?,serde_json::to_string(&dd.bcc_json)?,dd.subject,dd.body_html,serde_json::to_string(&dd.attachments_json)?,dd.updated_at])?;
      Ok(())
    }).await?;
        Ok(d)
    }
    pub async fn drafts_get(&self, local_id: &str) -> Result<Option<Draft>> {
        let id = local_id.to_string();
        self.read(move |c| {
            let mut s = c.prepare("SELECT * FROM drafts WHERE local_id=?")?;
            let mut rows = s.query_map(params![id], |r| {
                Ok(Draft {
                    local_id: r.get("local_id")?,
                    account_id: r.get("account_id")?,
                    remote_draft_id: r.get("remote_draft_id")?,
                    remote_message_id: r.get("remote_message_id")?,
                    thread_id: r.get("thread_id")?,
                    in_reply_to_message_id: r.get("in_reply_to_message_id")?,
                    mode: r.get("mode")?,
                    to_json: serde_json::from_str(&r.get::<_, String>("to_json")?)
                        .unwrap_or_default(),
                    cc_json: serde_json::from_str(&r.get::<_, String>("cc_json")?)
                        .unwrap_or_default(),
                    bcc_json: serde_json::from_str(&r.get::<_, String>("bcc_json")?)
                        .unwrap_or_default(),
                    subject: r.get("subject")?,
                    body_html: r.get("body_html")?,
                    attachments_json: serde_json::from_str(
                        &r.get::<_, String>("attachments_json")?,
                    )
                    .unwrap_or_default(),
                    updated_at: r.get("updated_at")?,
                })
            })?;
            let __out = rows.next().transpose()?;
            Ok(__out)
        })
        .await
    }
    pub async fn drafts_delete(&self, local_id: &str) -> Result<()> {
        let id = local_id.to_string();
        self.write(move |c| {
            c.execute("DELETE FROM drafts WHERE local_id=?", params![id])?;
            Ok(())
        })
        .await
    }
    pub async fn drafts_list(&self, account_id: &str) -> Result<Vec<Draft>> {
        let a = account_id.to_string();
        self.read(move |c| {
            let mut s =
                c.prepare("SELECT * FROM drafts WHERE account_id=? ORDER BY updated_at DESC")?;
            {
                let __v = s
                    .query_map(params![a], |r| {
                        Ok(Draft {
                            local_id: r.get("local_id")?,
                            account_id: r.get("account_id")?,
                            remote_draft_id: r.get("remote_draft_id")?,
                            remote_message_id: r.get("remote_message_id")?,
                            thread_id: r.get("thread_id")?,
                            in_reply_to_message_id: r.get("in_reply_to_message_id")?,
                            mode: r.get("mode")?,
                            to_json: serde_json::from_str(&r.get::<_, String>("to_json")?)
                                .unwrap_or_default(),
                            cc_json: serde_json::from_str(&r.get::<_, String>("cc_json")?)
                                .unwrap_or_default(),
                            bcc_json: serde_json::from_str(&r.get::<_, String>("bcc_json")?)
                                .unwrap_or_default(),
                            subject: r.get("subject")?,
                            body_html: r.get("body_html")?,
                            attachments_json: serde_json::from_str(
                                &r.get::<_, String>("attachments_json")?,
                            )
                            .unwrap_or_default(),
                            updated_at: r.get("updated_at")?,
                        })
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(__v)
            }
        })
        .await
    }
}
