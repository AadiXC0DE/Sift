use super::Db;
use crate::dto::{Address, ThreadRow, ThreadsPage, ThreadsQuery, View};
use crate::errors::SiftError;
use crate::search::cursor::{self, Cursor, SortKind};
use anyhow::Result;
use rusqlite::params;

/// One `threads` row as the UI sees it. Shared by the list query, the search
/// hydration and the single-row lookup so all three can never disagree.
pub(crate) fn thread_row_from(r: &rusqlite::Row<'_>) -> rusqlite::Result<ThreadRow> {
    let label_ids: Vec<String> = serde_json::from_str(&r.get::<_, String>("label_ids")?).unwrap_or_default();
    let participants: Vec<Address> =
        serde_json::from_str(&r.get::<_, String>("participants")?).unwrap_or_default();
    Ok(ThreadRow {
        account_id: r.get("account_id")?,
        id: r.get("id")?,
        subject: r.get("subject")?,
        snippet: r.get("snippet")?,
        participants,
        last_message_at: r.get("last_message_at")?,
        message_count: r.get("message_count")?,
        unread_count: r.get("unread_count")?,
        is_starred: r.get::<_, i64>("is_starred")? != 0,
        has_attachments: r.get::<_, i64>("has_attachments")? != 0,
        label_ids,
        snoozed_until: r.get("snoozed_until")?,
        // Absent from queries that do not join it: a reminder is an
        // indicator, never a column a caller must supply.
        reminder_at: r.get("reminder_at").unwrap_or(None),
        server_only: false,
    })
}

