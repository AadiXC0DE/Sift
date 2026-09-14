//! `sift-att://` resolution (P2.1).
//!
//! The scheme's `<account-id>/<message-id>/<key>` is a *lookup* key: a row id,
//! the stored MIME section, or a Content-ID, scoped to one account.
//! Resolution goes through the attachment service, which selects the row and
//! then derives the transport locator from the row's own stored fields — a CID
//! such as `logo@example.test` must fetch the row's numeric section, never
//! `BODY.PEEK[logo@example.test]`.

use crate::db::Db;
use crate::dto::MessageRef;
use crate::provider::Provider;

/// Bytes + MIME for a scheme key. Callers that need typed cache metadata use
/// [`crate::attachments::ensure_local`] instead.
pub async fn resolve_attachment(
    db: &Db,
    provider: &dyn Provider,
    message: &MessageRef,
    key: &str,
) -> Result<(Vec<u8>, String), crate::errors::SiftError> {
    crate::attachments::resolve_bytes(db, provider, message, key).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::attachments::AttPut;
    use crate::provider::gmail::api::GmailApiProvider;
    use crate::provider::gmail::client::GmailClient;

    async fn db_with_inline(content_id: Option<&str>) -> (Db, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let a = db.new_account("a@x.com", None, None).await.unwrap();
        let aid = a.id;
        db.write({
            let aid = aid.clone();
            move |c| {
                c.execute(
                    "INSERT INTO messages (id,account_id,thread_id,internal_date) VALUES ('m1',?,'t1',1)",
                    rusqlite::params![aid],
                )?;
                Ok(())
            }
        })
        .await
        .unwrap();
        db.attachments_put(AttPut {
            id: "att1".into(),
            account_id: aid.clone(),
            message_id: "m1".into(),
            gmail_att_id: None,
            part_id: "2".into(),
            filename: Some("logo.png".into()),
            mime: "image/png".into(),
            size: 3,
            content_id: content_id.map(str::to_string),
            is_inline: true,
            data: Some(vec![1, 2, 3]),
        })
        .await
        .unwrap();
        (db, aid, dir)
    }

    #[tokio::test]
    async fn p5_t01_uri_serves_inline() {
        let (db, aid, _dir) = db_with_inline(None).await;
        let c = GmailApiProvider::new("a".into(), GmailClient::new("t".into()));
        let r = MessageRef::new(aid.clone(), "m1");
        let (b, mime) = resolve_attachment(&db, &c, &r, "2").await.unwrap();
        assert_eq!(b, vec![1, 2, 3]);
        assert_eq!(mime, "image/png");
        assert!(resolve_attachment(&db, &c, &r, "nope").await.is_err());
    }

    /// P2.1: a CID key resolves to the row's numeric section. Nothing here
    /// reaches the network (`data` is cached), and the resolver never treats
    /// the CID as a locator.
    #[tokio::test]
    async fn cid_key_is_not_a_locator() {
        let (db, aid, _dir) = db_with_inline(Some("<logo@example.test>")).await;
        let c = GmailApiProvider::new("a".into(), GmailClient::new("t".into()));
        let r = MessageRef::new(aid.clone(), "m1");
        let (b, _) = resolve_attachment(&db, &c, &r, "logo@example.test")
            .await
            .unwrap();
        assert_eq!(b, vec![1, 2, 3]);
        let rec = db
            .attachment_resolve(&r, "logo@example.test")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rec.part_id, "2");
        assert_eq!(
            rec.locator_for(crate::provider::ProviderKind::GmailImap),
            Some("2")
        );
        // No REST attachment id on this row: the REST transport has no locator
        // for it, which is an error rather than a fabricated request.
        assert_eq!(
            rec.locator_for(crate::provider::ProviderKind::GmailApi),
            None
        );
    }

    /// P4.2: a scheme key from another account never resolves.
    #[tokio::test]
    async fn cross_account_key_is_not_found() {
        let (db, _aid, _dir) = db_with_inline(None).await;
        let c = GmailApiProvider::new("a".into(), GmailClient::new("t".into()));
        let other = MessageRef::new("b9", "m1");
        assert!(resolve_attachment(&db, &c, &other, "2").await.is_err());
    }
}
