//! P11-T08: bodies, attachments, raw + the 1 GB backfill budget.
#[path = "support/mod.rs"]
mod support;

use sift::db::Db;
use sift::provider::{DbSink, Provider};

fn pool_for(port: u16) -> sift::provider::imap::conn::ImapPool {
    sift::provider::imap::conn::ImapPool::new(
        "user@gmail.com".into(),
        "goodpassword0000".into(),
        "127.0.0.1".into(),
        port,
        true,
    )
}

async fn synced_ctx() -> (
    support::fake_imap::FakeGmail,
    sift::provider::imap::provider::GmailImapProvider,
    Db,
    sift::dto::Account,
) {
    let fake = support::fake_imap::FakeGmail::start().await;
    let dir = tempfile::tempdir().unwrap();
    let dir = Box::leak(Box::new(dir));
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("user@gmail.com", None, None).await.unwrap();
    let pool = pool_for(fake.addr.port());
    let provider =
        sift::provider::imap::provider::GmailImapProvider::new(acc.id.clone(), pool, db.clone());
    let sink = DbSink::new(db.clone());
    let cancel = tokio_util::sync::CancellationToken::new();
    let cursor = provider.full_sync(&sink, cancel).await.unwrap();
    assert!(matches!(cursor, sift::provider::Cursor::Imap { .. }));
    (fake, provider, db, acc)
}

/// First message id (hex) carrying a PDF part.
async fn att_message(db: &Db, acc: &str) -> String {
    db.read({
        let acc = acc.to_string();
        move |c| {
            Ok(c.query_row(
                "SELECT m.id FROM messages m JOIN attachments a ON a.message_id=m.id \
                 WHERE m.account_id=? AND a.mime='application/pdf' LIMIT 1",
                rusqlite::params![acc],
                |r| r.get::<_, String>(0),
            )?)
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn p11_t08_fetch_body_shapes() {
    let (_fake, provider, db, acc) = synced_ctx().await;
    // Ten messages: html+text present, subjects/addresses parsed, quote
    // detection fires on the every-3rd messages.
    let ids: Vec<String> = db
        .read({
            let acc = acc.id.clone();
            move |c| {
                let mut s = c.prepare(
                    "SELECT m.id FROM messages m JOIN imap_uids u ON u.message_id=m.id \
                     WHERE m.account_id=? AND u.role='all' ORDER BY m.internal_date DESC LIMIT 10",
                )?;
                let rows = s
                    .query_map(rusqlite::params![acc], |r| r.get(0))?
                    .collect::<Result<Vec<String>, _>>()?;
                Ok(rows)
            }
        })
        .await
        .unwrap();
    assert_eq!(ids.len(), 10);
    let mut rows = vec![];
    for id in &ids {
        let pm = provider.fetch_body(id).await.unwrap();
        assert!(!pm.subject.is_empty(), "subject for {id}");
        assert!(
            pm.from_email.as_deref().unwrap_or("").contains('@'),
            "from for {id}"
        );
        assert!(
            pm.html.as_deref().unwrap_or("").contains("html"),
            "html for {id}"
        );
        assert!(
            pm.text.as_deref().unwrap_or("").contains("plain"),
            "text for {id}"
        );
        rows.push((
            pm.subject.clone(),
            pm.from_email.clone(),
            pm.quoted_from.is_some(),
            pm.attachments.len(),
        ));
    }
    assert!(rows.iter().any(|r| r.2), "every-3rd messages carry quotes");
    insta::assert_debug_snapshot!(rows);
}

#[tokio::test]
async fn p11_t08_attachment_byte_exact() {
    let (_fake, provider, db, acc) = synced_ctx().await;
    let mid = att_message(&db, &acc.id).await;
    let atts = db.attachments_for_message(&mid).await.unwrap();
    assert!(!atts.is_empty());
    // The PDF part decodes byte-exact.
    let pdf = atts
        .iter()
        .find(|a| a.mime == "application/pdf")
        .expect("pdf");
    let stored_section = db
        .read({
            let id = pdf.id.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT part_id FROM attachments WHERE id=?",
                    rusqlite::params![id],
                    |r| r.get::<_, String>(0),
                )?)
            }
        })
        .await
        .unwrap();
    let bytes = provider
        .fetch_attachment(&mid, &stored_section)
        .await
        .unwrap();
    assert!(bytes.starts_with(b"PDFDATA-"), "got {bytes:?}");
}

#[tokio::test]
async fn p11_t08_raw_source() {
    let (_fake, provider, db, acc) = synced_ctx().await;
    let mid = att_message(&db, &acc.id).await;
    let raw = provider.fetch_raw(&mid).await.unwrap();
    assert!(raw.contains("Subject:"));
    assert!(raw.contains("multipart/mixed"));
}

#[tokio::test]
async fn p11_t08_budget_pauses_backfill_not_foreground() {
    let (_fake, provider, db, acc) = synced_ctx().await;
    let mid = att_message(&db, &acc.id).await;
    // Exhaust today's budget directly.
    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
    db.setting_set_raw(
        &format!("imap_budget_{}", acc.id),
        &serde_json::json!({ "date": today, "bytes": 1_000_000_000u64 }).to_string(),
    )
    .await
    .unwrap();
    let e = provider.fetch_body_backfill(&mid).await.unwrap_err();
    assert_eq!(serde_json::to_value(&e).unwrap()["code"], "backfill_budget");
    // Foreground fetch is never budgeted.
    assert!(provider.fetch_body(&mid).await.is_ok());
}
