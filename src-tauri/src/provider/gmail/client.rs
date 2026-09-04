use crate::errors::SiftError;
use crate::provider::gmail::types::*;
use governor::{Quota, RateLimiter};
use std::num::NonZeroU32;
use std::sync::Arc;

fn base_url() -> String {
    std::env::var("SIFT_GMAIL_BASE")
        .unwrap_or_else(|_| "https://gmail.googleapis.com/gmail/v1/users/me".into())
}
fn batch_url() -> String {
    std::env::var("SIFT_BATCH_URL")
        .unwrap_or_else(|_| "https://www.googleapis.com/batch/gmail/v1".into())
}

#[derive(Clone)]
pub struct GmailClient {
    http: reqwest::Client,
    token: Arc<tokio::sync::RwLock<String>>,
    limiter: Arc<
        RateLimiter<
            governor::state::NotKeyed,
            governor::state::InMemoryState,
            governor::clock::DefaultClock,
        >,
    >,
}

impl GmailClient {
    pub fn new(token: String) -> Self {
        let http = reqwest::Client::builder()
            .user_agent(format!("Sift/{}", env!("CARGO_PKG_VERSION")))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self {
            http,
            token: Arc::new(tokio::sync::RwLock::new(token)),
            limiter: Arc::new(RateLimiter::direct(
                Quota::per_second(NonZeroU32::new(40).unwrap())
                    .allow_burst(NonZeroU32::new(10).unwrap()),
            )),
        }
    }
    pub async fn set_token(&self, t: String) {
        *self.token.write().await = t;
    }

    async fn quota(&self, _units: u32) {
        // 200 units/s expressed in 5-unit base permits (40/s, burst 10).
        // governor counts requests, not units; we loop `units/5`-ish permits for heavy calls.
        // Simple: 5-unit calls take 1 permit, 50-unit take 10, 100-unit take 20.
        let permits = ((_units as f64 / 5.0).ceil() as usize).max(1);
        for _ in 0..permits {
            loop {
                match self.limiter.check() {
                    Ok(()) => break,
                    Err(_) => tokio::time::sleep(std::time::Duration::from_millis(5)).await,
                }
            }
        }
    }

    async fn send_with_retry(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, SiftError> {
        let mut attempt = 0u32;
        let mut delay = std::time::Duration::from_secs(1);
        loop {
            let token = self.token.read().await.clone();
            let resp = req.try_clone().unwrap().bearer_auth(&token).send().await;
            match resp {
                Err(e) if e.is_timeout() || e.is_connect() => {
                    attempt += 1;
                    if attempt >= 8 {
                        return Err(SiftError::app("http", e.to_string(), true));
                    }
                    tokio::time::sleep(delay + jitter()).await;
                    delay = (delay * 2).min(std::time::Duration::from_secs(64));
                }
                Err(e) => return Err(SiftError::from(e)),
                Ok(r) => {
                    let status = r.status();
                    if status.as_u16() == 429 || status.is_server_error() {
                        let wait = r
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok())
                            .and_then(|s| s.parse::<u64>().ok())
                            .map(std::time::Duration::from_secs);
                        attempt += 1;
                        if attempt >= 8 {
                            return Err(SiftError::app("http", format!("status {status}"), true));
                        }
                        tokio::time::sleep(wait.unwrap_or(delay) + jitter()).await;
                        delay = (delay * 2).min(std::time::Duration::from_secs(64));
                        continue;
                    }
                    if status.as_u16() == 401 {
                        return Err(SiftError::reauth("unauthorized"));
                    }
                    if status.as_u16() == 404 {
                        return Err(SiftError::NotFound("not found".into()));
                    }
                    if !status.is_success() {
                        let body = r.text().await.unwrap_or_default();
                        return Err(SiftError::app("gmail", body, false));
                    }
                    return Ok(r);
                }
            }
        }
    }
}

fn jitter() -> std::time::Duration {
    // ±20%: simple pseudo-random from nanos
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    std::time::Duration::from_millis((n % 400) as u64)
}

const META_HEADERS: &str = "From&metadataHeaders=To&metadataHeaders=Cc&metadataHeaders=Bcc&metadataHeaders=Reply-To&metadataHeaders=Subject&metadataHeaders=Date&metadataHeaders=Message-ID&metadataHeaders=In-Reply-To&metadataHeaders=References&metadataHeaders=List-Unsubscribe&metadataHeaders=List-Unsubscribe-Post";
const META_FIELDS: &str =
    "id,threadId,labelIds,snippet,historyId,internalDate,sizeEstimate,payload/headers";

