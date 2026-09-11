//! End-to-end body pipeline: realistic newsletter HTML goes through the same
//! sanitize -> store -> read path as a foreground message_body fetch.

use sift::db::Db;
use sift::provider::gmail::mime::{ParsedMessage, ParsedPart};
use sift::provider::{store_parsed, DbSink};

const NEWSLETTER: &str = r##"<!doctype html><html><head><style>@media(max-width:600px){.hero{width:100%}}</style></head>
<body bgcolor="#111111"><table width="600" cellspacing="12" cellpadding="8" role="presentation"><tr>
<td nowrap background="https://cdn.example/bg.png"><picture>
<source media="(min-width:600px)" srcset="https://cdn.example/hero@2x.png 2x">
<img class="hero" src="https://cdn.example/hero.png" width="600" height="240" alt="hero">
<img src="https://cdn.example/banner.png" width="600" height="80" alt="banner">
<img src="cid:logo" width="120" height="40" alt="logo">
</picture></td></tr></table></body></html>"##;

#[tokio::test]
async fn newsletter_survives_fetch_store_and_read() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let account = db
        .new_account("reader@example.com", None, None)
        .await
        .unwrap();
    let account_id = account.id;
    db.write(move |connection| {
        connection.execute(
            "INSERT INTO messages (id,account_id,thread_id,internal_date,body_state) VALUES ('m1',?, 't1', 1, 'none')",
            rusqlite::params![account_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let parsed = ParsedMessage {
        html: Some(NEWSLETTER.into()),
        text: Some("newsletter".into()),
        inline: vec![ParsedPart {
            part_id: "2".into(),
            filename: None,
            mime: "image/png".into(),
            size: 3,
            content_id: Some("logo".into()),
            is_inline: true,
            data: vec![1, 2, 3],
            attachment_id: None,
        }],
        ..Default::default()
    };
    let sink = DbSink::new(db.clone());
    store_parsed(&sink, "m1", &parsed).await.unwrap();

    let stored = db.bodies_get("m1").await.unwrap().expect("body stored");
    let html = stored.0.expect("html stored");
    let remote_images = stored.2;

    // Serve-time embedding mirrors render_message_html in commands/threads.rs:
    // stored bodies keep sift-att:// refs; bytes are embedded when read.
    let parts = db.attachments_with_bytes("m1").await.unwrap();
    assert_eq!(parts.len(), 1);
    let html = sift::render::sanitize::embed_local_images(&html, "m1", &parts);

    for expected in [
        "width=",
        "cellspacing=",
        "<picture>",
        "srcset=",
        "cdn.example/hero.png",
        "cdn.example/banner.png",
        "@media(max-width:600px)",
    ] {
        assert!(html.contains(expected));
    }
    assert!(!html.contains("sift-att://"));
    assert!(html.contains("data:image/png;base64,"));
    assert!(remote_images >= 2);
}

const DOCUMENT: &str = r##"<!doctype html><html><head><title>Weekly Digest</title><style>.hero{color:#123}</style></head><body bgcolor="#f4f4f4" style="padding:24px"><h1>This week</h1><p>Hello.</p></body></html>"##;

#[tokio::test]
async fn document_head_and_body_survive_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let account = db
        .new_account("reader@example.com", None, None)
        .await
        .unwrap();
    let account_id = account.id;
    db.write(move |connection| {
        connection.execute(
            "INSERT INTO messages (id,account_id,thread_id,internal_date,body_state) VALUES ('m2',?, 't2', 1, 'none')",
            rusqlite::params![account_id],
        )?;
        Ok(())
    })
    .await
    .unwrap();

    let parsed = ParsedMessage {
        html: Some(DOCUMENT.into()),
        ..Default::default()
    };
    store_parsed(&DbSink::new(db.clone()), "m2", &parsed)
        .await
        .unwrap();

    let html = db.bodies_get("m2").await.unwrap().expect("body").0.unwrap();
    assert!(!html.contains("Weekly Digest"), "title leaked: {html}");
    assert!(html.contains("background-color:#f4f4f4"), "{html}");
    assert!(html.contains("padding:24px"), "{html}");
    assert!(html.contains(".hero{color:#123}"), "{html}");
    assert!(html.contains("<h1>This week</h1>"), "{html}");
}