/// Recompute one thread's aggregate row on an existing connection/transaction,
/// so a partial-sync batch can write its messages and their thread rows in one
/// commit (P4.5).
pub(crate) fn recompute_thread_conn(c: &rusqlite::Connection, a: &str, t: &str) -> Result<()> {
      // 8 aggregate columns in one row scan; tuple keeps it in one query (perf hot path).
      #[allow(clippy::type_complexity)]
      let (mc, uc, starred, has_att, min_d, max_d, subj, snip): (i64,i64,i64,i64,Option<i64>,Option<i64>,Option<String>,Option<String>) =
        c.query_row("SELECT count(*), COALESCE(SUM(is_unread),0), COALESCE(MAX(is_starred),0), COALESCE(MAX(has_attachments),0), MIN(internal_date), MAX(internal_date), NULL, NULL FROM messages WHERE account_id=? AND thread_id=?", params![a,t], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,None,None)))?;
      if mc == 0 {
        c.execute("DELETE FROM threads WHERE account_id=? AND id=?", params![a,t])?;
        return Ok(());
      }
      let in_inbox: i64 = c.query_row("SELECT EXISTS(SELECT 1 FROM messages m JOIN message_labels ml ON ml.account_id=m.account_id AND ml.message_id=m.id WHERE m.account_id=? AND m.thread_id=? AND ml.label_id='INBOX')", params![a,t], |r| r.get(0))?;
      let all_trash: i64 = c.query_row("SELECT CASE WHEN EXISTS(SELECT 1 FROM messages m LEFT JOIN message_labels ml ON ml.account_id=m.account_id AND ml.message_id=m.id AND ml.label_id='TRASH' WHERE m.account_id=? AND m.thread_id=? AND ml.label_id IS NULL) THEN 0 ELSE 1 END", params![a,t], |r| r.get(0))?;
      let all_spam: i64 = c.query_row("SELECT CASE WHEN EXISTS(SELECT 1 FROM messages m LEFT JOIN message_labels ml ON ml.account_id=m.account_id AND ml.message_id=m.id AND ml.label_id='SPAM' WHERE m.account_id=? AND m.thread_id=? AND ml.label_id IS NULL) THEN 0 ELSE 1 END", params![a,t], |r| r.get(0))?;
      let subject: String = c.query_row("SELECT subject FROM messages WHERE account_id=? AND thread_id=? AND subject<>'' ORDER BY internal_date LIMIT 1", params![a,t], |r| r.get(0)).unwrap_or_default();
      let snippet: String = c.query_row("SELECT snippet FROM messages WHERE account_id=? AND thread_id=? AND is_draft=0 ORDER BY internal_date DESC LIMIT 1", params![a,t], |r| r.get(0)).unwrap_or_default();
      // participants: distinct from_email in date order
      let mut ps = c.prepare("SELECT from_name, from_email FROM messages WHERE account_id=? AND thread_id=? ORDER BY internal_date")?;
      let mut parts: Vec<Address> = vec![];
      let mut seen = std::collections::HashSet::new();
      for row in ps.query_map(params![a,t], |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)))? {
        let (n, e) = row?;
        if let Some(e) = e {
          if seen.insert(e.clone()) { parts.push(Address { n, e, me: None }); if parts.len() >= 6 { break; } }
        }
      }
      let parts_json = serde_json::to_string(&parts)?;
      // union labels
      let mut ls = c.prepare("SELECT DISTINCT label_id FROM message_labels WHERE account_id=? AND message_id IN (SELECT id FROM messages WHERE account_id=? AND thread_id=?)")?;
      let labels: Vec<String> = ls.query_map(params![a,a,t], |r| r.get(0))?.collect::<Result<Vec<_>,_>>()?;
      let labels_json = serde_json::to_string(&labels)?;
      let is_draft_only: i64 = c.query_row("SELECT CASE WHEN EXISTS(SELECT 1 FROM messages WHERE account_id=? AND thread_id=? AND is_draft=0) THEN 0 ELSE 1 END", params![a,t], |r| r.get(0))?;
      let _ = (subj, snip);
      c.execute("INSERT INTO threads (account_id,id,subject,snippet,last_message_at,first_message_at,message_count,unread_count,is_starred,has_attachments,participants,label_ids,in_inbox,in_trash,in_spam,is_draft_only) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)
        ON CONFLICT(account_id,id) DO UPDATE SET subject=excluded.subject, snippet=excluded.snippet, last_message_at=excluded.last_message_at, first_message_at=excluded.first_message_at, message_count=excluded.message_count, unread_count=excluded.unread_count, is_starred=excluded.is_starred, has_attachments=excluded.has_attachments, participants=excluded.participants, label_ids=excluded.label_ids, in_inbox=excluded.in_inbox, in_trash=excluded.in_trash, in_spam=excluded.in_spam, is_draft_only=excluded.is_draft_only",
        params![a,t,subject,snippet,max_d.unwrap_or(0),min_d.unwrap_or(0),mc,uc,starred,has_att,parts_json,labels_json,in_inbox,all_trash,all_spam,is_draft_only])?;
      Ok(())
}

/// Stable tag for one view, used in the cursor scope fingerprint.
fn view_tag(view: &View) -> String {
    match view {
        View::Inbox => "inbox".into(),
        View::Starred => "starred".into(),
        View::Snoozed => "snoozed".into(),
        View::Sent => "sent".into(),
        View::Drafts => "drafts".into(),
        View::Archive => "archive".into(),
        View::Spam => "spam".into(),
        View::Trash => "trash".into(),
        View::AllMail => "all_mail".into(),
        View::Label { label_id } => format!("label:{label_id}"),
        View::Search { .. } => "search".into(),
    }
}

/// `(sort kind, scope fingerprint, query fingerprint)` for a list request.
/// The cursor is only valid for exactly this triple.
pub fn page_identity(view: &View, account_ids: &[String]) -> (SortKind, String, String) {
    match view {
        View::Snoozed => (
            SortKind::Snoozed,
            cursor::scope_key(&view_tag(view), account_ids),
            String::new(),
        ),
        View::Search { q } => (
            SortKind::Search,
            cursor::scope_key(&view_tag(view), account_ids),
            crate::search::query::parse(q).fingerprint(),
        ),
        other => (
            SortKind::Default,
            cursor::scope_key(&view_tag(other), account_ids),
            String::new(),
        ),
    }
}

impl Db {
    pub async fn recompute_thread(&self, account_id: &str, thread_id: &str) -> Result<()> {
        let (a, t) = (account_id.to_string(), thread_id.to_string());
        self.write(move |c| recompute_thread_conn(c, &a, &t)).await
    }

