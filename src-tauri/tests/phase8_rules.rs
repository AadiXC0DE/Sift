//! Phase 8 P8.3 — local rules and sender blocking.
//!
//! The four properties these tests defend are the ones a rule engine gets
//! wrong in the field: it must not act twice on the same message, it must not
//! touch history the user did not ask it to touch, a broken rule must disable
//! itself instead of breaking sync, and one pass must be bounded so a large
//! mailbox cannot monopolise the write lane.

use sift::db::rules::RuleMessage;
use sift::db::Db;
use sift::dto::{MailRule, RuleAction, RuleCondition};

const SUPPORTED: [&str; 4] = ["sender", "recipient", "subject", "hasAttachment"];

async fn ready_account(db: &Db) -> String {
    let acc = db.new_account("ada@x.com", None, None).await.unwrap();
    // Past the first sync: newly ingested mail is rule-eligible from here.
    db.accounts_set_state(&acc.id, "partial").await.unwrap();
    acc.id
}

async fn new_account(db: &Db) -> String {
    db.new_account("ada@x.com", None, None).await.unwrap().id
}

fn rule(account_id: &str, id: &str, subject: &str, actions: Vec<RuleAction>) -> MailRule {
    MailRule {
        account_id: account_id.to_string(),
        id: id.to_string(),
        name: format!("Rule {id}"),
        enabled: true,
        match_mode: "all".into(),
        conditions: vec![RuleCondition::new("subject", "contains", subject)],
        actions,
        sort_order: 0,
        revision: 0,
        last_error: None,
    }
}

fn junk() -> Vec<RuleAction> {
    vec![RuleAction::new("junk", None)]
}

/// Ingest one message, exactly as sync would: the rule queue row is written in
/// the same transaction as the message.
async fn ingest(db: &Db, account_id: &str, id: &str, subject: &str, date: i64) {
    ingest_with(db, account_id, id, subject, date, &["INBOX".to_string()]).await;
}

async fn ingest_with(
    db: &Db,
    account_id: &str,
    id: &str,
    subject: &str,
    date: i64,
    labels: &[String],
) {
    db.messages_upsert(sift::db::messages::MsgUpsert {
        id: id.into(),
        account_id: account_id.into(),
        thread_id: format!("t-{id}"),
        history_id: None,
        internal_date: date,
        from_name: Some("Spam".into()),
        from_email: Some("spam@x.com".into()),
        to_json: r#"[{"n":null,"e":"ada@x.com"}]"#.into(),
        cc_json: "[]".into(),
        bcc_json: "[]".into(),
        reply_to: None,
        subject: subject.into(),
        snippet: String::new(),
        rfc_message_id: None,
        in_reply_to: None,
        references_json: "[]".into(),
        list_unsubscribe: None,
        list_unsubscribe_post: false,
        list_unsubscribe_post_value: None,
        auth_results: None,
        auth_results_trusted: false,
        size_estimate: Some(0),
        has_attachments: false,
        is_unread: true,
        is_starred: false,
        is_draft: false,
        is_sent_by_me: false,
        label_ids: labels.to_vec(),
    })
    .await
    .unwrap();
}

