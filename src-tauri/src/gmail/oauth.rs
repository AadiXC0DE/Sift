use crate::errors::SiftError;
use oauth2::PkceCodeChallenge;
use std::collections::HashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const SCOPES: &[&str] = &[
    "https://www.googleapis.com/auth/gmail.modify",
    "https://www.googleapis.com/auth/userinfo.email",
    "https://www.googleapis.com/auth/userinfo.profile",
];

fn client_id() -> String {
    std::env::var("SIFT_GOOGLE_CLIENT_ID")
        .unwrap_or_else(|_| "test-client-id.apps.googleusercontent.com".into())
}
fn client_secret() -> String {
    std::env::var("SIFT_GOOGLE_CLIENT_SECRET").unwrap_or_else(|_| "test-secret".into())
}

pub struct PendingFlow {
    pub auth_url: String,
    pub state: String,
    pub verifier: String,
    pub port: u16,
    cancel: tokio::sync::oneshot::Sender<()>,
    done: tokio::sync::oneshot::Receiver<Result<String, SiftError>>,
}

fn build_url(port: u16, state: &str, challenge: &str) -> String {
    let scopes = SCOPES.join(" ");
    format!(
    "https://accounts.google.com/o/oauth2/v2/auth?client_id={}&redirect_uri={}&response_type=code&scope={}&access_type=offline&prompt=consent&state={}&code_challenge={}&code_challenge_method=S256",
    urlencoding::encode(&client_id()),
    urlencoding::encode(&format!("http://127.0.0.1:{port}")),
    urlencoding::encode(&scopes),
    urlencoding::encode(state),
    urlencoding::encode(challenge),
  )
}

pub fn start_flow() -> Result<PendingFlow, SiftError> {
    let std_listener =
        std::net::TcpListener::bind("127.0.0.1:0").map_err(|e| SiftError::Oauth(e.to_string()))?;
    std_listener
        .set_nonblocking(true)
        .map_err(|e| SiftError::Oauth(e.to_string()))?;
    let port = std_listener
        .local_addr()
        .map_err(|e| SiftError::Oauth(e.to_string()))?
        .port();
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let state = random_state();
    let auth_url = build_url(port, &state, pkce_challenge.as_str());
    let verifier_secret = pkce_verifier.secret().clone();
    let (cancel_tx, cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let expected_state = state.clone();
    tokio::spawn(run_loopback(
        std_listener,
        expected_state,
        done_tx,
        cancel_rx,
    ));
    Ok(PendingFlow {
        auth_url,
        state,
        verifier: verifier_secret,
        port,
        cancel: cancel_tx,
        done: done_rx,
    })
}

impl PendingFlow {
    pub fn auth_url(&self) -> &str {
        &self.auth_url
    }
    pub fn cancel(self) {
        let _ = self.cancel.send(());
    }
    pub async fn wait(self, timeout_secs: u64) -> Result<String, SiftError> {
        let PendingFlow { done, cancel, .. } = self;
        let _ = cancel;
        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), done)
            .await
            .map_err(|_| SiftError::oauth_timeout())?
            .map_err(|_| SiftError::oauth_timeout())?
    }
}

async fn run_loopback(
    std_listener: std::net::TcpListener,
    expected_state: String,
    done: tokio::sync::oneshot::Sender<Result<String, SiftError>>,
    mut cancel: tokio::sync::oneshot::Receiver<()>,
) {
    let listener = tokio::net::TcpListener::from_std(std_listener).unwrap();
    let success_html = include_str!("../../assets/oauth-done.html");
    loop {
        tokio::select! {
          _ = &mut cancel => { let _ = done.send(Err(SiftError::app("oauth_cancelled", "cancelled", false))); return; }
          conn = listener.accept() => {
            let Ok((mut stream, _)) = conn else { continue; };
            let mut buf = vec![0u8; 8192];
            let Ok(n) = stream.read(&mut buf).await else { continue; };
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let path = req.lines().next().unwrap_or("").split_whitespace().nth(1).unwrap_or("/").to_string();
            let query: HashMap<String,String> = path.split_once('?').map(|(_, q)| url::form_urlencoded::parse(q.as_bytes()).into_owned().collect()).unwrap_or_default();
            let code = query.get("code").cloned();
            let state = query.get("state").cloned();
            let ok = code.is_some() && state.as_deref() == Some(expected_state.as_str());
            let body = if ok { success_html } else { "<h1>Invalid state — try again</h1>" };
            let status = if ok { "200 OK" } else { "400 Bad Request" };
            let resp = format!("HTTP/1.1 {status}\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
            let _ = stream.write_all(resp.as_bytes()).await;
            if ok {
              let _ = done.send(Ok(code.unwrap()));
              return;
            }
          }
        }
    }
}

#[derive(Debug, Clone)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_in: i64,
}