impl GmailClient {
    pub async fn get_profile(&self) -> Result<Profile, SiftError> {
        self.quota(1).await;
        let r = self
            .send_with_retry(self.http.get(format!("{}/profile", base_url())))
            .await?;
        r.json().await.map_err(SiftError::from)
    }
    pub async fn userinfo(&self) -> Result<Userinfo, SiftError> {
        self.quota(1).await;
        let base = std::env::var("SIFT_USERINFO_URL")
            .unwrap_or_else(|_| "https://openidconnect.googleapis.com/v1/userinfo".into());
        let r = self.send_with_retry(self.http.get(base)).await?;
        r.json().await.map_err(SiftError::from)
    }
    pub async fn list_labels(&self) -> Result<Vec<Label>, SiftError> {
        self.quota(1).await;
        let r = self
            .send_with_retry(self.http.get(format!("{}/labels", base_url())))
            .await?;
        let v: serde_json::Value = r.json().await.map_err(SiftError::from)?;
        Ok(serde_json::from_value(v["labels"].clone()).unwrap_or_default())
    }
    pub async fn create_label(&self, name: &str) -> Result<Label, SiftError> {
        self.quota(5).await;
        let r = self.send_with_retry(self.http.post(format!("{}/labels", base_url())).json(&serde_json::json!({"name": name, "labelListVisibility": "labelShow", "messageListVisibility": "show"}))).await?;
        r.json().await.map_err(SiftError::from)
    }
    pub async fn list_messages(
        &self,
        page_token: Option<&str>,
        q: Option<&str>,
        include_spam_trash: bool,
    ) -> Result<ListMessagesResponse, SiftError> {
        self.quota(5).await;
        let mut req = self.http.get(format!("{}/messages", base_url())).query(&[
            ("maxResults", "500"),
            (
                "includeSpamTrash",
                if include_spam_trash { "true" } else { "false" },
            ),
            ("fields", "messages(id,threadId),nextPageToken"),
        ]);
        if let Some(p) = page_token {
            req = req.query(&[("pageToken", p)]);
        }
        if let Some(q) = q {
            req = req.query(&[("q", q)]);
        }
        let r = self.send_with_retry(req).await?;
        r.json().await.map_err(SiftError::from)
    }
    pub async fn get_message_meta(&self, id: &str) -> Result<Message, SiftError> {
        self.quota(5).await;
        let url = format!(
            "{}/messages/{}?format=metadata&metadataHeaders={META_HEADERS}&fields={META_FIELDS}",
            base_url(),
            id
        );
        let r = self.send_with_retry(self.http.get(url)).await?;
        r.json().await.map_err(SiftError::from)
    }
    pub async fn get_message_full(&self, id: &str) -> Result<Message, SiftError> {
        self.quota(5).await;
        let r = self
            .send_with_retry(
                self.http
                    .get(format!("{}/messages/{}?format=full", base_url(), id)),
            )
            .await?;
        r.json().await.map_err(SiftError::from)
    }
    pub async fn get_message_raw(&self, id: &str) -> Result<Message, SiftError> {
        self.quota(5).await;
        let r = self
            .send_with_retry(self.http.get(format!(
                "{}/messages/{}?format=raw&fields=raw",
                base_url(),
                id
            )))
            .await?;
        r.json().await.map_err(SiftError::from)
    }
    pub async fn get_attachment(
        &self,
        msg_id: &str,
        att_id: &str,
    ) -> Result<Attachment, SiftError> {
        self.quota(5).await;
        let r = self
            .send_with_retry(self.http.get(format!(
                "{}/messages/{}/attachments/{}",
                base_url(),
                msg_id,
                att_id
            )))
            .await?;
        r.json().await.map_err(SiftError::from)
    }
    pub async fn history_list(
        &self,
        start: &str,
        page_token: Option<&str>,
    ) -> Result<HistoryResponse, SiftError> {
        self.quota(2).await;
        let mut req = self.http.get(format!("{}/history", base_url())).query(&[
            ("startHistoryId", start),
            ("maxResults", "500"),
            ("historyTypes", "messageAdded"),
            ("historyTypes", "messageDeleted"),
            ("historyTypes", "labelAdded"),
            ("historyTypes", "labelRemoved"),
        ]);
        if let Some(p) = page_token {
            req = req.query(&[("pageToken", p)]);
        }
        let r = self.send_with_retry(req).await?;
        r.json().await.map_err(SiftError::from)
    }
    pub async fn batch_modify(
        &self,
        ids: Vec<String>,
        add: Vec<String>,
        remove: Vec<String>,
    ) -> Result<(), SiftError> {
        if ids.is_empty() {
            return Ok(());
        }
        self.quota(50).await;
        // Gmail batchModify caps at 1000 ids
        for chunk in ids.chunks(1000) {
            let body = BatchModifyRequest {
                ids: chunk.to_vec(),
                add: add.clone(),
                remove: remove.clone(),
            };
            self.send_with_retry(
                self.http
                    .post(format!("{}/messages/batchModify", base_url()))
                    .json(&body),
            )
            .await?;
        }
        Ok(())
    }
    pub async fn modify_message(
        &self,
        id: &str,
        add: Vec<String>,
        remove: Vec<String>,
    ) -> Result<(), SiftError> {
        self.quota(5).await;
        self.send_with_retry(
            self.http
                .post(format!("{}/messages/{}/modify", base_url(), id))
                .json(&ModifyRequest { add, remove }),
        )
        .await?;
        Ok(())
    }
    pub async fn trash_thread(&self, id: &str) -> Result<(), SiftError> {
        self.quota(10).await;
        self.send_with_retry(
            self.http
                .post(format!("{}/threads/{}/trash", base_url(), id)),
        )
        .await?;
        Ok(())
    }
    pub async fn untrash_thread(&self, id: &str) -> Result<(), SiftError> {
        self.quota(10).await;
        self.send_with_retry(
            self.http
                .post(format!("{}/threads/{}/untrash", base_url(), id)),
        )
        .await?;
        Ok(())
    }
    pub async fn delete_thread(&self, id: &str) -> Result<(), SiftError> {
        self.quota(20).await;
        self.send_with_retry(self.http.delete(format!("{}/threads/{}", base_url(), id)))
            .await?;
        Ok(())
    }
    pub async fn send_raw(&self, raw: &str, thread_id: Option<&str>) -> Result<Message, SiftError> {
        self.quota(100).await;
        let body = SendRequest {
            raw: raw.into(),
            thread_id: thread_id.map(|s| s.to_string()),
        };
        let r = self
            .send_with_retry(
                self.http
                    .post(format!("{}/messages/send", base_url()))
                    .json(&body),
            )
            .await?;
        r.json().await.map_err(SiftError::from)
    }
    pub async fn send_as_list(&self) -> Result<serde_json::Value, SiftError> {
        self.quota(1).await;
        let base = std::env::var("SIFT_SETTINGS_URL").unwrap_or_else(|_| {
            "https://gmail.googleapis.com/gmail/v1/users/me/settings/sendAs".into()
        });
        let r = self.send_with_retry(self.http.get(base)).await?;
        r.json().await.map_err(SiftError::from)
    }