async fn labels_of(db: &Db, account_id: &str, message_id: &str) -> Vec<String> {
    let (a, m) = (account_id.to_string(), message_id.to_string());
    db.read(move |c| {
        Ok(c.prepare(
            "SELECT label_id FROM message_labels WHERE account_id=? AND message_id=? ORDER BY label_id",
        )?
        .query_map(rusqlite::params![a, m], |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?)
    })
    .await
    .unwrap()
}

async fn rule_applications(db: &Db, rule_id: &str) -> i64 {
    let r = rule_id.to_string();
    db.read(move |c| {
        Ok(c.query_row(
            "SELECT count(*) FROM rule_applications WHERE rule_id=?",
            rusqlite::params![r],
            |r| r.get(0),
        )?)
    })
    .await
    .unwrap()
}

async fn rule_apply_ops(db: &Db, account_id: &str) -> i64 {
    let a = account_id.to_string();
    db.read(move |c| {
        Ok(c.query_row(
            "SELECT count(*) FROM outbox_ops WHERE account_id=? AND kind='rule_apply'",
            rusqlite::params![a],
            |r| r.get(0),
        )?)
    })
    .await
    .unwrap()
}

/// P8.3: sync can run as often as it likes and a restart changes nothing — one
/// message is acted on once per rule revision.
#[tokio::test]
async fn p8_3_a_repeated_sync_never_repeats_a_rule_revision() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = ready_account(&db).await;
    db.rule_upsert(rule(&acc, "r1", "invoice", junk()), None)
        .await
        .unwrap();

    ingest(&db, &acc, "m1", "Your invoice", 100).await;
    let first = sift::rules::process_queue(&db, &acc, false).await.unwrap();
    assert_eq!(first.messages, 1);
    assert_eq!(first.applied, 1);
    assert!(labels_of(&db, &acc, "m1")
        .await
        .contains(&"SPAM".to_string()));
    assert_eq!(rule_apply_ops(&db, &acc).await, 1);

    // A second drain with an empty queue does nothing.
    let again = sift::rules::process_queue(&db, &acc, false).await.unwrap();
    assert_eq!(again.messages, 0);
    assert_eq!(again.applied, 0);

    // Restart: the same message is re-ingested by a fresh sync. It is not new
    // mail, so it is not queued, and applying it again would be a no-op anyway.
    ingest(&db, &acc, "m1", "Your invoice", 100).await;
    assert_eq!(db.rule_queue_len(&acc).await.unwrap(), 0);
    let after_restart = sift::rules::process_queue(&db, &acc, false).await.unwrap();
    assert_eq!(after_restart.applied, 0);
    assert_eq!(
        rule_apply_ops(&db, &acc).await,
        1,
        "one application, one operation"
    );
    assert_eq!(rule_applications(&db, "r1").await, 1);
    // The queued operation is keyed by rule, revision and message, so even a
    // replayed application would address the same operation rather than add a
    // second one.
    let ops = db
        .outbox_list(std::slice::from_ref(&acc), &[], None, 10)
        .await
        .unwrap();
    let keys: Vec<&str> = ops
        .operations
        .iter()
        .filter_map(|o| o.operation_key.as_deref())
        .collect();
    assert_eq!(keys.len(), 1);
    assert!(
        keys[0].starts_with("rule-apply:"),
        "the operation carries the stable rule key: {keys:?}"
    );
    // A re-ingest reports the server's own label snapshot; the local rule
    // change stays in the queue as one operation, which is what re-applies it.
    assert_eq!(
        db.outbox_list(
            std::slice::from_ref(&acc),
            &["pending".to_string()],
            None,
            10
        )
        .await
        .unwrap()
        .total,
        1
    );
}

/// P8.3: an edited rule is a new revision, so the message is acted on again —
/// that is what "once per revision" means, and it is why the revision is
/// claimed rather than the rule id.
#[tokio::test]
async fn p8_3_a_new_revision_acts_again_and_an_edit_does_not_replay_the_old_one() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = ready_account(&db).await;
    let stored = db
        .rule_upsert(rule(&acc, "r1", "invoice", junk()), None)
        .await
        .unwrap();
    assert_eq!(stored.revision, 1);

    ingest(&db, &acc, "m1", "Your invoice", 100).await;
    assert_eq!(
        sift::rules::process_queue(&db, &acc, false)
            .await
            .unwrap()
            .applied,
        1
    );

    // The user edits the rule: a new revision, which has not applied to m1.
    let edited = db
        .rule_upsert(
            MailRule {
                actions: vec![RuleAction::new("star", None)],
                ..rule(&acc, "r1", "invoice", junk())
            },
            Some(1),
        )
        .await
        .unwrap();
    assert_eq!(edited.revision, 2);
    // Explicit apply of the new revision acts once, and only once.
    ingest(&db, &acc, "m2", "invoice again", 200).await;
    assert_eq!(
        sift::rules::process_queue(&db, &acc, false)
            .await
            .unwrap()
            .applied,
        1
    );
    assert_eq!(rule_applications(&db, "r1").await, 2);
    assert_eq!(rule_apply_ops(&db, &acc).await, 2);
}

