use super::Db;
use crate::dto::Label;
use anyhow::Result;
use rusqlite::{params, OptionalExtension};

impl Db {
    pub async fn labels_upsert(&self, l: &Label) -> Result<()> {
        let l = l.clone();
        self.write(move |c| {
      c.execute("INSERT OR REPLACE INTO labels (account_id,id,name,kind,color_bg,color_fg,visible,unread_count,total_count,sort_order) VALUES (?,?,?,?,?,?,?,?,?,?)",
        params![l.account_id,l.id,l.name,l.kind,l.color_bg,l.color_fg,l.visible as i32,l.unread_count,l.total_count,l.sort_order])?;
      Ok(())
    }).await
    }
    pub async fn labels_list(&self, account_id: &str) -> Result<Vec<Label>> {
        let aid = account_id.to_string();
        self.read(move |c| {
            let mut s = c.prepare(
                "SELECT * FROM labels WHERE account_id=? ORDER BY kind DESC, sort_order, name",
            )?;
            let rows = s
                .query_map(params![aid], |r| {
                    Ok(Label {
                        account_id: r.get("account_id")?,
                        id: r.get("id")?,
                        name: r.get("name")?,
                        kind: r.get("kind")?,
                        color_bg: r.get("color_bg")?,
                        color_fg: r.get("color_fg")?,
                        visible: r.get::<_, i64>("visible")? != 0,
                        unread_count: r.get("unread_count")?,
                        total_count: r.get("total_count")?,
                        sort_order: r.get("sort_order")?,
                        ..Default::default()
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }
    /// The provider-addressing id of a label by its display name.
    ///
    /// Symbolic references in a queued operation (`name:Sift/Snoozed`) are
    /// resolved through this at claim time, so an id is never invented: for
    /// Gmail it is the REST label id, for IMAP the account's folder-derived
    /// id. A label that has not been created yet resolves to `None`, and the
    /// caller waits for the `create_label` operation it depends on.
    pub async fn label_id_by_name(&self, account_id: &str, name: &str) -> Result<Option<String>> {
        let (a, n) = (account_id.to_string(), name.to_string());
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT id FROM labels WHERE account_id=? AND name=? ORDER BY id LIMIT 1",
                params![a, n],
                |r| r.get::<_, String>(0),
            )
            .optional()?)
        })
        .await
    }

    /// Adopt the real provider id of a label that was created while offline.
    ///
    /// The placeholder row (a `sift-local:` id, never sent to a provider) is
    /// removed in the same write that stores the resolved label, and every
    /// local reference to it moves with it: message membership, the
    /// denormalized message flags and the thread aggregates. Without that
    /// rewrite a snoozed thread would keep a label id the provider has never
    /// heard of, and a wake could never remove it.
    pub async fn labels_adopt_created(&self, l: &Label) -> Result<()> {
        let l = l.clone();
        self.write(move |c| {
            let tx = c.unchecked_transaction()?;
            let placeholders: Vec<String> = tx
                .prepare(
                    "SELECT id FROM labels WHERE account_id=? AND name=? AND id LIKE 'sift-local:%'",
                )?
                .query_map(params![l.account_id, l.name], |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for placeholder in &placeholders {
                // The messages that carried the placeholder, before the rows
                // are rewritten.
                let messages: Vec<(String, String)> = tx
                    .prepare(
                        "SELECT m.id, m.thread_id FROM messages m \
                         JOIN message_labels ml ON ml.message_id=m.id AND ml.label_id=?1 \
                         WHERE m.account_id=?2",
                    )?
                    .query_map(params![placeholder, l.account_id], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                tx.execute(
                    "DELETE FROM labels WHERE account_id=? AND id=?",
                    params![l.account_id, placeholder],
                )?;
                tx.execute(
                    "UPDATE message_labels SET label_id=?1 WHERE label_id=?2 \
                     AND message_id IN (SELECT id FROM messages WHERE account_id=?3)",
                    params![l.id, placeholder, l.account_id],
                )?;
                for (message_id, thread_id) in messages {
                    let labels: Vec<String> = tx
                        .prepare(
                            "SELECT label_id FROM message_labels WHERE account_id=? AND message_id=? \
                             ORDER BY label_id",
                        )?
                        .query_map(params![l.account_id, message_id], |r| r.get(0))?
                        .collect::<Result<Vec<_>, _>>()?;
                    tx.execute(
                        "UPDATE messages SET label_ids=? WHERE account_id=? AND id=?",
                        params![
                            serde_json::to_string(&labels)?,
                            l.account_id,
                            message_id
                        ],
                    )?;
                    crate::db::threads::recompute_thread_conn(&tx, &l.account_id, &thread_id)?;
                }
            }
            tx.execute("INSERT OR REPLACE INTO labels (account_id,id,name,kind,color_bg,color_fg,visible,unread_count,total_count,sort_order) VALUES (?,?,?,?,?,?,?,?,?,?)",
                params![l.account_id,l.id,l.name,l.kind,l.color_bg,l.color_fg,l.visible as i32,l.unread_count,l.total_count,l.sort_order])?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Create-or-adopt a local label row under a placeholder id. Returns the
    /// id other rows should reference until the remote create completes.
    pub async fn labels_ensure_local(&self, account_id: &str, name: &str) -> Result<String> {
        if let Some(id) = self.label_id_by_name(account_id, name).await? {
            return Ok(id);
        }
        let placeholder = format!("sift-local:{account_id}:{name}");
        let label = Label {
            account_id: account_id.to_string(),
            id: placeholder.clone(),
            name: name.to_string(),
            kind: "user".into(),
            color_bg: None,
            color_fg: None,
            visible: true,
            unread_count: 0,
            total_count: 0,
            sort_order: 200,
            ..Default::default()
        };
        self.labels_upsert(&label).await?;
        Ok(placeholder)
    }

    pub async fn labels_set_counts(
        &self,
        account_id: &str,
        label_id: &str,
        unread: i64,
        total: i64,
    ) -> Result<()> {
        let (a, b) = (account_id.to_string(), label_id.to_string());
        self.write(move |c| {
            c.execute(
                "UPDATE labels SET unread_count=?, total_count=? WHERE account_id=? AND id=?",
                params![unread, total, a, b],
            )?;
            Ok(())
        })
        .await
    }
}