    /// Batched metadata/full fetch, ≤50 per HTTP request. Returns per-id results (404 → NotFound).
    pub async fn batch_get_messages(
        &self,
        ids: &[String],
        format: &str,
    ) -> Result<Vec<Result<Message, SiftError>>, SiftError> {
        let mut out = vec![];
        for chunk in ids.chunks(50) {
            self.quota(5 * chunk.len() as u32).await;
            let boundary = format!("batch_{}", rand_boundary());
            let mut body = String::new();
            for id in chunk {
                let path = if format == "metadata" {
                    format!("/gmail/v1/users/me/messages/{id}?format=metadata&metadataHeaders={META_HEADERS}&fields={META_FIELDS}")
                } else {
                    format!("/gmail/v1/users/me/messages/{id}?format={format}")
                };
                body.push_str(&format!("--{boundary}\r\nContent-Type: application/http\r\nContent-ID: {id}\r\n\r\nGET {path} HTTP/1.1\r\n\r\n"));
            }
            body.push_str(&format!("--{boundary}--\r\n"));
            let token = self.token.read().await.clone();
            let resp = self
                .http
                .post(batch_url())
                .header(
                    "Content-Type",
                    format!("multipart/mixed; boundary={boundary}"),
                )
                .bearer_auth(&token)
                .body(body)
                .send()
                .await
                .map_err(SiftError::from)?;
            if resp.status().as_u16() == 401 {
                return Err(SiftError::reauth("batch 401"));
            }
            let ct = resp
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("")
                .to_string();
            let text = resp.text().await.map_err(SiftError::from)?;
            let b = ct
                .split("boundary=")
                .nth(1)
                .unwrap_or("batch")
                .trim_matches('"')
                .trim()
                .to_string();
            out.extend(parse_batch_response(&text, &b, chunk));
        }
        Ok(out)
    }
}

