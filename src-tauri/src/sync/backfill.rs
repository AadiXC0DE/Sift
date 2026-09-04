use crate::db::Db;
use crate::gmail::client::GmailClient;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[derive(Clone)]
pub struct BackfillGate {
    pub foreground: Arc<AtomicUsize>,
}
impl Default for BackfillGate {
    fn default() -> Self {
        Self::new()
    }
}

impl BackfillGate {
    pub fn new() -> Self {
        Self {
            foreground: Arc::new(AtomicUsize::new(0)),
        }
    }
    pub fn enter(&self) -> FG {
        self.foreground.fetch_add(1, Ordering::SeqCst);
        FG { g: self.clone() }
    }
}
pub struct FG {
    g: BackfillGate,
}
impl Drop for FG {
    fn drop(&mut self) {
        self.g.foreground.fetch_sub(1, Ordering::SeqCst);
    }
}

pub async fn run_backfill_once(
    db: &Db,
    account_id: &str,
    client: &GmailClient,
    gate: &BackfillGate,
    horizon_days: i64,
) -> anyhow::Result<usize> {
    let min_date = crate::db::now_ms() - horizon_days * 24 * 3600 * 1000;
    let ids = db.next_bodies_to_fetch(account_id, 20, min_date).await?;
    if ids.is_empty() {
        return Ok(0);
    }
    if gate.foreground.load(Ordering::SeqCst) > 0 {
        return Ok(0);
    } // yield to foreground
    let mut n = 0;
    for id in ids {
        if gate.foreground.load(Ordering::SeqCst) > 0 {
            break;
        }
        match client.get_message_full(&id).await {
            Ok(m) => {
                let parsed = crate::gmail::mime::parse_full(&m);
                let html = parsed.html.clone();
                let text = parsed.text.clone();
                let (final_html, remote, trackers, dark_safe) = if let Some(h) = html {
                    let s = crate::render::sanitize::sanitize(&id, &h);
                    (Some(s.html), s.remote_images, s.trackers, s.dark_safe)
                } else if let Some(t) = text.clone() {
                    let (h, q) = crate::render::text::to_html(&t);
                    let s = crate::render::sanitize::sanitize(&id, &h);
                    let _ = q;
                    (Some(s.html), s.remote_images, s.trackers, s.dark_safe)
                } else {
                    (None, 0, 0, true)
                };
                // store attachments meta
                for p in parsed.attachments.iter().chain(parsed.inline.iter()) {
                    let small = if p.data.len() < 64 * 1024 {
                        Some(p.data.clone())
                    } else {
                        None
                    };
                    let _ = db
                        .attachments_put(crate::db::attachments::AttPut {
                            id: uuid::Uuid::now_v7().to_string(),
                            message_id: id.clone(),
                            gmail_att_id: p.attachment_id.clone(),
                            part_id: p.part_id.clone(),
                            filename: p.filename.clone(),
                            mime: p.mime.clone(),
                            size: p.size,
                            content_id: p.content_id.clone(),
                            is_inline: p.is_inline,
                            data: small,
                        })
                        .await;
                }
                let _ = db
                    .bodies_put(crate::db::bodies::BodyPut {
                        message_id: id.clone(),
                        html: final_html,
                        text,
                        remote_images: remote,
                        trackers,
                        dark_safe,
                        quoted_from: parsed.quoted_from.map(|q| q as i64),
                    })
                    .await;
                n += 1;
            }
            Err(_) => {
                // mark error to avoid hot loop? keep none so it retries later with backoff
                break;
            }
        }
    }
    Ok(n)
}