    fn view_where(view: &View) -> (&'static str, Vec<String>) {
        match view {
      View::Inbox => ("t.in_inbox=1 AND t.in_trash=0 AND t.in_spam=0 AND t.snoozed_until IS NULL", vec![]),
      View::Starred => ("t.is_starred=1 AND t.in_trash=0 AND t.in_spam=0", vec![]),
      View::Snoozed => ("t.snoozed_until IS NOT NULL", vec![]),
      View::Sent => ("EXISTS (SELECT 1 FROM messages m JOIN message_labels ml ON ml.account_id=m.account_id AND ml.message_id=m.id WHERE m.thread_id=t.id AND m.account_id=t.account_id AND ml.label_id='SENT')", vec![]),
      View::Drafts => ("EXISTS (SELECT 1 FROM messages m JOIN message_labels ml ON ml.account_id=m.account_id AND ml.message_id=m.id WHERE m.thread_id=t.id AND m.account_id=t.account_id AND ml.label_id='DRAFT')", vec![]),
      View::Archive => ("t.in_inbox=0 AND t.in_trash=0 AND t.in_spam=0 AND t.is_draft_only=0", vec![]),
      // P3.6: All Mail is message-level membership - a thread appears when at
      // least one of its messages carries neither TRASH nor SPAM. It is NOT
      // the unified account scope: `accountIds` still filters rows.
      View::AllMail => ("EXISTS (SELECT 1 FROM messages m WHERE m.account_id=t.account_id AND m.thread_id=t.id AND NOT EXISTS (SELECT 1 FROM message_labels ml WHERE ml.account_id=m.account_id AND ml.message_id=m.id AND ml.label_id IN ('TRASH','SPAM')))", vec![]),
      View::Spam => ("t.in_spam=1", vec![]),
      View::Trash => ("t.in_trash=1", vec![]),
      View::Label { label_id } => ("EXISTS (SELECT 1 FROM json_each(t.label_ids) WHERE value=?) AND t.in_trash=0 AND t.in_spam=0", vec![label_id.clone()]),
      // The Search view is served by the compiled candidate query; this arm
      // exists only so the mailbox filter stays exhaustive.
      View::Search { .. } => ("t.in_trash=0 AND t.in_spam=0", vec![]),
    }
    }

