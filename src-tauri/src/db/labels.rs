use super::Db;
use crate::dto::Label;
use anyhow::Result;
use rusqlite::params;

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
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
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
