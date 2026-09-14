use super::Db;
use crate::dto::MessageRef;
use anyhow::Result;
use rusqlite::params;

/// `List-Unsubscribe` and its authentication evidence, known only once a
/// message has been parsed (P9.4). Stored through the body pipeline because
/// that is the one place a transport holds both the headers and the message
/// identity.
#[derive(Debug, Clone, Default)]
pub struct UnsubscribeHeaders {
    pub list_unsubscribe: Option<String>,
    pub post_value: Option<String>,
    pub auth_results: Option<String>,
    pub trusted: bool,
}

pub struct BodyPut {
    pub account_id: String,
    pub message_id: String,
    pub html: Option<String>,
    pub text: Option<String>,
    pub remote_images: i64,
    pub trackers: i64,
    pub dark_safe: bool,
    pub quoted_from: Option<i64>,
    pub unsubscribe: UnsubscribeHeaders,
}

fn z(s: &str) -> Vec<u8> {
    zstd::encode_all(s.as_bytes(), 3).unwrap_or_default()
}
/// Decode a stored zstd body payload. Unknown bytes decode to an empty
/// string rather than failing a read (P2.2 behaviour, reused by the draft
/// import path).
pub fn uz(b: &[u8]) -> String {
    zstd::decode_all(b)
        .map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_default()
}

impl Db {
    pub async fn bodies_put(&self, b: BodyPut) -> Result<()> {
        let (aid, mid, hz, tz) = (
            b.account_id.clone(),
            b.message_id.clone(),
            b.html.as_deref().map(z),
            b.text.as_deref().map(z),
        );
        let (ri, tc, ds, qf) = (
            b.remote_images,
            b.trackers,
            b.dark_safe as i32,
            b.quoted_from,
        );
        let aid_w = aid.clone();
        let mid_w = mid.clone();
        let (lu, lup, ar, art) = (
            b.unsubscribe.list_unsubscribe.clone(),
            b.unsubscribe.post_value.clone(),
            b.unsubscribe.auth_results.clone(),
            b.unsubscribe.trusted as i32,
        );
        let one_click = crate::unsubscribe::post_is_one_click(lup.as_deref()) as i32;
        self.write(move |c| {
      c.execute("INSERT INTO bodies (account_id,message_id,html_z,text_z,remote_image_count,tracker_count,dark_safe,quoted_from) VALUES (?,?,?,?,?,?,?,?) ON CONFLICT(account_id,message_id) DO UPDATE SET html_z=excluded.html_z, text_z=excluded.text_z, remote_image_count=excluded.remote_image_count, tracker_count=excluded.tracker_count, dark_safe=excluded.dark_safe, quoted_from=excluded.quoted_from",
        params![aid_w, mid_w, hz, tz, ri, tc, ds, qf])?;
      c.execute("UPDATE messages SET body_state='fetched', fetched_at=?, list_unsubscribe=COALESCE(?,list_unsubscribe), list_unsubscribe_post_value=COALESCE(?,list_unsubscribe_post_value), auth_results=COALESCE(?,auth_results), auth_results_trusted=MAX(auth_results_trusted,?), list_unsubscribe_post=MAX(list_unsubscribe_post,?) WHERE account_id=? AND id=?", params![super::now_ms(), lu, lup, ar, art, one_click, aid_w, mid_w])?;
      Ok(())
    }).await?;
        // index body text into FTS
        if let Some(t) = b.text {
            let (aid2, mid2) = (aid, mid);
            let capped: String = t.chars().take(50_000).collect();
            self.write(move |c| {
                c.execute(
                    "UPDATE messages_fts SET body=? WHERE account_id=? AND message_id=?",
                    params![capped, aid2, mid2],
                )?;
                Ok(())
            })
            .await?;
        }
        Ok(())
    }
    pub async fn bodies_get(
        &self,
        r: &MessageRef,
    ) -> Result<Option<(Option<String>, Option<String>, i64, i64, bool, Option<i64>)>> {
        let (aid, mid) = (r.account_id.clone(), r.message_id.clone());
        self.read(move |c| {
      let mut s = c.prepare("SELECT html_z,text_z,remote_image_count,tracker_count,dark_safe,quoted_from FROM bodies WHERE account_id=? AND message_id=?")?;
      let mut rows = s.query_map(params![aid, mid], |r| {
        let hz: Option<Vec<u8>> = r.get(0)?; let tz: Option<Vec<u8>> = r.get(1)?;
        Ok((hz.map(|b| uz(&b)), tz.map(|b| uz(&b)), r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)? != 0, r.get::<_, Option<i64>>(5)?))
      })?;
      let __out = rows.next().transpose()?; Ok(__out)
    }).await
    }
}