    /// One page of a thread list or of a local search (P7.3).
    ///
    /// The cursor is decoded and matched against this exact request
    /// (sort, scope, query) before any row is read; a cursor from another list
    /// is a typed error, never a silently different page.
    pub async fn threads_query(&self, q: ThreadsQuery) -> std::result::Result<ThreadsPage, SiftError> {
        let limit = q.limit.clamp(1, 100);
        let (sort, scope, query) = page_identity(&q.view, &q.account_ids);
        let cursor = match q.cursor.as_deref() {
            Some(raw) => Some(cursor::expect(raw, sort, &scope, &query)?),
            None => None,
        };
        if let View::Search { q: text } = &q.view {
            let parsed = crate::search::query::parse(text);
            let page = crate::search::local::search_page(
                self,
                &q.account_ids,
                &parsed,
                cursor.as_ref(),
                &scope,
                limit,
            )
            .await
            .map_err(internal)?;
            return Ok(ThreadsPage {
                rows: page.rows,
                next_cursor: page.next_cursor,
                total: None,
                generation: 0,
            });
        }
        if q.account_ids.is_empty() {
            return Ok(ThreadsPage {
                rows: vec![],
                next_cursor: None,
                total: None,
                generation: 0,
            });
        }
        let (where_c, wparams) = Self::view_where(&q.view);
        // The keyset predicate compares exactly the ORDER BY tuple.
        let (order, keyset) = match sort {
            SortKind::Snoozed => (
                "t.snoozed_until ASC, t.account_id ASC, t.id ASC",
                "(t.snoozed_until, t.account_id, t.id) > (?,?,?)",
            ),
            _ => (
                "t.last_message_at DESC, t.account_id DESC, t.id DESC",
                "(t.last_message_at, t.account_id, t.id) < (?,?,?)",
            ),
        };
        let placeholders = q
            .account_ids
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let extra_unread = if q.unread_only {
            " AND t.unread_count>0"
        } else {
            ""
        };
        let extra_att = if q.has_attachment {
            " AND t.has_attachments=1"
        } else {
            ""
        };
        let keyset_clause = if cursor.is_some() {
            format!(" AND {keyset}")
        } else {
            String::new()
        };
        let sql = format!(
            "SELECT t.*, (SELECT r.remind_at FROM reminders r WHERE r.account_id=t.account_id \
                AND r.thread_id=t.id AND r.completed_at IS NULL) AS reminder_at \
         FROM threads t WHERE t.account_id IN ({placeholders}) AND {where_c}{extra_unread}{extra_att}{keyset_clause} ORDER BY {order} LIMIT ?"
        );
        let account_ids = q.account_ids.clone();
        let key = cursor.map(|c| c.key);
        self.read(move |c| {
            let mut stmt = c.prepare(&sql)?;
            let mut idx = 1usize;
            for a in &account_ids {
                stmt.raw_bind_parameter(idx, a)?;
                idx += 1;
            }
            for w in &wparams {
                stmt.raw_bind_parameter(idx, w)?;
                idx += 1;
            }
            if let Some((date, account, id)) = &key {
                stmt.raw_bind_parameter(idx, *date)?;
                idx += 1;
                stmt.raw_bind_parameter(idx, account)?;
                idx += 1;
                stmt.raw_bind_parameter(idx, id)?;
                idx += 1;
            }
            // One row past the page: only its existence makes nextCursor real.
            stmt.raw_bind_parameter(idx, limit + 1)?;
            let mut rows_iter = stmt.raw_query();
            let mut rows: Vec<ThreadRow> = vec![];
            while let Some(r) = rows_iter.next()? {
                rows.push(thread_row_from(r)?);
            }
            let has_more = rows.len() as i64 > limit;
            if has_more {
                rows.pop();
            }
            let next_cursor = if has_more {
                rows.last().map(|r| {
                    let date = if sort == SortKind::Snoozed {
                        r.snoozed_until.unwrap_or(0)
                    } else {
                        r.last_message_at
                    };
                    cursor::encode(&Cursor {
                        sort,
                        scope: scope.clone(),
                        query: query.clone(),
                        key: (date, r.account_id.clone(), r.id.clone()),
                    })
                })
            } else {
                None
            };
            Ok(ThreadsPage {
                rows,
                next_cursor,
                total: None,
                generation: 0,
            })
        })
        .await
        .map_err(internal)
    }
}

fn internal(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    #[tokio::test]
    async fn p3_t09_recompute() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let a = db.new_account("a@x.com", None, None).await.unwrap();
        db.write({
      let aid = a.id.clone();
      move |c| {
        c.execute("INSERT INTO messages (id,account_id,thread_id,internal_date,subject,snippet,is_unread,is_starred,label_ids) VALUES ('m1',?,'t1',1,'Hello','s',1,0,'[]')", params![aid])?;
        c.execute("INSERT INTO messages (id,account_id,thread_id,internal_date,subject,snippet,is_unread,is_starred,label_ids) VALUES ('m2',?,'t1',2,'Re: Hello','s',0,0,'[]')", params![aid])?;
        c.execute("INSERT INTO message_labels (account_id,message_id,label_id) VALUES (?1,'m1','INBOX'),(?1,'m1','UNREAD'),(?1,'m2','SENT')", params![aid])?;
        Ok(())
      }
    }).await.unwrap();
        db.recompute_thread(&a.id, "t1").await.unwrap();
        let (in_inbox, unread, mc): (i64,i64,i64) = db.read({
      let aid = a.id.clone();
      move |c| Ok(c.query_row("SELECT in_inbox, unread_count, message_count FROM threads WHERE account_id=? AND id='t1'", params![aid], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?)
    }).await.unwrap();
        assert_eq!((in_inbox, unread, mc), (1, 1, 2));
    }
}
