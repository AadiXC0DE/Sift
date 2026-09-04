use super::Db;
use anyhow::Result;
use rusqlite::params;

pub struct BodyPut {
    pub message_id: String,
    pub html: Option<String>,
    pub text: Option<String>,
    pub remote_images: i64,
    pub trackers: i64,
    pub dark_safe: bool,
    pub quoted_from: Option<i64>,
}

fn z(s: &str) -> Vec<u8> {
    zstd::encode_all(s.as_bytes(), 3).unwrap_or_default()
}
fn uz(b: &[u8]) -> String {
    zstd::decode_all(b)
        .map(|v| String::from_utf8_lossy(&v).into_owned())
        .unwrap_or_default()
}

impl Db {
    pub async fn bodies_put(&self, b: BodyPut) -> Result<()> {
        let (mid, hz, tz) = (
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
        self.write(move |c| {
      c.execute("INSERT INTO bodies (message_id,html_z,text_z,remote_image_count,tracker_count,dark_safe,quoted_from) VALUES (?,?,?,?,?,?,?) ON CONFLICT(message_id) DO UPDATE SET html_z=excluded.html_z, text_z=excluded.text_z, remote_image_count=excluded.remote_image_count, tracker_count=excluded.tracker_count, dark_safe=excluded.dark_safe, quoted_from=excluded.quoted_from",
        params![mid, hz, tz, ri, tc, ds, qf])?;
      c.execute("UPDATE messages SET body_state='fetched', fetched_at=? WHERE id=?", params![super::now_ms(), mid])?;
      Ok(())
    }).await?;
        // index body text into FTS
        if let Some(t) = b.text {
            let mid2 = b.message_id.clone();
            let capped: String = t.chars().take(50_000).collect();
            self.write(move |c| {
                c.execute(
                    "UPDATE messages_fts SET body=? WHERE message_id=?",
                    params![capped, mid2],
                )?;
                Ok(())
            })
            .await?;
        }
        Ok(())
    }
    pub async fn bodies_get(
        &self,
        message_id: &str,
    ) -> Result<Option<(Option<String>, Option<String>, i64, i64, bool, Option<i64>)>> {
        let mid = message_id.to_string();
        self.read(move |c| {
      let mut s = c.prepare("SELECT html_z,text_z,remote_image_count,tracker_count,dark_safe,quoted_from FROM bodies WHERE message_id=?")?;
      let mut rows = s.query_map(params![mid], |r| {
        let hz: Option<Vec<u8>> = r.get(0)?; let tz: Option<Vec<u8>> = r.get(1)?;
        Ok((hz.map(|b| uz(&b)), tz.map(|b| uz(&b)), r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)? != 0, r.get::<_, Option<i64>>(5)?))
      })?;
      let __out = rows.next().transpose()?; Ok(__out)
    }).await
    }
}
