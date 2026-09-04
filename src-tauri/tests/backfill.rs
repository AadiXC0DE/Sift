// P3-T15: backfill pauses while foreground in flight
#[tokio::test]
async fn p3_t15_backfill_yields_to_foreground() {
    let dir = tempfile::tempdir().unwrap();
    let db = sift::db::Db::open(dir.path()).unwrap();
    let acc = db.new_account("a@x", None, None).await.unwrap();
    db.messages_upsert(sift::db::messages::MsgUpsert {
        id: "m1".into(),
        account_id: acc.id.clone(),
        thread_id: "t1".into(),
        internal_date: sift::db::now_ms(),
        label_ids: vec![],
        ..Default::default()
    })
    .await
    .unwrap();
    let gate = sift::sync::backfill::BackfillGate::new();
    let _fg = gate.enter();
    let provider = sift::provider::gmail::api::GmailApiProvider::new(
        acc.id.clone(),
        sift::provider::gmail::client::GmailClient::new("t".into()),
    );
    let sink = sift::provider::DbSink::new(db.clone());
    let n = sift::sync::backfill::run_backfill_once(&sink, &acc.id, &provider, &gate, 365 * 2)
        .await
        .unwrap();
    assert_eq!(n, 0); // paused
}
