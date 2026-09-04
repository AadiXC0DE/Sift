use crate::db::{attachments::AttPut, bodies::BodyPut, messages::MsgUpsert, Db};
use crate::dto::Label;
use crate::render::sanitize::sanitize;
use anyhow::Result;

pub fn is_demo() -> bool {
    std::env::var("SIFT_DEMO").as_deref() == Ok("1")
}

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;

/// Seed a fictional two-account mailbox so the app is fully explorable with no
/// credentials (`SIFT_DEMO=1`). Idempotent: does nothing when accounts exist.
pub async fn seed_if_enabled(db: &Db) -> Result<bool> {
    if !is_demo() {
        return Ok(false);
    }
    if !db.accounts_list().await?.is_empty() {
        return Ok(false);
    }
    let now = crate::db::now_ms();
    let ada = db
        .new_account("ada@acme.com", Some("Ada Lovelace".into()), None)
        .await?;
    let ben = db
        .new_account("ben@globex.io", Some("Ben Carter".into()), None)
        .await?;
    let mut ada_db = ada.clone();
    ada_db.color = "blue".into();
    let mut ben_db = ben.clone();
    ben_db.color = "rose".into();
    db.accounts_update_meta(
        &ada.id,
        Some("blue".into()),
        None,
        Some("<p>— Ada</p>".into()),
        Some(0),
    )
    .await?;
    db.accounts_update_meta(&ben.id, Some("rose".into()), None, None, Some(1))
        .await?;

    for aid in [&ada.id, &ben.id] {
        for (i, id) in [
            "INBOX",
            "STARRED",
            "SENT",
            "DRAFT",
            "SPAM",
            "TRASH",
            "UNREAD",
            "IMPORTANT",
        ]
        .iter()
        .enumerate()
        {
            db.labels_upsert(&Label {
                account_id: aid.clone(),
                id: id.to_string(),
                name: id.to_string(),
                kind: "system".into(),
                color_bg: None,
                color_fg: None,
                visible: *id != "UNREAD",
                unread_count: 0,
                total_count: 0,
                sort_order: i as i64,
            })
            .await?;
        }
    }
    for (id, name) in [
        ("L-acme", "Clients/Acme"),
        ("L-globex", "Clients/Globex"),
        ("L-receipts", "Receipts"),
        ("L-snoozed", "Sift/Snoozed"),
    ] {
        db.labels_upsert(&Label {
            account_id: ada.id.clone(),
            id: id.into(),
            name: name.into(),
            kind: "user".into(),
            color_bg: None,
            color_fg: None,
            visible: true,
            unread_count: 0,
            total_count: 0,
            sort_order: 200,
        })
        .await?;
    }
    for (id, name) in [("L-initech", "Clients/Initech"), ("L-receipts", "Receipts")] {
        db.labels_upsert(&Label {
            account_id: ben.id.clone(),
            id: id.into(),
            name: name.into(),
            kind: "user".into(),
            color_bg: None,
            color_fg: None,
            visible: true,
            unread_count: 0,
            total_count: 0,
            sort_order: 200,
        })
        .await?;
    }

    // (id, thread, hours_ago, from_name, from_email, subject, snippet, labels, unread, starred, has_att)
    #[allow(clippy::too_many_arguments)]
    struct M<'a>(
        &'a str,
        &'a str,
        i64,
        &'a str,
        &'a str,
        &'a str,
        &'a str,
        Vec<&'a str>,
        bool,
        bool,
        bool,
    );
    let ada_mail = [
        M(
            "da-m1",
            "da-t1",
            1,
            "Grace Hopper",
            "grace@acme.com",
            "Launch checklist — final review",
            "Went through the list twice, one open item on…",
            vec!["INBOX", "UNREAD", "IMPORTANT"],
            true,
            false,
            false,
        ),
        M(
            "da-m2",
            "da-t2",
            2,
            "Linus Torvalds",
            "linus@globex.io",
            "Re: Q3 numbers",
            "Looks good, one thing on row 14…",
            vec!["INBOX", "UNREAD"],
            true,
            true,
            true,
        ),
        M(
            "da-m3",
            "da-t2",
            3,
            "Ada Lovelace",
            "ada@acme.com",
            "Re: Q3 numbers",
            "Fixed — take another look?",
            vec!["SENT"],
            false,
            false,
            false,
        ),
        M(
            "da-m4",
            "da-t3",
            5,
            "Console Digest",
            "digest@console.dev",
            "[Product] Weekly digest",
            "Highlights: edge runtimes, local-first sync…",
            vec!["INBOX", "UNREAD"],
            true,
            false,
            false,
        ),
        M(
            "da-m5",
            "da-t4",
            8,
            "Margaret Hamilton",
            "margaret@acme.com",
            "Contract v2 attached",
            "Signed copy attached, please countersign…",
            vec!["INBOX", "L-acme"],
            false,
            false,
            true,
        ),
        M(
            "da-m6",
            "da-t5",
            26,
            "Alan Turing",
            "alan@globex.io",
            "Design feedback",
            "Three notes on the new thread view…",
            vec!["INBOX"],
            false,
            true,
            false,
        ),
        M(
            "da-m7",
            "da-t6",
            30,
            "Ada Lovelace",
            "ada@acme.com",
            "Invoice #1042",
            "Thanks — receipt attached.",
            vec!["SENT", "L-receipts"],
            false,
            false,
            true,
        ),
        M(
            "da-m8",
            "da-t7",
            50,
            "Katherine Johnson",
            "katherine@acme.com",
            "Offsite notes",
            "Action items from Friday…",
            vec!["INBOX", "L-acme"],
            false,
            false,
            false,
        ),
        M(
            "da-m9",
            "da-t8",
            55,
            "Flightdesk",
            "noreply@flightdesk.io",
            "Your receipt",
            "Thanks for your purchase of $18.40…",
            vec!["L-receipts"],
            false,
            false,
            false,
        ),
        M(
            "da-m10",
            "da-t9",
            80,
            "Ada Lovelace",
            "ada@acme.com",
            "Draft: roadmap thoughts",
            "Not ready to send…",
            vec!["DRAFT"],
            false,
            false,
            false,
        ),
        M(
            "da-m11",
            "da-t10",
            90,
            "Prize Committee",
            "prizes@spam.example",
            "You WON a prize!!!",
            "Click here to claim…",
            vec!["SPAM"],
            false,
            false,
            false,
        ),
        M(
            "da-m12",
            "da-t11",
            100,
            "Old Newsletter",
            "news@old.example",
            "10x your inbox",
            "Unsubscribe below…",
            vec!["TRASH"],
            false,
            false,
            false,
        ),
        M(
            "da-m13",
            "da-t12",
            10,
            "Grace Hopper",
            "grace@acme.com",
            "Compiler milestone hit",
            "All green on the new backend…",
            vec!["INBOX", "UNREAD"],
            true,
            false,
            false,
        ),
        M(
            "da-m14",
            "da-t13",
            14,
            "Linus Torvalds",
            "linus@globex.io",
            "Kernel sync notes",
            "Pulled the latest, two conflicts…",
            vec!["INBOX", "L-globex"],
            false,
            false,
            false,
        ),
        M(
            "da-m15",
            "da-t14",
            22,
            "Margaret Hamilton",
            "margaret@acme.com",
            "Hiring: systems engineer",
            "Loop is set for Thursday…",
            vec!["INBOX", "UNREAD", "L-acme"],
            true,
            true,
            false,
        ),
        M(
            "da-m16",
            "da-t15",
            33,
            "Alan Turing",
            "alan@globex.io",
            "Paper draft",
            "Comments inline, take a look…",
            vec!["INBOX"],
            false,
            false,
            true,
        ),
        M(
            "da-m17",
            "da-t16",
            44,
            "Katherine Johnson",
            "katherine@acme.com",
            "Trajectory tables",
            "Updated numbers attached…",
            vec!["INBOX", "L-acme"],
            false,
            false,
            true,
        ),
    ];
    let ben_mail = [
        M(
            "db-m1",
            "db-t1",
            2,
            "Jean Bartik",
            "jean@initech.com",
            "Sprint demo recording",
            "Recording + timestamps inside…",
            vec!["INBOX", "UNREAD"],
            true,
            false,
            false,
        ),
        M(
            "db-m2",
            "db-t2",
            6,
            "Ben Carter",
            "ben@globex.io",
            "Re: Sprint demo recording",
            "Watched — shipping it.",
            vec!["SENT"],
            false,
            false,
            false,
        ),
        M(
            "db-m3",
            "db-t3",
            20,
            "Ada Lovelace",
            "ada@acme.com",
            "Initech SOW",
            "Statement of work for Q4…",
            vec!["INBOX", "L-initech"],
            false,
            true,
            true,
        ),
        M(
            "db-m4",
            "db-t4",
            40,
            "Console Digest",
            "digest@console.dev",
            "[Product] Weekly digest",
            "Highlights from the week…",
            vec!["INBOX"],
            false,
            false,
            false,
        ),
        M(
            "db-m5",
            "db-t5",
            70,
            "Ben Carter",
            "ben@globex.io",
            "Trip receipts",
            "Hotels + flights…",
            vec!["SENT", "L-receipts"],
            false,
            false,
            false,
        ),
        M(
            "db-m6",
            "db-t6",
            9,
            "Jean Bartik",
            "jean@initech.com",
            "Test plan review",
            "Left comments on section 3…",
            vec!["INBOX", "UNREAD", "L-initech"],
            true,
            false,
            false,
        ),
        M(
            "db-m7",
            "db-t7",
            16,
            "Grace Hopper",
            "grace@acme.com",
            "Intro: Ben <> Alan",
            "Alan — meet Ben…",
            vec!["INBOX"],
            false,
            false,
            false,
        ),
        M(
            "db-m8",
            "db-t8",
            28,
            "Console Digest",
            "digest@console.dev",
            "Your weekly readout",
            "Top posts + releases…",
            vec!["INBOX", "UNREAD"],
            true,
            false,
            false,
        ),
    ];

    for m in ada_mail.iter().chain(ben_mail.iter()) {
        let aid = if m.0.starts_with("da-") {
            &ada.id
        } else {
            &ben.id
        };
        let labels: Vec<String> = m.7.iter().map(|s| s.to_string()).collect();
        db.messages_upsert(MsgUpsert {
            id: m.0.into(),
            account_id: aid.clone(),
            thread_id: m.1.into(),
            history_id: None,
            internal_date: now - m.2 * HOUR,
            from_name: Some(m.3.into()),
            from_email: Some(m.4.into()),
            to_json: "[]".into(),
            cc_json: "[]".into(),
            bcc_json: "[]".into(),
            reply_to: None,
            subject: m.5.into(),
            snippet: m.6.into(),
            rfc_message_id: Some(format!("<{}@demo.sift>", m.0)),
            in_reply_to: None,
            references_json: "[]".into(),
            list_unsubscribe: if m.0 == "da-m4" {
                Some("<https://console.dev/unsub>, <mailto:leave@console.dev>".into())
            } else {
                None
            },
            list_unsubscribe_post: m.0 == "da-m4",
            size_estimate: Some(4096),
            has_attachments: m.10,
            is_unread: m.8,
            is_starred: m.9,
            is_draft: labels.contains(&"DRAFT".to_string()),
            is_sent_by_me: labels.contains(&"SENT".to_string()),
            label_ids: labels,
        })
        .await?;
    }

    // Bodies for the newest messages (HTML goes through the real sanitizer).
    let bodies: Vec<(&str, &str)> = vec![
        ("da-m1", "<h2>Launch checklist</h2><p>Went through the list twice. One open item on the <b>release notes</b> — see <a href=\"https://example.com/notes\">notes</a>.</p>"),
        ("da-m2", "<p>Looks good, one thing on row 14.</p><div class=\"gmail_quote\"><p>On Tuesday, Ada wrote: here are the Q3 numbers…</p></div>"),
        ("da-m4", "<table><tr><td><h3>Weekly digest</h3></td></tr><tr><td>Edge runtimes, local-first sync, and keyboard-first triage.</td></tr></table><img src=\"https://example.com/hero.jpg\" width=\"600\" height=\"200\"><img src=\"https://open.tracker.example/o.gif\" width=\"1\" height=\"1\">"),
        ("da-m5", "<p>Signed copy attached, please countersign.</p>"),
        ("da-m6", "<blockquote><p>Previous design thread…</p></blockquote><p>Three notes on the new thread view: density, chips, hover actions.</p>"),
        ("db-m1", "<p>Recording + timestamps inside. <a href=\"https://example.com/demo\">Watch</a></p>"),
        ("db-m3", "<p>Statement of work for Q4 is ready for review.</p>"),
    ];
    for (mid, html) in bodies {
        let s = sanitize(mid, html);
        db.bodies_put(BodyPut {
            message_id: mid.into(),
            html: Some(s.html),
            text: None,
            remote_images: s.remote_images,
            trackers: s.trackers,
            dark_safe: s.dark_safe,
            quoted_from: None,
        })
        .await?;
    }
    // One PDF-ish attachment + one inline image (bytes inline, served via sift-att://).
    db.attachments_put(AttPut {
        id: "demo-att1".into(),
        message_id: "da-m5".into(),
        gmail_att_id: None,
        part_id: "att1".into(),
        filename: Some("contract-v2.pdf".into()),
        mime: "application/pdf".into(),
        size: 2100,
        content_id: None,
        is_inline: false,
        data: Some(b"%PDF-1.4 demo".to_vec()),
    })
    .await?;
    db.attachments_put(AttPut {
        id: "demo-att2".into(),
        message_id: "da-m2".into(),
        gmail_att_id: None,
        part_id: "img1".into(),
        filename: Some("row14.png".into()),
        mime: "image/png".into(),
        size: 70,
        content_id: Some("ii_row14".into()),
        is_inline: true,
        data: Some(
            base64::Engine::decode(
                &base64::engine::general_purpose::STANDARD,
                "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
            )
            .unwrap_or_default(),
        ),
    })
    .await?;

    // Snoozed threads (leave INBOX already removed by seed labels).
    let wake1 = now + 20 * HOUR;
    let wake2 = now + 3 * DAY;
    for (tid, wake) in [("da-t3", wake1), ("da-t8", wake2)] {
        db.write({
            let (aid, tid) = (ada.id.clone(), tid.to_string());
            move |c| {
                c.execute(
                    "INSERT OR REPLACE INTO snoozes (account_id,thread_id,wake_at) VALUES (?,?,?)",
                    rusqlite::params![aid, tid, wake],
                )?;
                c.execute(
                    "UPDATE threads SET snoozed_until=? WHERE account_id=? AND id=?",
                    rusqlite::params![wake, aid, tid],
                )?;
                Ok(())
            }
        })
        .await?;
    }
    // da-t3/da-t8 are snoozed: leave INBOX (snoozed view filters on snoozed_until).
    for mid in ["da-m4", "da-m8"] {
        db.apply_label_change(mid, &[], &["INBOX".to_string(), "UNREAD".to_string()])
            .await?;
    }

    for (aid, email, name) in [
        (ada.id.clone(), "grace@acme.com", "Grace Hopper"),
        (ada.id.clone(), "margaret@acme.com", "Margaret Hamilton"),
        (ben.id.clone(), "jean@initech.com", "Jean Bartik"),
    ] {
        db.contacts_upsert(&aid, email, Some(name)).await?;
    }

    db.write(|c| {
        c.execute(
            "INSERT INTO sync_log (account_id,at,kind,detail) VALUES (?,?,?,?)",
            rusqlite::params![String::new(), crate::db::now_ms(), "demo-seed", "done"],
        )?;
        Ok(())
    })
    .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn demo_seed_builds_browsable_mailbox() {
        std::env::set_var("SIFT_DEMO", "1");
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        assert!(seed_if_enabled(&db).await.unwrap());
        // idempotent
        assert!(!seed_if_enabled(&db).await.unwrap());
        let accs = db.accounts_list().await.unwrap();
        assert_eq!(accs.len(), 2);
        let inbox: i64 = db
            .read(|c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM threads WHERE in_inbox=1 AND in_trash=0 AND in_spam=0 AND snoozed_until IS NULL",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        assert!(inbox >= 15, "inbox threads: {inbox}");
        let bodies: i64 = db
            .read(|c| Ok(c.query_row("SELECT count(*) FROM bodies", [], |r| r.get(0))?))
            .await
            .unwrap();
        assert!(bodies >= 5, "bodies: {bodies}");
        let snoozed: i64 = db
            .read(|c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM threads WHERE snoozed_until IS NOT NULL",
                    [],
                    |r| r.get(0),
                )?)
            })
            .await
            .unwrap();
        assert_eq!(snoozed, 2);
        std::env::remove_var("SIFT_DEMO");
    }
}
