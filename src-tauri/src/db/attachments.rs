use super::Db;
use anyhow::Result;
use rusqlite::params;

#[derive(Clone)]
pub struct AttPut {
    pub id: String,
    pub message_id: String,
    pub gmail_att_id: Option<String>,
    pub part_id: String,
    pub filename: Option<String>,
    pub mime: String,
    pub size: i64,
    pub content_id: Option<String>,
    pub is_inline: bool,
    pub data: Option<Vec<u8>>,
}

impl Db {
    pub async fn attachments_put(&self, a: AttPut) -> Result<()> {
        self.write(move |c| {
      let dz: Option<Vec<u8>> = a.data.map(|d| zstd::encode_all(d.as_slice(), 3).unwrap_or_default());
      c.execute("INSERT OR REPLACE INTO attachments (id,message_id,gmail_att_id,part_id,filename,mime,size,content_id,is_inline,data_z) VALUES (?,?,?,?,?,?,?,?,?,?)",
        params![a.id,a.message_id,a.gmail_att_id,a.part_id,a.filename,a.mime,a.size,a.content_id,a.is_inline as i32,dz])?;
      Ok(())
    }).await
    }
    pub async fn attachments_for_message(
        &self,
        message_id: &str,
    ) -> Result<Vec<crate::dto::AttachmentMeta>> {
        let mid = message_id.to_string();
        self.read(move |c| {
      let mut s = c.prepare("SELECT id,filename,mime,size,is_inline,local_path,data_z FROM attachments WHERE message_id=? ORDER BY is_inline, filename")?;
      { let __v = s.query_map(params![mid], |r| {
        let dz: Option<Vec<u8>> = r.get("data_z")?;
        let lp: Option<String> = r.get("local_path")?;
        Ok(crate::dto::AttachmentMeta { id: r.get("id")?, filename: r.get("filename")?, mime: r.get("mime")?, size: r.get("size")?, is_inline: r.get::<_, i64>("is_inline")? != 0, downloaded: lp.is_some() || dz.is_some() })
      })?.collect::<Result<Vec<_>,_>>()?; Ok(__v) }
    }).await
    }
    pub async fn attachment_get(
        &self,
        id: &str,
    ) -> Result<
        Option<(
            String,
            String,
            Option<String>,
            String,
            Option<Vec<u8>>,
            Option<String>,
        )>,
    > {
        let id = id.to_string();
        self.read(move |c| {
      let mut s = c.prepare("SELECT message_id,part_id,gmail_att_id,mime,data_z,local_path FROM attachments WHERE id=?")?;
      let mut rows = s.query_map(params![id], |r| {
        let dz: Option<Vec<u8>> = r.get(4)?;
        let raw = dz.map(|b| zstd::decode_all(b.as_slice()).unwrap_or_default());
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(3)?, raw, r.get::<_, Option<String>>(5)?))
      })?;
      let __out = rows.next().transpose()?; Ok(__out)
    }).await
    }
    pub async fn attachment_by_msg_part(
        &self,
        message_id: &str,
        part_id: &str,
    ) -> Result<
        Option<(
            String,
            String,
            Option<String>,
            String,
            Option<Vec<u8>>,
            Option<String>,
            Option<String>,
        )>,
    > {
        let (m, p) = (message_id.to_string(), part_id.to_string());
        self.read(move |c| {
      let mut s = c.prepare("SELECT id,mime,gmail_att_id,data_z,local_path,filename,content_id FROM attachments WHERE message_id=? AND part_id=?")?;
      let mut rows = s.query_map(params![m,p], |r| {
        let dz: Option<Vec<u8>> = r.get(3)?;
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?, r.get::<_, String>(1)?, dz.map(|b| zstd::decode_all(b.as_slice()).unwrap_or_default()), r.get::<_, Option<String>>(4)?, r.get::<_, Option<String>>(5)?))
      })?;
      let __out = rows.next().transpose()?; Ok(__out)
    }).await
    }
}
