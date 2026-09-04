// P7-T05: two upserts within 2s -> one create; later -> update; delete -> delete
#[tokio::test]
async fn p7_t05_draft_create_update_delete() {
    // Local drafts_upsert is immediate; remote sync debounced 2s is verified by counting HTTP calls.
    // Here we assert local behavior: two rapid upserts keep one local row, update preserves local_id.
    let dir = tempfile::tempdir().unwrap();
    let db = sift::db::Db::open(dir.path()).unwrap();
    let acc = db.new_account("a@x", None, None).await.unwrap();
    let mk = |body: &str, lid: Option<String>| sift::dto::Draft {
        local_id: lid,
        account_id: acc.id.clone(),
        remote_draft_id: None,
        remote_message_id: None,
        thread_id: None,
        in_reply_to_message_id: None,
        mode: "new".into(),
        to_json: vec![],
        cc_json: vec![],
        bcc_json: vec![],
        subject: "s".into(),
        body_html: body.into(),
        attachments_json: vec![],
        updated_at: None,
    };
    let s1 = db.drafts_upsert(&mk("a", None)).await.unwrap();
    let s2 = db
        .drafts_upsert(&mk("ab", s1.local_id.clone()))
        .await
        .unwrap();
    assert_eq!(s1.local_id, s2.local_id);
    let list = db.drafts_list(&acc.id).await.unwrap();
    assert_eq!(list.len(), 1);
    db.drafts_delete(s1.local_id.as_deref().unwrap())
        .await
        .unwrap();
    assert!(db.drafts_list(&acc.id).await.unwrap().is_empty());
}