fn rand_boundary() -> String {
    use rand::Rng;
    rand::thread_rng().gen_range(100000..999999).to_string()
}

fn parse_batch_response(
    text: &str,
    boundary: &str,
    ids: &[String],
) -> Vec<Result<Message, SiftError>> {
    // Split parts; each part contains an HTTP status line + JSON body
    let mut parts: Vec<(u16, String)> = vec![];
    for chunk in text.split(&format!("--{boundary}")) {
        let chunk = chunk.trim();
        if chunk.is_empty() || chunk == "--" {
            continue;
        }
        // Find inner HTTP status
        let status: u16 = chunk
            .lines()
            .find(|l| l.starts_with("HTTP/"))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(200);
        // Body is after blank line following the inner headers
        let body = chunk
            .split("\r\n\r\n")
            .last()
            .unwrap_or("")
            .trim()
            .trim_end_matches("--")
            .trim()
            .to_string();
        // The outer part headers precede inner response; the LAST \r\n\r\n is the JSON
        parts.push((status, body));
    }
    ids.iter()
        .enumerate()
        .map(|(i, _)| {
            if i >= parts.len() {
                return Err(SiftError::app("batch", "missing part", true));
            }
            let (st, body) = &parts[i];
            if *st == 404 {
                return Err(SiftError::NotFound("message gone".into()));
            }
            if *st < 200 || *st >= 300 {
                return Err(SiftError::app("batch", body.clone(), true));
            }
            serde_json::from_str::<Message>(body).map_err(SiftError::from)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn p3_t01_batch_shape() {
        let server = wiremock::MockServer::start().await;
        std::env::set_var("SIFT_BATCH_URL", format!("{}/batch/gmail/v1", server.uri()));
        let expected_ids: Vec<String> = (0..3).map(|i| format!("m{i}")).collect();
        wiremock::Mock::given(wiremock::matchers::method("POST"))
      .and(wiremock::matchers::path("/batch/gmail/v1"))
      .respond_with(move |req: &wiremock::Request| {
        let body = String::from_utf8_lossy(&req.body).to_string();
        assert!(body.contains("multipart/mixed") || body.contains("GET /gmail/v1/users/me/messages/m0"));
        // Build a fake multipart response
        let ct = req.headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let b = ct.split("boundary=").nth(1).unwrap_or("batch").to_string();
        let mut resp = String::new();
        for i in 0..3 {
          resp.push_str(&format!("--{b}\r\nContent-Type: application/http\r\n\r\nHTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\r\n{{\"id\":\"m{i}\",\"threadId\":\"t{i}\"}}\r\n"));
        }
        resp.push_str(&format!("--{b}--\r\n"));
        wiremock::ResponseTemplate::new(200).set_body_string(resp).insert_header("content-type", format!("multipart/mixed; boundary={b}"))
      })
      .expect(1)
      .mount(&server)
      .await;
        // Re-point: batch_url reads env each call
        let c = GmailClient::new("tok".into());
        let res = c
            .batch_get_messages(&expected_ids, "metadata")
            .await
            .unwrap();
        assert_eq!(res.len(), 3);
        std::env::remove_var("SIFT_BATCH_URL");
    }

    #[tokio::test]
    async fn p3_t02_batch_404_part() {
        let server = wiremock::MockServer::start().await;
        std::env::set_var("SIFT_BATCH_URL", format!("{}/batch/gmail/v1", server.uri()));
        wiremock::Mock::given(wiremock::matchers::method("POST"))
      .respond_with(|req: &wiremock::Request| {
        let ct = req.headers.get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_string();
        let b = ct.split("boundary=").nth(1).unwrap_or("batch").to_string();
        let resp = format!("--{b}\r\nContent-Type: application/http\r\n\r\nHTTP/1.1 200 OK\r\n\r\n{{\"id\":\"m0\",\"threadId\":\"t0\"}}\r\n--{b}\r\nContent-Type: application/http\r\n\r\nHTTP/1.1 404 Not Found\r\n\r\n{{}}\r\n--{b}--\r\n");
        wiremock::ResponseTemplate::new(200).set_body_string(resp).insert_header("content-type", format!("multipart/mixed; boundary={b}"))
      })
      .mount(&server)
      .await;
        let c = GmailClient::new("tok".into());
        let res = c
            .batch_get_messages(&["m0".to_string(), "m1".to_string()], "metadata")
            .await
            .unwrap();
        assert!(res[0].is_ok());
        assert!(matches!(res[1], Err(SiftError::NotFound(_))));
        std::env::remove_var("SIFT_BATCH_URL");
    }
}
