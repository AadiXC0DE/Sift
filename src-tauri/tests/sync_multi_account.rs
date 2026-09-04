// P3-T20: two accounts independent; removing A cancels only A
#[tokio::test]
async fn p3_t20_independent_accounts() {
    let dir = tempfile::tempdir().unwrap();
    let db = sift::db::Db::open(dir.path()).unwrap();
    let a = db.new_account("a@x", None, None).await.unwrap();
    let b = db.new_account("b@x", None, None).await.unwrap();
    db.messages_upsert(sift::db::messages::MsgUpsert {
        id: "ma".into(),
        account_id: a.id.clone(),
        thread_id: "ta".into(),
        internal_date: 1,
        label_ids: vec![],
        ..Default::default()
    })
    .await
    .unwrap();
    db.messages_upsert(sift::db::messages::MsgUpsert {
        id: "mb".into(),
        account_id: b.id.clone(),
        thread_id: "tb".into(),
        internal_date: 1,
        label_ids: vec![],
        ..Default::default()
    })
    .await
    .unwrap();
    db.accounts_remove(&a.id).await.unwrap();
    // b intact
    let n: i64 = db
        .read(move |c| Ok(c.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?))
        .await
        .unwrap();
    assert_eq!(n, 1);
}
