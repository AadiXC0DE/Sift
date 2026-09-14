//! DB-02 / P4.2 acceptance: account-qualified identity end to end.
//!
//! Two accounts may legitimately hold the *same* provider message, thread and
//! attachment ids. Reads, search, labels, bodies, attachments and deletion
//! must never cross between them, and an upgrade from a v6 database must carry
//! every body, byte and FTS row across without loss.

use rusqlite::params;
use sift::db::attachments::AttPut;
use sift::db::bodies::BodyPut;
use sift::db::messages::MsgUpsert;
use sift::db::Db;
use sift::dto::MessageRef;

async fn seed_message(db: &Db, aid: &str, id: &str, thread: &str, subject: &str, labels: &[&str]) {
    db.messages_upsert(MsgUpsert {
        id: id.into(),
        account_id: aid.into(),
        thread_id: thread.into(),
        internal_date: 1,
        subject: subject.into(),
        from_name: Some("Ada".into()),
        from_email: Some("ada@x.com".into()),
        label_ids: labels.iter().map(|s| s.to_string()).collect(),
        ..Default::default()
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn identical_provider_ids_stay_isolated_per_account() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let a = db.new_account("a@x.com", None, None).await.unwrap().id;
    let b = db.new_account("b@x.com", None, None).await.unwrap().id;

    // Same provider ids on purpose; only the account differs.
    seed_message(&db, &a, "hex1", "t1", "alpha subject", &["INBOX", "UNREAD"]).await;
    seed_message(&db, &b, "hex1", "t1", "beta subject", &["INBOX"]).await;

    let ma = MessageRef::new(a.clone(), "hex1");
    let mb = MessageRef::new(b.clone(), "hex1");

    db.bodies_put(BodyPut {
        account_id: a.clone(),
        message_id: "hex1".into(),
        html: Some("<p>alpha body</p>".into()),
        text: Some("alpha body".into()),
        remote_images: 0,
        trackers: 0,
        dark_safe: true,
        quoted_from: None,
        unsubscribe: Default::default(),
    })
    .await
    .unwrap();
    db.bodies_put(BodyPut {
        account_id: b.clone(),
        message_id: "hex1".into(),
        html: Some("<p>beta body</p>".into()),
        text: Some("beta body".into()),
        remote_images: 0,
        trackers: 0,
        dark_safe: true,
        quoted_from: None,
        unsubscribe: Default::default(),
    })
    .await
    .unwrap();

    db.attachments_put(AttPut {
        id: "att-2".into(),
        account_id: a.clone(),
        message_id: "hex1".into(),
        gmail_att_id: None,
        part_id: "2".into(),
        filename: Some("alpha.pdf".into()),
        mime: "application/pdf".into(),
        size: 4,
        content_id: None,
        is_inline: false,
        data: Some(b"AAAA".to_vec()),
    })
    .await
    .unwrap();
    db.attachments_put(AttPut {
        id: "att-2".into(),
        account_id: b.clone(),
        message_id: "hex1".into(),
        gmail_att_id: None,
        part_id: "2".into(),
        filename: Some("beta.pdf".into()),
        mime: "application/pdf".into(),
        size: 6,
        content_id: None,
        is_inline: false,
        data: Some(b"BBBBBB".to_vec()),
    })
    .await
    .unwrap();

    // Read isolation.
    let (_, ta, _, _, _, _) = db.bodies_get(&ma).await.unwrap().unwrap();
    let (_, tb, _, _, _, _) = db.bodies_get(&mb).await.unwrap().unwrap();
    assert_eq!(ta.as_deref(), Some("alpha body"));
    assert_eq!(tb.as_deref(), Some("beta body"));

    let ra = db.attachments_records(&ma).await.unwrap();
    let rb = db.attachments_records(&mb).await.unwrap();
    assert_eq!(ra[0].filename.as_deref(), Some("alpha.pdf"));
    assert_eq!(ra[0].data.as_deref(), Some(&b"AAAA"[..]));
    assert_eq!(rb[0].filename.as_deref(), Some("beta.pdf"));
    assert_eq!(rb[0].data.as_deref(), Some(&b"BBBBBB"[..]));

    // Search isolation: the term exists only in account A's body.
    let hits = db
        .fts_search_local(std::slice::from_ref(&a), "alpha", None, None, None, None, None, None, None, None, 10)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].1, a);
    let none = db
        .fts_search_local(std::slice::from_ref(&b), "alpha", None, None, None, None, None, None, None, None, 10)
        .await
        .unwrap();
    assert!(none.is_empty(), "account B must not see account A's body");

    // Label isolation.
    db.apply_label_change(&ma, &[], &["INBOX".to_string()])
        .await
        .unwrap();
    let (a_unread, a_starred) = db.message_flags(&ma).await.unwrap();
    assert!(a_unread, "A keeps its own labels");
    assert!(!a_starred);
    let b_labels: Vec<String> = db
        .read({
            let (aid, mid) = (b.clone(), "hex1".to_string());
            move |c| {
                let labels = c
                    .prepare(
                        "SELECT label_id FROM message_labels WHERE account_id=? AND message_id=? ORDER BY label_id",
                    )?
                    .query_map(params![aid, mid], |r| r.get::<_, String>(0))?
                    .collect::<Result<Vec<String>, _>>()?;
                Ok(labels)
            }
        })
        .await
        .unwrap();
    assert_eq!(b_labels, vec!["INBOX".to_string()]);

    // Delete isolation.
    db.messages_delete(&ma, "t1").await.unwrap();
    assert!(db.message_thread(&ma).await.unwrap().is_none());
    assert_eq!(db.message_thread(&mb).await.unwrap().as_deref(), Some("t1"));
    assert_eq!(db.attachments_records(&mb).await.unwrap().len(), 1);

    let violations: i64 = db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(violations, 0);
}