pub async fn exchange(
    code: &str,
    verifier: &str,
    port: u16,
    http: &reqwest::Client,
) -> Result<Tokens, SiftError> {
    let params = [
        ("code", code.to_string()),
        ("client_id", client_id()),
        ("client_secret", client_secret()),
        ("redirect_uri", format!("http://127.0.0.1:{port}")),
        ("grant_type", "authorization_code".into()),
        ("code_verifier", verifier.to_string()),
    ];
    let base = std::env::var("SIFT_TOKEN_URL")
        .unwrap_or_else(|_| "https://oauth2.googleapis.com/token".into());
    let resp = http.post(&base).form(&params).send().await?;
    if !resp.status().is_success() {
        let t = resp.text().await.unwrap_or_default();
        if t.contains("invalid_grant") {
            return Err(SiftError::reauth(t));
        }
        return Err(SiftError::app("oauth", t, false));
    }
    let v: serde_json::Value = resp.json().await.map_err(SiftError::from)?;
    Ok(Tokens {
        access_token: v["access_token"].as_str().unwrap_or_default().into(),
        refresh_token: v["refresh_token"].as_str().map(|s| s.to_string()),
        expires_in: v["expires_in"].as_i64().unwrap_or(3600),
    })
}

pub async fn refresh(refresh_token: &str, http: &reqwest::Client) -> Result<Tokens, SiftError> {
    let params = [
        ("refresh_token", refresh_token.to_string()),
        ("client_id", client_id()),
        ("client_secret", client_secret()),
        ("grant_type", "refresh_token".into()),
    ];
    let base = std::env::var("SIFT_TOKEN_URL")
        .unwrap_or_else(|_| "https://oauth2.googleapis.com/token".into());
    let resp = http.post(&base).form(&params).send().await?;
    if !resp.status().is_success() {
        let t = resp.text().await.unwrap_or_default();
        if t.contains("invalid_grant") {
            return Err(SiftError::reauth(t));
        }
        return Err(SiftError::app("oauth", t, true));
    }
    let v: serde_json::Value = resp.json().await.map_err(SiftError::from)?;
    Ok(Tokens {
        access_token: v["access_token"].as_str().unwrap_or_default().into(),
        refresh_token: v["refresh_token"].as_str().map(|s| s.to_string()),
        expires_in: v["expires_in"].as_i64().unwrap_or(3600),
    })
}

fn random_state() -> String {
    use rand::Rng;
    let mut rng = rand::thread_rng();
    (0..48)
        .map(|_| {
            let b = rng.gen_range(0..62);

            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"[b as usize] as char
        })
        .collect()
}

pub fn build_auth_url_for_test() -> String {
    let (pkce, _v) = PkceCodeChallenge::new_random_sha256();
    build_url(12345, &random_state(), pkce.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn p2_t01_auth_url_shape() {
        let url = build_auth_url_for_test();
        assert!(url.contains("code_challenge_method=S256"), "{url}");
        assert!(url.contains("access_type=offline"), "{url}");
        assert!(url.contains("prompt=consent"), "{url}");
        for s in ["gmail.modify", "userinfo.email", "userinfo.profile"] {
            assert!(url.contains(s), "{url} missing {s}");
        }
        let state = url
            .split("state=")
            .nth(1)
            .unwrap_or("")
            .split('&')
            .next()
            .unwrap_or("");
        // urlencoded; decode % then check length
        assert!(state.len() >= 32, "state too short: {state}");
    }
}
