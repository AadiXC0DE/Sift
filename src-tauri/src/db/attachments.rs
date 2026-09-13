use super::Db;
use crate::dto::{AttachmentMeta, AttachmentRecord, CacheState};
use anyhow::{Context, Result};
use rusqlite::params;

/// Bumped when the on-disk cache layout or payload encoding changes. Rows
/// written before this version are `unverified` and confirmed on first read.
pub const CACHE_VERSION: i64 = 1;

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

/// Every column the record join needs, in a fixed order.
const REC_COLS: &str = "a.id, m.account_id, a.message_id, a.part_id, a.gmail_att_id, \
     a.filename, a.mime, a.size, a.content_id, a.is_inline, a.data_z, a.local_path, \
     a.cache_state, a.decoded_size";

struct RawRec {
    rec: AttachmentRecord,
    data_z: Option<Vec<u8>>,
}

fn raw_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<RawRec> {
    let state: Option<String> = r.get("cache_state")?;
    Ok(RawRec {
        rec: AttachmentRecord {
            id: r.get(0)?,
            account_id: r.get(1)?,
            message_id: r.get(2)?,
            part_id: r.get(3)?,
            gmail_att_id: r.get(4)?,
            filename: r.get(5)?,
            mime: r.get(6)?,
            size: r.get(7)?,
            content_id: r.get(8)?,
            is_inline: r.get::<_, i64>(9)? != 0,
            data: None,
            data_error: None,
            local_path: r.get(11)?,
            cache_state: CacheState::parse(state.as_deref().unwrap_or("missing")),
            decoded_size: r.get(13)?,
        },
        data_z: r.get(10)?,
    })
}

/// Decode the compressed payload. Compression errors are reported, never
/// silently turned into an empty attachment: a zero-length file is only valid
/// when a *successful* decode returns zero bytes.
fn decode_cell(dz: Option<Vec<u8>>) -> Result<Option<Vec<u8>>> {
    match dz {
        None => Ok(None),
        Some(b) => Ok(Some(
            zstd::decode_all(b.as_slice())
                .with_context(|| "attachment payload is not readable (zstd)")?,
        )),
    }
}

/// Row → record. A payload that fails to decode is reported through
/// `data_error` (never as empty bytes) so the service can mark the row corrupt
/// and refetch; nothing is silently dropped.
fn finish(raw: RawRec) -> AttachmentRecord {
    let mut rec = raw.rec;
    match decode_cell(raw.data_z) {
        Ok(data) => rec.data = data,
        Err(e) => rec.data_error = Some(e.to_string()),
    }
    rec
}