/// Account removal clears every table that can hold account-scoped rows, and
/// leaves a sibling account untouched.
#[tokio::test]
async fn removal_cleans_every_scoped_table() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let a = db.new_account("a@x.com", None, None).await.unwrap().id;
    let b = db.new_account("b@x.com", None, None).await.unwrap().id;

    for (aid, tag) in [(&a, "a"), (&b, "b")] {
        seed_message(&db, aid, &format!("m-{tag}"), &format!("t-{tag}"), "s", &["INBOX"]).await;
        db.bodies_put(BodyPut {
            account_id: aid.clone(),
            message_id: format!("m-{tag}"),
            html: Some("<p>x</p>".into()),
            text: Some(format!("body {tag}")),
            remote_images: 0,
            trackers: 0,
            dark_safe: true,
            quoted_from: None,
            unsubscribe: Default::default(),
        })
        .await
        .unwrap();
        db.attachments_put(AttPut {
            id: format!("att-{tag}"),
            account_id: aid.clone(),
            message_id: format!("m-{tag}"),
            gmail_att_id: None,
            part_id: "1".into(),
            filename: Some(format!("{tag}.pdf")),
            mime: "application/pdf".into(),
            size: 1,
            content_id: None,
            is_inline: false,
            data: Some(vec![1]),
        })
        .await
        .unwrap();
        db.contacts_upsert(aid, &format!("{tag}@x.com"), Some("P"))
            .await
            .unwrap();
        db.drafts_upsert(
            &sift::dto::Draft {
                local_id: format!("d-{tag}"),
                account_id: aid.clone(),
                subject: "d".into(),
                body_html: "d".into(),
                ..Default::default()
            },
            None,
        )
        .await
        .unwrap();
        db.outbox_enqueue(aid, "modify_labels", "{}", None, 0)
            .await
            .unwrap();
        let (aid2, tid) = (aid.clone(), format!("t-{tag}"));
        db.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO snoozes (account_id,thread_id,wake_at) VALUES (?,?,1)",
                params![aid2, tid],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let aid3 = aid.clone();
        db.write(move |c| {
            c.execute(
                "INSERT OR REPLACE INTO sender_prefs (account_id,email,allow_remote_images) VALUES (?,'x@y.z',1)",
                params![aid3],
            )?;
            Ok(())
        })
        .await
        .unwrap();
    }

    db.accounts_remove(&a).await.unwrap();

    let scoped = [
        "messages",
        "message_labels",
        "bodies",
        "attachments",
        "labels",
        "threads",
        "imap_folders",
        "imap_uids",
        "drafts",
        "outbox_ops",
        "snoozes",
        "sender_prefs",
        "contacts",
        "sync_log",
    ];
    for table in scoped {
        let left: i64 = db
            .read({
                let a = a.clone();
                move |c| {
                    Ok(c.query_row(
                        &format!("SELECT count(*) FROM {table} WHERE account_id=?"),
                        params![a],
                        |r| r.get(0),
                    )?)
                }
            })
            .await
            .unwrap();
        assert_eq!(left, 0, "{table} still has rows for the removed account");
    }
    let fts_left: i64 = db
        .read({
            let a = a.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM messages_fts WHERE account_id=?",
                    params![a],
                    |r| r.get(0),
                )?)
            }
        })
        .await
        .unwrap();
    assert_eq!(fts_left, 0);

    // The sibling account is intact.
    assert_eq!(
        db.drafts_list(std::slice::from_ref(&b), None, 10).await.unwrap().drafts.len(),
        1
    );
    assert_eq!(
        db.attachments_records(&MessageRef::new(b.clone(), "m-b"))
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(db.outbox_pending_count(&b).await.unwrap(), 1);
    let violations: i64 = db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(violations, 0);
}

fn database_bytes(text: &str) -> Vec<u8> {
    zstd::encode_all(text.as_bytes(), 3).unwrap()
}

