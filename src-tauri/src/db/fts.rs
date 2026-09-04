use super::Db;
use anyhow::Result;
use rusqlite::params;

impl Db {
    #[allow(clippy::too_many_arguments)] // single indexed query needs all filters
    pub async fn fts_search_local(
        &self,
        account_ids: &[String],
        terms: &str,
        from: Option<&str>,
        to: Option<&str>,
        subject_only: Option<&str>,
        after: Option<i64>,
        before: Option<i64>,
        unread: Option<bool>,
        starred: Option<bool>,
        has_att: Option<bool>,
        limit: i64,
    ) -> Result<Vec<(String, String, i64)>> {
        let (aids, terms, from, to, subj) = (
            account_ids.to_vec(),
            terms.to_string(),
            from.map(|s| s.to_string()),
            to.map(|s| s.to_string()),
            subject_only.map(|s| s.to_string()),
        );
        self.read(move |c| {
      // Build FTS MATCH expression
      let mut m: Vec<String> = vec![];
      if !terms.trim().is_empty() {
        // prefix last token
        let toks: Vec<&str> = terms.split_whitespace().collect();
        let expr = toks.iter().enumerate().map(|(i,t)| {
          let clean = t.replace('"', "\"\"");
          if i == toks.len()-1 { format!("\"{clean}\"*") } else { format!("\"{clean}\"") }
        }).collect::<Vec<_>>().join(" ");
        m.push(format!("{{subject from_text to_text body}} : ({expr})"));
      }
      if let Some(f) = &from { m.push(format!("{{from_text}} : (\"{}\"*)", f.replace('"', "\"\""))); }
      if let Some(t) = &to { m.push(format!("{{to_text}} : (\"{}\"*)", t.replace('"', "\"\""))); }
      if let Some(s) = &subj { m.push(format!("{{subject}} : (\"{}\"*)", s.replace('"', "\"\""))); }
      let match_q = if m.is_empty() { None } else { Some(m.join(" AND ")) };
      let placeholders = aids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
      let mut sql = format!("SELECT f.message_id, f.account_id, m.internal_date FROM messages_fts f JOIN messages m ON m.id=f.message_id WHERE f.account_id IN ({placeholders})");
      if match_q.is_some() { sql += " AND messages_fts MATCH ?"; }
      if after.is_some() { sql += " AND m.internal_date >= ?"; }
      if before.is_some() { sql += " AND m.internal_date <= ?"; }
      if unread == Some(true) { sql += " AND m.is_unread=1"; }
      if starred == Some(true) { sql += " AND m.is_starred=1"; }
      if has_att == Some(true) { sql += " AND m.has_attachments=1"; }
      sql += " ORDER BY m.internal_date DESC LIMIT ?";
      let mut st = c.prepare(&sql)?;
      // bind: use raw binding for flexibility
      let mut idx = 1;
      for a in &aids { st.raw_bind_parameter(idx, a)?; idx += 1; }
      if let Some(mq) = &match_q { st.raw_bind_parameter(idx, mq)?; idx += 1; }
      if let Some(a) = after { st.raw_bind_parameter(idx, a)?; idx += 1; }
      if let Some(b) = before { st.raw_bind_parameter(idx, b)?; idx += 1; }
      st.raw_bind_parameter(idx, limit)?; idx += 1;
      let mut q = st.raw_query();
      let mut out = vec![];
      while let Some(r) = q.next()? { out.push((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?)); }
      let _ = idx;
      Ok(out)
    }).await
    }
    pub async fn fts_delete_message(&self, message_id: &str) -> Result<()> {
        let m = message_id.to_string();
        self.write(move |c| {
            c.execute("DELETE FROM messages_fts WHERE message_id=?", params![m])?;
            Ok(())
        })
        .await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::messages::MsgUpsert;
    #[tokio::test]
    async fn p3_t16_fts_index_and_delete() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let a = db.new_account("a@x.com", None, None).await.unwrap();
        db.messages_upsert(MsgUpsert {
            id: "m1".into(),
            account_id: a.id.clone(),
            thread_id: "t1".into(),
            internal_date: 1,
            subject: "Quarterly numbers".into(),
            snippet: "".into(),
            from_name: Some("Ada".into()),
            from_email: Some("ada@x.com".into()),
            label_ids: vec!["INBOX".into()],
            ..Default::default()
        })
        .await
        .unwrap();
        let r = db
            .fts_search_local(
                std::slice::from_ref(&a.id),
                "numbers",
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                10,
            )
            .await
            .unwrap();
        assert_eq!(r.len(), 1);
        db.messages_delete("m1", &a.id, "t1").await.unwrap();
        let r2 = db
            .fts_search_local(
                std::slice::from_ref(&a.id),
                "numbers",
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                10,
            )
            .await
            .unwrap();
        assert_eq!(r2.len(), 0);
    }
}