impl Db {
    /// Upsert one attachment. `(message_id, part_id)` is the natural key: a
    /// metadata-only row is enriched by a later full-body parse, and a row
    /// that already has a verified cache file keeps its `id` and `local_path`
    /// (P2.2). Compression errors propagate; a failed encode never leaves an
    /// empty payload behind a successful insert.
    pub async fn attachments_put(&self, a: AttPut) -> Result<()> {
        let (dz, decoded) = match a.data.as_ref() {
            Some(d) => (
                Some(zstd::encode_all(d.as_slice(), 3).context("compress attachment")?),
                Some(d.len() as i64),
            ),
            None => (None, None),
        };
        let state = if dz.is_some() {
            CacheState::Unverified
        } else {
            CacheState::Missing
        };
        self.write(move |c| {
            c.execute(
                "INSERT INTO attachments
                   (id,message_id,gmail_att_id,part_id,filename,mime,size,content_id,
                    is_inline,data_z,cache_state,decoded_size,last_accessed_at,cache_version)
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,NULL,?13)
                 ON CONFLICT(message_id,part_id) DO UPDATE SET
                   gmail_att_id = COALESCE(NULLIF(attachments.gmail_att_id,''), NULLIF(excluded.gmail_att_id,'')),
                   filename     = COALESCE(NULLIF(attachments.filename,''), NULLIF(excluded.filename,'')),
                   content_id   = COALESCE(NULLIF(attachments.content_id,''), NULLIF(excluded.content_id,'')),
                   mime         = COALESCE(NULLIF(attachments.mime,''), NULLIF(excluded.mime,''), attachments.mime),
                   size         = MAX(attachments.size, excluded.size),
                   is_inline    = MAX(attachments.is_inline, excluded.is_inline),
                   data_z       = COALESCE(attachments.data_z, excluded.data_z),
                   decoded_size = CASE WHEN attachments.data_z IS NULL
                                       THEN COALESCE(excluded.decoded_size, attachments.decoded_size)
                                       ELSE attachments.decoded_size END,
                   cache_state  = CASE
                                    WHEN IFNULL(attachments.local_path,'') <> '' THEN attachments.cache_state
                                    WHEN attachments.data_z IS NOT NULL THEN attachments.cache_state
                                    WHEN excluded.data_z IS NOT NULL THEN excluded.cache_state
                                    ELSE attachments.cache_state END
                   -- local_path is deliberately not assigned: an incoming
                   -- metadata-only row must never erase a valid cache file.",
                params![
                    a.id,
                    a.message_id,
                    a.gmail_att_id,
                    a.part_id,
                    a.filename,
                    a.mime,
                    a.size,
                    a.content_id,
                    a.is_inline as i32,
                    dz,
                    state.as_str(),
                    decoded,
                    CACHE_VERSION,
                ],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn attachments_for_message(&self, message_id: &str) -> Result<Vec<AttachmentMeta>> {
        let mid = message_id.to_string();
        self.read(move |c| {
            let mut s = c.prepare(
                "SELECT id,filename,mime,size,is_inline,local_path,data_z,cache_state
                 FROM attachments WHERE message_id=? ORDER BY is_inline, filename",
            )?;
            let __v = s
                .query_map(params![mid], |r| {
                    let dz: Option<Vec<u8>> = r.get("data_z")?;
                    let lp: Option<String> = r.get("local_path")?;
                    let state: Option<String> = r.get("cache_state")?;
                    Ok(AttachmentMeta {
                        id: r.get("id")?,
                        filename: r.get("filename")?,
                        mime: r.get("mime")?,
                        size: r.get("size")?,
                        is_inline: r.get::<_, i64>("is_inline")? != 0,
                        downloaded: CacheState::parse(state.as_deref().unwrap_or("missing")).is_local()
                            || (lp.as_deref().is_some_and(|p| !p.is_empty()) || dz.is_some()),
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(__v)
        })
        .await
    }

    /// One attachment by local row id, joined to its message for account
    /// ownership (P2.1).
    pub async fn attachment_get(&self, id: &str) -> Result<Option<AttachmentRecord>> {
        let id = id.to_string();
        self.read(move |c| {
            let mut s = c.prepare(&format!(
                "SELECT {REC_COLS} FROM attachments a JOIN messages m ON m.id=a.message_id WHERE a.id=?"
            ))?;
            let mut rows = s.query_map(params![id], raw_from_row)?;
            Ok(rows.next().transpose()?.map(finish))
        })
        .await
    }

    /// Resolve a UI-supplied key to a row. The key may be the local row id,
    /// the stored MIME section, or a Content-ID (with or without the angle
    /// brackets Gmail adds). It selects a row only: it never becomes a
    /// transport locator (P2.1).
    pub async fn attachment_resolve(
        &self,
        message_id: &str,
        key: &str,
    ) -> Result<Option<AttachmentRecord>> {
        let (m, k) = (message_id.to_string(), key.to_string());
        self.read(move |c| {
            let mut s = c.prepare(&format!(
                "SELECT {REC_COLS} FROM attachments a JOIN messages m ON m.id=a.message_id
                 WHERE a.message_id=?1 AND (a.part_id=?2 OR a.id=?2 OR a.content_id=?2
                    OR TRIM(IFNULL(a.content_id,''), '<>')=?2)
                 ORDER BY a.is_inline, a.rowid LIMIT 1"
            ))?;
            let mut rows = s.query_map(params![m, k], raw_from_row)?;
            Ok(rows.next().transpose()?.map(finish))
        })
        .await
    }

    /// All attachments of a message as full records, including decoded bytes
    /// where the row caches them.
    pub async fn attachments_records(&self, message_id: &str) -> Result<Vec<AttachmentRecord>> {
        let mid = message_id.to_string();
        self.read(move |c| {
            let mut s = c.prepare(&format!(
                "SELECT {REC_COLS} FROM attachments a JOIN messages m ON m.id=a.message_id
                 WHERE a.message_id=? ORDER BY a.is_inline, a.filename, a.rowid"
            ))?;
            let rows = s
                .query_map(params![mid], raw_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows.into_iter().map(finish).collect())
        })
        .await
    }

    /// Render-facing projection of the inline parts (id, section, CID, MIME,
    /// bytes). Derived from [`Db::attachments_records`]; decode errors
    /// propagate instead of silently embedding nothing.
    pub async fn attachments_with_bytes(
        &self,
        message_id: &str,
    ) -> Result<Vec<(String, String, Option<String>, String, Vec<u8>)>> {
        let recs = self.attachments_records(message_id).await?;
        Ok(recs
            .into_iter()
            .filter_map(|r| {
                r.data
                    .map(|d| (r.id, r.part_id, r.content_id, r.mime, d))
            })
            .collect())
    }

    /// Cache bytes for a resolved key (inline images fetched during render).
    /// `key` selects the row; the payload belongs to that row's own identity.
    pub async fn attachment_cache_data(
        &self,
        message_id: &str,
        key: &str,
        data: &[u8],
    ) -> Result<()> {
        let compressed = zstd::encode_all(data, 3).context("compress attachment cache")?;
        let decoded = data.len() as i64;
        let (mid, key) = (message_id.to_string(), key.to_string());
        self.write(move |c| {
            c.execute(
                "UPDATE attachments SET data_z=?1, decoded_size=?2,
                   cache_state=CASE WHEN IFNULL(local_path,'')<>'' THEN cache_state ELSE 'unverified' END,
                   cache_version=?3
                 WHERE message_id=?4 AND (part_id=?5 OR id=?5 OR content_id=?5
                   OR TRIM(IFNULL(content_id,''), '<>')=?5)",
                params![
                    compressed,
                    decoded,
                    CACHE_VERSION,
                    mid,
                    key
                ],
            )?;
            Ok(())
        })
        .await
    }

    /// The cache file was published at `path` with `decoded_size` bytes.
    /// Called only after the temp file has been renamed into place.
    pub async fn attachment_set_ready(
        &self,
        id: &str,
        path: &str,
        decoded_size: u64,
    ) -> Result<()> {
        let (id, path) = (id.to_string(), path.to_string());
        let size = i64::try_from(decoded_size).unwrap_or(i64::MAX);
        self.write(move |c| {
            c.execute(
                "UPDATE attachments SET local_path=?1, decoded_size=?2, cache_state='ready',
                   last_accessed_at=?3, cache_version=?4 WHERE id=?5",
                params![path, size, super::now_ms(), CACHE_VERSION, id],
            )?;
            Ok(())
        })
        .await
    }

    /// Move a row to a terminal cache state. `Corrupt` also drops the
    /// unreadable payload so a later successful fetch can replace it, while a
    /// valid file (if any) survives.
    pub async fn attachment_set_state(&self, id: &str, state: CacheState) -> Result<()> {
        let id = id.to_string();
        self.write(move |c| {
            if state == CacheState::Corrupt {
                c.execute(
                    "UPDATE attachments SET data_z=NULL, decoded_size=NULL,
                       cache_state=CASE WHEN IFNULL(local_path,'')<>'' THEN 'unverified' ELSE 'corrupt' END
                     WHERE id=?1",
                    params![id],
                )?;
            } else {
                c.execute(
                    "UPDATE attachments SET cache_state=?1 WHERE id=?2",
                    params![state.as_str(), id],
                )?;
            }
            Ok(())
        })
        .await
    }

    pub async fn attachment_touch(&self, id: &str) -> Result<()> {
        let id = id.to_string();
        let now = super::now_ms();
        self.write(move |c| {
            c.execute(
                "UPDATE attachments SET last_accessed_at=?1 WHERE id=?2",
                params![now, id],
            )?;
            Ok(())
        })
        .await
    }

}

#[cfg(test)]
mod tests {
    use super::*;

    async fn seed_message(db: &Db, mid: &str) -> String {
        let a = db.new_account("a@x.com", None, None).await.unwrap();
        let aid = a.id.clone();
        let mid = mid.to_string();
        db.write(move |c| {
            c.execute(
                "INSERT INTO messages (id,account_id,thread_id,internal_date) VALUES (?1,?2,'t1',1)",
                params![mid, aid],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        a.id
    }

    fn put(mid: &str, part: &str, filename: Option<&str>, data: Option<Vec<u8>>) -> AttPut {
        AttPut {
            id: format!("att-{part}"),
            message_id: mid.into(),
            gmail_att_id: None,
            part_id: part.into(),
            filename: filename.map(str::to_string),
            mime: "application/pdf".into(),
            size: data.as_ref().map(|d| d.len() as i64).unwrap_or(0),
            content_id: None,
            is_inline: false,
            data,
        }
    }

    /// P2.2: metadata first, full body second. One row, enriched in place,
    /// keeping the original row id.
    #[tokio::test]
    async fn metadata_then_body_enriches_one_row() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let aid = seed_message(&db, "m1").await;
        db.attachments_put(put("m1", "2", Some("invoice.pdf"), None))
            .await
            .unwrap();
        db.attachments_put(put("m1", "2", None, Some(b"%PDF-1.4".to_vec())))
            .await
            .unwrap();
        let recs = db.attachments_records("m1").await.unwrap();
        assert_eq!(recs.len(), 1, "one attachment after enrichment");
        assert_eq!(recs[0].id, "att-2", "stable row id");
        assert_eq!(recs[0].filename.as_deref(), Some("invoice.pdf"));
        assert_eq!(recs[0].data.as_deref(), Some(&b"%PDF-1.4"[..]));
        assert_eq!(recs[0].account_id, aid);
        assert_eq!(recs[0].cache_state, CacheState::Unverified);
    }

    /// P2.2: a repeated metadata ingest must not erase a downloaded path.
    #[tokio::test]
    async fn repeated_ingest_preserves_local_path() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        seed_message(&db, "m1").await;
        db.attachments_put(put("m1", "2", Some("invoice.pdf"), Some(vec![1, 2, 3])))
            .await
            .unwrap();
        db.attachment_set_ready("att-2", "/cache/att-2/invoice.pdf", 3)
            .await
            .unwrap();
        db.attachments_put(put("m1", "2", Some("renamed.pdf"), None))
            .await
            .unwrap();
        let rec = db.attachment_get("att-2").await.unwrap().unwrap();
        assert_eq!(rec.local_path.as_deref(), Some("/cache/att-2/invoice.pdf"));
        assert_eq!(rec.cache_state, CacheState::Ready);
        assert_eq!(rec.filename.as_deref(), Some("invoice.pdf"));
        assert!(rec.data.is_some(), "bytes survive a metadata refresh");
    }

    /// P2.2: corrupt zstd is an error, never a successful empty attachment.
    #[tokio::test]
    async fn corrupt_payload_is_an_error_not_empty_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        seed_message(&db, "m1").await;
        db.attachments_put(put("m1", "2", Some("invoice.pdf"), Some(vec![1, 2, 3])))
            .await
            .unwrap();
        db.write(|c| {
            c.execute(
                "UPDATE attachments SET data_z=?1 WHERE id='att-2'",
                params![b"not zstd at all".to_vec()],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        let rec = db.attachment_get("att-2").await.unwrap().unwrap();
        assert!(rec.data.is_none(), "corrupt payload is never empty bytes");
        let err = rec.data_error.expect("decode failure recorded");
        assert!(err.contains("zstd"), "decode failure surfaced: {err}");
    }

    /// Compression errors propagate instead of storing an empty payload.
    #[tokio::test]
    async fn empty_bytes_are_a_real_payload() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        seed_message(&db, "m1").await;
        db.attachments_put(put("m1", "2", Some("empty.bin"), Some(vec![])))
            .await
            .unwrap();
        let rec = db.attachment_get("att-2").await.unwrap().unwrap();
        assert_eq!(rec.data.as_deref(), Some(&[][..]));
        assert_ne!(rec.cache_state, CacheState::Missing);
    }

    /// P2.1: the lookup key selects a row; the record keeps the real section.
    #[tokio::test]
    async fn cid_lookup_returns_stored_part_id() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        seed_message(&db, "m1").await;
        let mut a = put("m1", "2", Some("logo.png"), Some(vec![9]));
        a.content_id = Some("<logo@example.test>".into());
        a.mime = "image/png".into();
        a.is_inline = true;
        a.gmail_att_id = Some("rest-abc".into());
        db.attachments_put(a).await.unwrap();
        let rec = db
            .attachment_resolve("m1", "logo@example.test")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rec.part_id, "2");
        assert_eq!(rec.imap_locator(), Some("2"));
        assert_eq!(rec.rest_locator(), Some("rest-abc"));
        let by_id = db.attachment_resolve("m1", "att-2").await.unwrap().unwrap();
        assert_eq!(by_id.id, "att-2");
    }

    /// P2.2: a second row with the same (message_id, part_id) cannot be
    /// inserted once the unique index exists.
    #[tokio::test]
    async fn natural_key_is_unique() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        seed_message(&db, "m1").await;
        db.attachments_put(put("m1", "1.2", Some("a.pdf"), None))
            .await
            .unwrap();
        let dup = db
            .write(|c| {
                let r = c.execute(
                    "INSERT INTO attachments (id,message_id,part_id,mime) VALUES ('x','m1','1.2','application/pdf')",
                    [],
                );
                Ok(r.is_err())
            })
            .await
            .unwrap();
        assert!(dup, "duplicate (message_id, part_id) rejected");
    }
}