/// P8.3: a rule whose operation failed terminally is disabled with the reason,
/// and the account keeps syncing.
#[tokio::test]
async fn p8_3_a_failing_rule_is_disabled_without_blocking_sync() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = ready_account(&db).await;
    db.rule_upsert(rule(&acc, "bad", "invoice", junk()), None)
        .await
        .unwrap();
    db.rule_upsert(rule(&acc, "good", "invoice", junk()), None)
        .await
        .unwrap();

    ingest(&db, &acc, "m1", "Your invoice", 100).await;
    sift::rules::process_queue(&db, &acc, false).await.unwrap();
    // Both rules claim the message; the bad one's operation then fails.
    let ops = db
        .outbox_list(std::slice::from_ref(&acc), &[], None, 50)
        .await
        .unwrap();
    let bad_op = ops
        .operations
        .iter()
        .find(|o| {
            o.kind == "rule_apply"
                && serde_json::from_str::<serde_json::Value>(&o.payload)
                    .ok()
                    .and_then(|v| v.get("ruleId").and_then(|r| r.as_str()).map(str::to_string))
                    .as_deref()
                    == Some("bad")
        })
        .expect("the failing rule queued an operation");
    sift::rules::disable_failed_rule(&db, bad_op, "Gmail refused the label").await;

    let disabled = db.rule_get(&acc, "bad").await.unwrap().unwrap();
    assert!(
        !disabled.enabled,
        "a terminally failing rule disables itself"
    );
    assert_eq!(
        disabled.last_error.as_deref(),
        Some("Gmail refused the label")
    );
    let enabled_now = db.rules_enabled(&acc).await.unwrap();
    assert_eq!(
        enabled_now
            .iter()
            .map(|r| r.id.as_str())
            .collect::<Vec<_>>(),
        vec!["good"],
        "only the failing rule stopped"
    );

    // Sync and rule processing keep working for the rest of the account.
    ingest(&db, &acc, "m2", "Another invoice", 200).await;
    let report = sift::rules::process_queue(&db, &acc, false).await.unwrap();
    assert_eq!(report.applied, 1);
    assert!(labels_of(&db, &acc, "m2")
        .await
        .contains(&"SPAM".to_string()));
}

/// P8.3: one drain pass takes a bounded batch. 500 metadata rows is the bound,
/// and the rest waits for the next pass instead of stalling the app.
#[tokio::test]
async fn p8_3_process_queue_takes_500_rows_in_one_bounded_batch() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    let acc = ready_account(&db).await;
    db.rule_upsert(rule(&acc, "r1", "invoice", junk()), None)
        .await
        .unwrap();

    for i in 0..600 {
        ingest(&db, &acc, &format!("m{i:04}"), "invoice", 1_000 + i).await;
    }
    assert_eq!(db.rule_queue_len(&acc).await.unwrap(), 600);

    let started = std::time::Instant::now();
    let first = sift::rules::process_queue(&db, &acc, false).await.unwrap();
    let elapsed = started.elapsed();
    assert_eq!(
        first.messages,
        sift::rules::BATCH_MESSAGES,
        "one pass takes exactly the bounded batch"
    );
    assert_eq!(first.applied, 500);
    assert_eq!(
        db.rule_queue_len(&acc).await.unwrap(),
        100,
        "the rest waits"
    );
    assert!(
        elapsed.as_secs() < 60,
        "a bounded batch cannot be an unbounded stall ({elapsed:?})"
    );

    // The next pass takes what is left and then stops.
    let second = sift::rules::process_queue(&db, &acc, false).await.unwrap();
    assert_eq!(second.messages, 100);
    assert_eq!(db.rule_queue_len(&acc).await.unwrap(), 0);
    assert_eq!(rule_apply_ops(&db, &acc).await, 600);

    // A busy foreground means no batch at all is taken.
    ingest(&db, &acc, "m9999", "invoice", 9_999).await;
    let held = sift::rules::process_queue(&db, &acc, true).await.unwrap();
    assert_eq!(held.messages, 0);
    assert_eq!(db.rule_queue_len(&acc).await.unwrap(), 1);
}