/// Build a v6 database (0001..0006) with cached bodies, an attachment, label
/// rows, and pre-existing orphans, then upgrade through the real runner.
#[tokio::test]
async fn v6_upgrade_preserves_bodies_bytes_fts_and_reports_orphans() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sift.db");
    let body_text = "cached body bytes survive the scoping rebuild";
    let attachment_bytes = b"%PDF-1.4 exact payload".to_vec();
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        for sql in [
            include_str!("../src/db/migrations/0001_init.sql"),
            include_str!("../src/db/migrations/0002_contacts_backfill.sql"),
            include_str!("../src/db/migrations/0003_imap.sql"),
            include_str!("../src/db/migrations/0004_mail_rendering.sql"),
            include_str!("../src/db/migrations/0005_remote_images_default.sql"),
            include_str!("../src/db/migrations/0006_rerender_bodies.sql"),
        ] {
            conn.execute_batch(sql).unwrap();
        }
        conn.execute("UPDATE schema_version SET version=6", []).unwrap();
        conn.execute_batch(
            "INSERT INTO accounts (id,email,created_at) VALUES ('acc','a@x.com',1);
             INSERT INTO messages (id,account_id,thread_id,internal_date,subject,body_state)
               VALUES ('hex1','acc','t1',1,'hello','fetched');
             INSERT INTO message_labels (message_id,label_id) VALUES ('hex1','INBOX');",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO bodies (message_id,text_z,remote_image_count) VALUES ('hex1',?,2)",
            params![database_bytes(body_text)],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO attachments (id,message_id,part_id,filename,mime,size,data_z)
             VALUES ('att-2','hex1','2','invoice.pdf','application/pdf',?,?)",
            params![attachment_bytes.len() as i64, database_bytes("%PDF-1.4 exact payload")],
        )
        .unwrap();
        // Pre-existing orphans (deletes that ran without foreign keys on).
        conn.execute_batch("PRAGMA foreign_keys=OFF").unwrap();
        conn.execute(
            "INSERT INTO message_labels (message_id,label_id) VALUES ('ghost','INBOX')",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO bodies (message_id,text_z) VALUES ('ghost',?)",
            params![database_bytes("orphan body")],
        )
        .unwrap();
        conn.execute_batch("PRAGMA foreign_keys=ON").unwrap();
    }

    let db = Db::open(dir.path()).unwrap();
    let m = MessageRef::new("acc", "hex1");

    let version: i64 = db
        .read(|c| Ok(c.query_row("SELECT version FROM schema_version", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(version, sift::db::SCHEMA_VERSION);

    let (_, text, remote, _, _, _) = db.bodies_get(&m).await.unwrap().expect("body kept");
    assert_eq!(text.as_deref(), Some(body_text), "body text is byte-for-byte");
    assert_eq!(remote, 2, "render metadata kept");

    let recs = db.attachments_records(&m).await.unwrap();
    assert_eq!(
        recs[0].data.as_deref(),
        Some(&attachment_bytes[..]),
        "attachment bytes are byte-for-byte"
    );
    assert_eq!(recs[0].filename.as_deref(), Some("invoice.pdf"));

    // FTS rows: one per surviving message.
    let (msg_count, fts_count): (i64, i64) = db
        .read(|c| {
            Ok((
                c.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?,
                c.query_row("SELECT count(*) FROM messages_fts", [], |r| r.get(0))?,
            ))
        })
        .await
        .unwrap();
    assert_eq!(fts_count, msg_count);
    let hits = db
        .fts_search_local(std::slice::from_ref(&"acc".to_string()), "cached", None, None, None, None, None, None, None, None, 10)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1, "rebuilt FTS still matches the cached body");

    // The orphaned children were preserved in the recovery report, not kept in
    // the active tables and not silently discarded.
    let (orphan_labels, orphan_bodies, report): (i64, i64, i64) = db
        .read(|c| {
            Ok((
                c.query_row(
                    "SELECT count(*) FROM migration_recovery_rows WHERE kind='message_labels' AND row_json LIKE '%ghost%'",
                    [],
                    |r| r.get(0),
                )?,
                c.query_row(
                    "SELECT count(*) FROM migration_recovery_rows WHERE kind='bodies' AND row_json LIKE '%ghost%'",
                    [],
                    |r| r.get(0),
                )?,
                c.query_row("SELECT count(*) FROM migration_recovery_report", [], |r| r.get(0))?,
            ))
        })
        .await
        .unwrap();
    assert_eq!((orphan_labels, orphan_bodies), (1, 1));
    assert!(report >= 2, "each orphan class is reported");
    let live_labels: i64 = db
        .read(|c| {
            Ok(c.query_row(
                "SELECT count(*) FROM message_labels WHERE message_id='ghost'",
                [],
                |r| r.get(0),
            )?)
        })
        .await
        .unwrap();
    assert_eq!(live_labels, 0);

    let violations: i64 = db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(violations, 0);
}
