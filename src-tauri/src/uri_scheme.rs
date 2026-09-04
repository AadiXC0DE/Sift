use crate::db::Db;
use crate::provider::Provider;

fn bad_id(s: &str) -> bool {
    s.is_empty() || s.contains("..") || s.contains('/') || s.contains('\\')
}

/// Resolve sift-att message/part to bytes + mime.
pub async fn resolve_attachment(
    db: &Db,
    provider: &dyn Provider,
    message_id: &str,
    part_id: &str,
) -> Result<(Vec<u8>, String), crate::errors::SiftError> {
    if bad_id(message_id) || bad_id(part_id) {
        return Err(crate::errors::SiftError::app(
            "bad_id",
            "invalid attachment id",
            false,
        ));
    }
    let rec = db
        .attachment_by_msg_part(message_id, part_id)
        .await
        .map_err(|e| crate::errors::SiftError::app("db", e.to_string(), false))?;
    let Some((_row_id, mime, att_id_opt, _mime2, data_opt, local_opt, _fname)) = rec else {
        return Err(crate::errors::SiftError::NotFound("attachment".into()));
    };
    if let Some(b) = data_opt {
        return Ok((b, mime));
    }
    if let Some(p) = local_opt.clone() {
        if std::path::Path::new(&p).exists() {
            let b = tokio::fs::read(&p)
                .await
                .map_err(crate::errors::SiftError::from)?;
            return Ok((b, mime));
        }
    }
    if let Some(att_id) = att_id_opt {
        // Transport locator: Gmail attachmentId, or the IMAP section path
        // stored in part_id (gmail_att_id is NULL for IMAP rows).
        let locator = if att_id.is_empty() {
            part_id
        } else {
            att_id.as_str()
        };
        let bytes = provider.fetch_attachment(message_id, locator).await?;
        if !bytes.is_empty() {
            return Ok((bytes, mime));
        }
    }
    Err(crate::errors::SiftError::NotFound("attachment".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn p5_t01_uri_serves_inline() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let a = db.new_account("a@x.com", None, None).await.unwrap();
        let aid = a.id.clone();
        db.write(move |c| {
      c.execute("INSERT INTO messages (id,account_id,thread_id,internal_date) VALUES ('m1',?, 't1', 1)", rusqlite::params![aid])?;
      Ok(())
    }).await.unwrap();
        db.attachments_put(crate::db::attachments::AttPut {
            id: "att1".into(),
            message_id: "m1".into(),
            gmail_att_id: None,
            part_id: "p1".into(),
            filename: Some("a.png".into()),
            mime: "image/png".into(),
            size: 3,
            content_id: None,
            is_inline: true,
            data: Some(vec![1, 2, 3]),
        })
        .await
        .unwrap();
        let c = crate::provider::gmail::api::GmailApiProvider::new(
            "a".into(),
            crate::provider::gmail::client::GmailClient::new("t".into()),
        );
        let (b, mime) = resolve_attachment(&db, &c, "m1", "p1").await.unwrap();
        assert_eq!(b, vec![1, 2, 3]);
        assert_eq!(mime, "image/png");
        assert!(resolve_attachment(&db, &c, "m1", "nope").await.is_err());
    }
}