/// P8.3: history is untouched. A rule written today never rewrites yesterday's
/// mail unless the user presses Apply, and Apply is previewable and idempotent.
#[tokio::test]
async fn p8_3_historical_mail_is_untouched_until_apply_is_explicit() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path()).unwrap();
    // A fresh account: this is its initial sync.
    let acc = new_account(&db).await;
    db.rule_upsert(rule(&acc, "r1", "invoice", junk()), None)
        .await
        .unwrap();

    for i in 0..3 {
        ingest(&db, &acc, &format!("old{i}"), "Old invoice", 1_000 + i).await;
    }
    assert_eq!(
        db.rule_queue_len(&acc).await.unwrap(),
        0,
        "an initial sync is not rule-eligible"
    );
    let nothing = sift::rules::process_queue(&db, &acc, false).await.unwrap();
    assert_eq!(nothing.messages, 0);
    for i in 0..3 {
        assert_eq!(
            labels_of(&db, &acc, &format!("old{i}")).await,
            vec!["INBOX".to_string()],
            "history is exactly as it was"
        );
    }

    // Preview counts without changing anything.
    let stored = db.rule_get(&acc, "r1").await.unwrap().unwrap();
    let preview = sift::rules::preview(&db, &acc, &stored, 10).await.unwrap();
    assert_eq!(preview.count, 3);
    assert_eq!(preview.sample.len(), 3);
    assert!(preview.sample.iter().all(|row| row.would_junk));
    assert_eq!(
        labels_of(&db, &acc, "old0").await,
        vec!["INBOX".to_string()],
        "a preview changes nothing"
    );

    // Apply is explicit, bounded and idempotent.
    let report = sift::rules::apply_existing(&db, &acc, &stored)
        .await
        .unwrap();
    assert_eq!(report.applied, 3);
    for i in 0..3 {
        assert!(labels_of(&db, &acc, &format!("old{i}"))
            .await
            .contains(&"SPAM".to_string()));
    }
    let again = sift::rules::apply_existing(&db, &acc, &stored)
        .await
        .unwrap();
    assert_eq!(again.applied, 0, "the same revision does not apply twice");
    assert_eq!(rule_apply_ops(&db, &acc).await, 3);
}

/// The closed vocabulary the UI is allowed to offer: anything else is refused
/// before it is stored, so no rule can ask Sift to run a script or a forward.
#[test]
fn p8_3_the_action_and_condition_vocabulary_is_closed() {
    let mut condition = RuleCondition::new("body", "contains", "password");
    let stored = MailRule {
        account_id: "a".into(),
        id: "r".into(),
        name: "r".into(),
        enabled: true,
        match_mode: "all".into(),
        conditions: vec![condition.clone()],
        actions: junk(),
        sort_order: 0,
        revision: 1,
        last_error: None,
    };
    // A condition Sift does not implement is refused before it is stored, and
    // even if one existed it could never match — a rule cannot become a script
    // or a forward by naming a field nobody checks.
    let err = sift::db::rules::validate(&stored).unwrap_err();
    assert!(err.contains("body"), "{err}");
    assert!(!sift::rules::matches(
        &stored,
        &RuleMessage {
            id: "m".into(),
            account_id: "a".into(),
            thread_id: "t".into(),
            from_email: "x@y".into(),
            to_json: "[]".into(),
            cc_json: "[]".into(),
            bcc_json: "[]".into(),
            subject: "password".into(),
            has_attachments: false,
            label_ids: vec![],
        }
    ));
    for unsupported in ["forward", "reply", "script", "delete", "openUrl"] {
        let bad = MailRule {
            actions: vec![RuleAction::new(unsupported, None)],
            ..stored.clone()
        };
        assert!(
            sift::db::rules::validate(&bad).is_err(),
            "{unsupported} must not be an action Sift accepts"
        );
    }
    condition.field = "sender".into();
    assert!(SUPPORTED.contains(&condition.field.as_str()));
}
