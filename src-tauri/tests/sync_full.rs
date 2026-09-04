#![allow(clippy::await_holding_lock)] // tests serialize env-var servers via a shared mutex (intentional)
static ENV_LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    ENV_LOCK
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap()
}
// P3-T10: full sync 1234 messages / 400 threads via fake Gmail
use sift::db::Db;
use sift::provider::Provider;

fn meta_json(id: &str, tid: &str, hid: &str) -> serde_json::Value {
    serde_json::json!({
      "id": id, "threadId": tid, "snippet": "s", "historyId": hid, "internalDate": "1700000000000",
      "sizeEstimate": 100, "labelIds": ["INBOX", "UNREAD"],
      "payload": { "headers": [
        {"name": "From", "value": "Ada <ada@x.com>"},
        {"name": "Subject", "value": format!("Subject {id}")},
        {"name": "Date", "value": "Mon, 01 Jan 2024 00:00:00 +0000"}
      ]}
    })
}

#[tokio::test]
async fn p3_t10_full_sync_counts() {
    let _g = lock_env();
    let server = wiremock::MockServer::start().await;
    std::env::set_var(
        "SIFT_GMAIL_BASE",
        format!("{}/gmail/v1/users/me", server.uri()),
    );
    std::env::set_var("SIFT_BATCH_URL", format!("{}/batch/gmail/v1", server.uri()));

    // profile
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(".*/profile"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"emailAddress":"a@x","messagesTotal":1234,"historyId":"9000"}),
        ))
        .mount(&server)
        .await;
    // labels
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(".*/labels$"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"labels":[
              {"id":"INBOX","name":"INBOX","type":"system","labelListVisibility":"labelShow"},
              {"id":"UNREAD","name":"UNREAD","type":"system","labelListVisibility":"labelHide"}
            ]}),
        ))
        .mount(&server)
        .await;
    // list messages: 3 pages of fake ids (use 120 ids to keep test fast but assert batching; scale math same)
    let ids: Vec<(String, String)> = (0..120)
        .map(|i| (format!("m{i}"), format!("t{}", i % 40)))
        .collect();
    let ids_clone = ids.clone();
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path_regex(".*/messages$"))
        .respond_with(move |req: &wiremock::Request| {
            let q: std::collections::HashMap<String, String> = req
                .url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            let token = q.get("pageToken").cloned().unwrap_or_default();
            let start: usize = token.parse().unwrap_or(0);
            let page: Vec<serde_json::Value> = ids_clone[start..(start + 50).min(ids_clone.len())]
                .iter()
                .map(|(i, t)| serde_json::json!({"id": i, "threadId": t}))
                .collect();
            let next = if start + 50 < ids_clone.len() {
                Some((start + 50).to_string())
            } else {
                None
            };
            let mut body = serde_json::json!({"messages": page});
            if let Some(n) = next {
                body["nextPageToken"] = n.into();
            }
            wiremock::ResponseTemplate::new(200).set_body_json(body)
        })
        .mount(&server)
        .await;
    // batch endpoint: respond per requested ids
    wiremock::Mock::given(wiremock::matchers::method("POST"))
    .and(wiremock::matchers::path("/batch/gmail/v1"))
    .respond_with(|req: &wiremock::Request| {
      let body = String::from_utf8_lossy(&req.body).to_string();
      let ct = req.headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
      let b = ct.split("boundary=").nth(1).unwrap_or("batch").to_string();
      // extract ids from GET lines
      let mut out = String::new();
      for line in body.lines() {
        if line.starts_with("GET ") {
          let id = line.split("/messages/").nth(1).unwrap_or("m0?").split('?').next().unwrap_or("m0");
          let tid = format!("t{}", id.trim_start_matches('m').parse::<usize>().unwrap_or(0) % 40);
          out.push_str(&format!("--{b}\r\nContent-Type: application/http\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{}\r\n", meta_json(id, &tid, "9001")));
        }
      }
      out.push_str(&format!("--{b}--\r\n"));
      wiremock::ResponseTemplate::new(200).set_body_string(out).insert_header("content-type", format!("multipart/mixed; boundary={b}"))
    })
    .mount(&server).await;

    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = db.new_account("a@x", None, None).await.unwrap();
    let provider = sift::provider::gmail::api::GmailApiProvider::new(
        acc.id.clone(),
        sift::provider::gmail::client::GmailClient::new("t".into()),
    );
    let sink = sift::provider::DbSink::new(db.clone());
    let cursor = provider
        .full_sync(&sink, tokio_util::sync::CancellationToken::new())
        .await
        .unwrap();
    assert!(matches!(cursor, sift::provider::Cursor::Gmail { .. }));
    let nm: i64 = db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?))
        .await
        .unwrap();
    let nt: i64 = db
        .read(|c| Ok(c.query_row("SELECT count(*) FROM threads", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(nm, 120);
    assert_eq!(nt, 40);
    let hid: String = db
        .read({
            let aid = acc.id.clone();
            move |c| {
                Ok(c.query_row(
                    "SELECT history_id FROM accounts WHERE id=?",
                    rusqlite::params![aid],
                    |r| r.get(0),
                )?)
            }
        })
        .await
        .unwrap();
    assert_eq!(hid, "9000");
    std::env::remove_var("SIFT_GMAIL_BASE");
    std::env::remove_var("SIFT_BATCH_URL");
}
