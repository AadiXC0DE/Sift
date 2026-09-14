//! Shared fixture for the P7.3 benchmarks: a 100 000-message mailbox.
//!
//! Seeding is one transaction with prepared statements, because a Criterion
//! setup that took minutes would be a benchmark nobody runs.

use sift::db::Db;

pub const MESSAGES: i64 = 100_000;

pub struct Fixture {
    pub db: Db,
    pub accounts: Vec<String>,
    pub _dir: tempfile::TempDir,
}

/// Two accounts, 100 000 messages spread over 20 000 conversations, one in ten
/// messages unread and one in ten carrying an attachment, so the search
/// filters have something to select.
pub fn seed() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let accounts = rt.block_on(async {
        let first = db.new_account("one@example.com", None, None).await.unwrap();
        let second = db.new_account("two@example.com", None, None).await.unwrap();
        vec![first.id, second.id]
    });
    let accounts_for_seed = accounts.clone();
    rt.block_on(seed_rows(&db, &accounts_for_seed)).unwrap();
    Fixture {
        db,
        accounts,
        _dir: dir,
    }
}

async fn seed_rows(db: &Db, accounts: &[String]) -> anyhow::Result<()> {
    let accounts = accounts.to_vec();
    db.write(move |conn| {
        let tx = conn.unchecked_transaction()?;
        let mut message = tx.prepare(
            "INSERT INTO messages (id,account_id,thread_id,internal_date,from_name,from_email,
               to_json,cc_json,bcc_json,subject,snippet,has_attachments,is_unread,is_starred,
               is_draft,is_sent_by_me,label_ids,body_state)
             VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,0,0,'[]','fetched')",
        )?;
        let mut fts = tx.prepare(
            "INSERT INTO messages_fts (message_id,account_id,subject,from_text,to_text,body)
             VALUES (?,?,?,?,?,?)",
        )?;
        let mut label = tx.prepare(
            "INSERT OR IGNORE INTO message_labels (account_id,message_id,label_id) VALUES (?,?,?)",
        )?;
        let mut thread = tx.prepare(
            "INSERT INTO threads (account_id,id,subject,snippet,last_message_at,first_message_at,
               message_count,unread_count,is_starred,has_attachments,participants,label_ids,
               in_inbox,in_trash,in_spam,is_draft_only,snoozed_until)
             VALUES (?,?,?,?,?,?,?,?,0,?,?,?,1,0,0,0,?)
             ON CONFLICT(account_id,id) DO UPDATE SET
               last_message_at=MAX(threads.last_message_at, excluded.last_message_at),
               message_count=threads.message_count+1,
               unread_count=threads.unread_count+excluded.unread_count,
               has_attachments=MAX(threads.has_attachments, excluded.has_attachments)",
        )?;
        for index in 0..MESSAGES {
            let account = &accounts[(index % 2) as usize];
            let thread_id = format!("t-{:05}", index / 5);
            let message_id = format!("m-{index:06}");
            let at = 1_700_000_000_000 + index;
            let unread = index % 10 == 0;
            let attachment = index % 10 == 3;
            let subject = if index % 3 == 0 {
                "quarterly numbers from the client".to_string()
            } else {
                format!("message {index}")
            };
            let from = if index % 4 == 0 {
                ("Ada Lovelace", "ada@example.com")
            } else {
                ("Carol Danvers", "carol@example.com")
            };
            let body = "the quarterly numbers are attached for your review";
            message.execute(rusqlite::params![
                message_id,
                account,
                thread_id,
                at,
                from.0,
                from.1,
                "[{\"n\":null,\"e\":\"carol@example.com\",\"me\":null}]",
                "[]",
                "[]",
                subject,
                body,
                attachment as i32,
                unread as i32,
            ])?;
            fts.execute(rusqlite::params![
                message_id,
                account,
                subject,
                format!("{} {}", from.0, from.1),
                "[]",
                body,
            ])?;
            label.execute(rusqlite::params![account, message_id, "INBOX"])?;
            if unread {
                label.execute(rusqlite::params![account, message_id, "UNREAD"])?;
            }
            if index % 25 == 0 {
                label.execute(rusqlite::params![account, message_id, "Label_12345"])?;
            }
            // Every 500th thread is snoozed, so the snoozed keyset has rows.
            let snooze = if index % 500 == 0 {
                Some(1_800_000_000_000i64 + index)
            } else {
                None
            };
            thread.execute(rusqlite::params![
                account,
                thread_id,
                subject,
                body,
                at,
                at,
                attachment as i32,
                "[{\"n\":\"Ada\",\"e\":\"ada@example.com\",\"me\":null}]",
                "[\"INBOX\"]",
                snooze,
            ])?;
        }
        drop(message);
        drop(fts);
        drop(label);
        drop(thread);
        tx.commit()?;
        Ok(())
    })
    .await
}

/// Print the plan the optimizer actually chose, so a benchmark run records
/// what the numbers were measured against.
pub fn report_plan(runtime: &tokio::runtime::Runtime, db: &Db, label: &str, sql: &str) {
    let sql = sql.to_string();
    let plan = runtime
        .block_on(async {
            db.read(move |c| {
                let mut statement = c.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
                let rows: Vec<String> = statement
                    .query_map([], |r| r.get::<_, String>(3))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows.join("\n  "))
            })
            .await
        })
        .unwrap();
    eprintln!("EXPLAIN QUERY PLAN ({label}):\n  {plan}");
}
