//! The local rule engine (P8.3).
//!
//! Rules are small, closed and local. There is no script, no regex, no
//! auto-reply, no forward, no permanent delete and no arbitrary URL action —
//! the vocabulary in [`crate::db::rules`] is the whole feature, and a rule that
//! names anything else is refused before it is stored.
//!
//! Two properties matter more than the matching itself:
//!
//! * **Once per revision per message.** Every application writes a row in
//!   `rule_applications` keyed by `(rule_id, revision, account_id,
//!   message_id)` inside the same transaction that queues the provider work.
//!   Sync can run as often as it likes; the same revision cannot act twice on
//!   the same message.
//! * **The original metadata decides.** All matching rules are evaluated
//!   against the message as it was ingested. A rule that adds a label can
//!   therefore never make a later rule match — or un-match — because of its own
//!   change, and rules never re-trigger from each other.
//!
//! Evaluation of newly ingested mail happens *after* the metadata commit (the
//! queue row is written in the ingest transaction and drained later) and
//! outside the first-page path, in bounded batches that yield to foreground
//! work.

use crate::db::rules::{diff_for, RuleMessage};
use crate::db::Db;
use crate::dto::{MailRule, RuleAction, RuleCondition, RulePreview, RulePreviewRow};
use crate::errors::SiftError;
use rusqlite::params;

/// How many metadata rows one evaluation batch reads. The bound is what keeps
/// a first sync of a huge mailbox from monopolising the write lane.
pub const BATCH_MESSAGES: i64 = 500;

/// How long one batch may spend before the worker yields to foreground work.
/// Measured on this machine (Apple M1, release-shaped batch of 500 metadata
/// rows): the pure matching pass costs well under 50 ms, so 250 ms leaves room
/// for the SQLite writes without ever being the thing the user waits for.
pub const BATCH_BUDGET_MS: i64 = 250;

/// How many existing messages an explicit "Apply to existing mail" run may
/// touch in one pass.
pub const APPLY_EXISTING_LIMIT: i64 = 5_000;

fn db_error(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

/// Does one message match a rule's conditions? Pure, so it is tested directly.
pub fn matches(rule: &MailRule, msg: &RuleMessage) -> bool {
    if rule.conditions.is_empty() {
        return false;
    }
    let one = |c: &RuleCondition| condition_matches(c, msg);
    match rule.match_mode.as_str() {
        "any" => rule.conditions.iter().any(one),
        _ => rule.conditions.iter().all(one),
    }
}

fn condition_matches(c: &RuleCondition, msg: &RuleMessage) -> bool {
    let needle = c.value.trim().to_lowercase();
    match c.field.as_str() {
        "sender" => address_matches(&msg.from_email, &c.op, &needle),
        "recipient" => recipient_addresses(msg)
            .iter()
            .any(|a| address_matches(a, &c.op, &needle)),
        "subject" => c.op == "contains" && msg.subject.to_lowercase().contains(&needle),
        "hasAttachment" => c.op == "isTrue" && msg.has_attachments,
        _ => false,
    }
}

/// Address matching is intentionally blunt: case-insensitive substring,
/// exact address, or the domain after `@`.
fn address_matches(address: &str, op: &str, needle: &str) -> bool {
    let address = address.trim().to_lowercase();
    if address.is_empty() || needle.is_empty() {
        return false;
    }
    match op {
        "is" => address == needle,
        "domain" => address
            .rsplit_once('@')
            .map(|(_, domain)| domain == needle)
            .unwrap_or(false),
        _ => address.contains(needle),
    }
}

fn recipient_addresses(msg: &RuleMessage) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for raw in [&msg.to_json, &msg.cc_json, &msg.bcc_json] {
        let parsed: Vec<serde_json::Value> = serde_json::from_str(raw).unwrap_or_default();
        for v in parsed {
            let email = v
                .get("e")
                .or_else(|| v.get("email"))
                .and_then(|e| e.as_str())
                .unwrap_or_default();
            if !email.is_empty() {
                out.push(email.to_string());
            }
        }
    }
    out
}

/// A deterministic digest of the message ids an application covers.
///
/// Not `DefaultHasher`: `RandomState` is seeded per process, so an operation
/// key built from it would differ after a restart and the idempotency the key
/// exists to provide would silently disappear.
fn digest(parts: &[&str]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for part in parts {
        for b in part.as_bytes() {
            hash ^= *b as u64;
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
        hash ^= 0xff;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")
}

/// The operation key of one rule application. Same rule revision, same
/// messages, same key — so a replay produces one operation, not two.
pub fn rule_operation_key(
    account_id: &str,
    rule_id: &str,
    revision: i64,
    message_ids: &[String],
) -> String {
    let joined = message_ids.join(",");
    let d = digest(&[account_id, rule_id, &revision.to_string(), &joined]);
    format!("rule-apply:{account_id}:{rule_id}:{revision}:{d}")
}

/// Deterministic id for the "Block sender in Sift" rule, so blocking the same
/// address twice edits one rule instead of stacking two.
pub fn block_rule_id(account_id: &str, email: &str) -> String {
    format!("block-{}", digest(&[account_id, email]))
}

/// One rule's actions turned into the payload the action service and the
/// outbox already understand.
fn payload_for(
    account_id: &str,
    rule: &MailRule,
    ids: &[String],
    add: &[String],
    remove: &[String],
) -> String {
    serde_json::json!({
        "ids": ids,
        "add": add,
        "remove": remove,
        "ruleId": rule.id,
        "ruleRevision": rule.revision,
        "accountId": account_id,
    })
    .to_string()
}

/// The result of one evaluation pass.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplyReport {
    pub messages: i64,
    pub applied: i64,
    pub skipped: i64,
    /// True when the batch budget ran out and the caller should yield.
    pub yielded: bool,
}

/// Apply every matching rule to one message, inside the caller's transaction.
///
/// Returns how many rules actually changed something.
fn apply_rules_conn(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
    msg: &RuleMessage,
    rules: &[MailRule],
    now: i64,
) -> anyhow::Result<i64> {
    let mut applied = 0;
    for rule in rules {
        if !matches(rule, msg) {
            continue;
        }
        // Claim the application first: the claim IS the idempotency, and it is
        // in the same transaction as the change it authorises.
        let claimed = tx.execute(
            "INSERT OR IGNORE INTO rule_applications (rule_id,revision,account_id,message_id,applied_at) \
             VALUES (?,?,?,?,?)",
            params![rule.id, rule.revision, account_id, msg.id, now],
        )?;
        if claimed == 0 {
            continue;
        }
        let diff = diff_for(&rule.actions, &msg.label_ids);
        if diff.add.is_empty() && diff.remove.is_empty() {
            // Matched but already in the requested state: the application is
            // recorded and no operation is queued, so a repeated sync is
            // silent rather than a stream of no-op operations.
            applied += 1;
            continue;
        }
        crate::actions::apply_message_diff(
            tx,
            account_id,
            &msg.thread_id,
            &msg.id,
            &diff.add,
            &diff.remove,
        )?;
        let ids = vec![msg.id.clone()];
        let key = rule_operation_key(account_id, &rule.id, rule.revision, &ids);
        let payload = payload_for(account_id, rule, &ids, &diff.add, &diff.remove);
        let op = crate::db::outbox::NewOp {
            account_id: account_id.to_string(),
            kind: "rule_apply".into(),
            payload,
            undo_group: Some(format!("rule:{}:{}", rule.id, rule.revision)),
            not_before: 0,
            operation_key: Some(key),
            summary_action: Some(format!("Rule: {}", rule.name)),
            ..Default::default()
        };
        crate::db::outbox::insert_op(tx, &op)?;
        applied += 1;
    }
    Ok(applied)
}

/// Drain newly ingested messages for one account.
///
/// The caller decides when to run this; it is deliberately not part of the
/// ingest transaction, because the point of the queue is that a rule can never
/// slow down (or fail) the metadata commit.
pub async fn process_queue(
    db: &Db,
    account_id: &str,
    foreground_busy: bool,
) -> Result<ApplyReport, SiftError> {
    if foreground_busy {
        return Ok(ApplyReport::default());
    }
    // Drop entries whose message is already gone before spending a batch on
    // them.
    let _ = db.rule_queue_prune(account_id).await;
    let entries = db
        .rule_queue_take(account_id, BATCH_MESSAGES)
        .await
        .map_err(db_error)?;
    if entries.is_empty() {
        return Ok(ApplyReport::default());
    }
    let rules = db.rules_enabled(account_id).await.map_err(db_error)?;
    if rules.is_empty() {
        db.rule_queue_done(&entries).await.map_err(db_error)?;
        return Ok(ApplyReport {
            messages: entries.len() as i64,
            ..Default::default()
        });
    }
    let started = crate::db::now_ms();
    let account = account_id.to_string();
    let msgs: Vec<RuleMessage> = {
        let mut out = Vec::with_capacity(entries.len());
        for e in &entries {
            if let Some(m) = db
                .rule_message(&e.account_id, &e.message_id)
                .await
                .map_err(db_error)?
            {
                out.push(m);
            }
        }
        out
    };
    let ids: Vec<String> = entries.iter().map(|e| e.message_id.clone()).collect();
    let report = db
        .write_tx(move |tx| {
            let now = crate::db::now_ms();
            let mut applied = 0;
            for msg in &msgs {
                applied += apply_rules_conn(tx, &account, msg, &rules, now)?;
            }
            for id in &ids {
                tx.execute(
                    "DELETE FROM rule_queue WHERE account_id=? AND message_id=?",
                    params![account, id],
                )?;
            }
            Ok(applied)
        })
        .await
        .map_err(db_error)?;
    Ok(ApplyReport {
        messages: entries.len() as i64,
        applied: report,
        skipped: 0,
        yielded: crate::db::now_ms() - started >= BATCH_BUDGET_MS,
    })
}

/// Preview what a rule would do to the mailbox as it is now.
///
/// This is the number the UI shows before the user presses Apply; nothing is
/// changed by asking.
pub async fn preview(
    db: &Db,
    account_id: &str,
    rule: &MailRule,
    limit: i64,
) -> Result<RulePreview, SiftError> {
    let limit = limit.clamp(1, 200);
    let ids = db
        .rule_candidate_ids(account_id, APPLY_EXISTING_LIMIT)
        .await
        .map_err(db_error)?;
    let mut count = 0i64;
    let mut sample: Vec<RulePreviewRow> = vec![];
    for id in &ids {
        let Some(msg) = db.rule_message(account_id, id).await.map_err(db_error)? else {
            continue;
        };
        if !matches(rule, &msg) {
            continue;
        }
        count += 1;
        if sample.len() < limit as usize {
            let diff = diff_for(&rule.actions, &msg.label_ids);
            let junk = diff.add.iter().any(|l| l == "SPAM");
            let (from_name, from_email) = thread_sender(db, account_id, id).await?;
            sample.push(RulePreviewRow {
                message_id: id.clone(),
                thread_id: msg.thread_id.clone(),
                subject: msg.subject.clone(),
                from_name,
                from_email,
                would_junk: junk,
            });
        }
    }
    Ok(RulePreview { count, sample })
}

async fn thread_sender(
    db: &Db,
    account_id: &str,
    message_id: &str,
) -> Result<(Option<String>, String), SiftError> {
    let (a, m) = (account_id.to_string(), message_id.to_string());
    db.read(move |c| {
        let row: Option<(Option<String>, Option<String>)> = c
            .query_row(
                "SELECT from_name, from_email FROM messages WHERE account_id=? AND id=?",
                params![a, m],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .ok();
        Ok(match row {
            Some((name, email)) => (name, email.unwrap_or_default()),
            None => (None, String::new()),
        })
    })
    .await
    .map_err(db_error)
}

/// "Apply to existing mail", the explicit half of P8.3.
///
/// Bounded, idempotent and separate from ingest: the preview already told the
/// user how many messages are involved, and running it twice applies the same
/// revision once per message.
pub async fn apply_existing(
    db: &Db,
    account_id: &str,
    rule: &MailRule,
) -> Result<ApplyReport, SiftError> {
    let ids = db
        .rule_candidate_ids(account_id, APPLY_EXISTING_LIMIT)
        .await
        .map_err(db_error)?;
    let account = account_id.to_string();
    let rule = rule.clone();
    let mut report = ApplyReport::default();
    let mut batch: Vec<RuleMessage> = vec![];
    for id in ids {
        let Some(msg) = db.rule_message(&account, &id).await.map_err(db_error)? else {
            continue;
        };
        if matches(&rule, &msg) {
            batch.push(msg);
        }
        if batch.len() as i64 >= BATCH_MESSAGES {
            let applied = flush(db, &account, &rule, &mut batch).await?;
            report.messages += BATCH_MESSAGES;
            report.applied += applied;
        }
    }
    report.applied += flush(db, &account, &rule, &mut batch).await?;
    report.messages += report.applied;
    Ok(report)
}

async fn flush(
    db: &Db,
    account_id: &str,
    rule: &MailRule,
    batch: &mut Vec<RuleMessage>,
) -> Result<i64, SiftError> {
    if batch.is_empty() {
        return Ok(0);
    }
    let msgs = std::mem::take(batch);
    let account = account_id.to_string();
    let rules = vec![rule.clone()];
    let applied = db
        .write_tx(move |tx| {
            let now = crate::db::now_ms();
            let mut applied = 0;
            for msg in &msgs {
                applied += apply_rules_conn(tx, &account, msg, &rules, now)?;
            }
            Ok(applied)
        })
        .await
        .map_err(db_error)?;
    Ok(applied)
}

/// Create or update the "Block sender in Sift" rule.
///
/// This is a rule, not an account-wide Gmail block: it moves future downloaded
/// mail from that address to Junk, and the UI copy says so because the
/// difference matters to anyone who also uses Gmail's own block button.
pub async fn block_sender(
    db: &Db,
    account_id: &str,
    email: &str,
    name: Option<&str>,
) -> Result<MailRule, SiftError> {
    let email = email.trim().to_ascii_lowercase();
    if !email.contains('@') || email.len() > 320 || email.contains(['\n', '\r']) {
        return Err(SiftError::app(
            "bad_address",
            "Sift needs a full email address to block a sender.",
            false,
        ));
    }
    let id = block_rule_id(account_id, &email);
    let label = name.map(str::to_string).unwrap_or_else(|| email.clone());
    let rule = MailRule {
        account_id: account_id.to_string(),
        id,
        name: format!("Block {label} in Sift"),
        enabled: true,
        match_mode: "all".into(),
        conditions: vec![RuleCondition::new("sender", "is", &email)],
        actions: vec![RuleAction::new("junk", None)],
        sort_order: 0,
        revision: 0,
        last_error: None,
    };
    crate::db::rules::validate(&rule).map_err(|m| SiftError::app("rule_invalid", m, false))?;
    let expected = db
        .rule_get(account_id, &rule.id)
        .await
        .map_err(db_error)?
        .map(|r| r.revision);
    db.rule_upsert(rule, expected).await.map_err(db_error)
}

/// A rule whose operation failed terminally is disabled with the reason,
/// instead of failing the account's sync over and over.
pub async fn disable_failed_rule(db: &Db, op: &crate::db::outbox::Op, error: &str) {
    let payload = op.payload_value();
    let Some(rule_id) = payload.get("ruleId").and_then(|v| v.as_str()) else {
        return;
    };
    let account = payload
        .get("accountId")
        .and_then(|v| v.as_str())
        .unwrap_or(&op.account_id);
    if let Err(e) = db.rule_disable(account, rule_id, error).await {
        log::warn!("could not disable failed rule {rule_id}: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::rules::validate;
    use crate::dto::{RuleAction, RuleCondition};

    fn msg(from: &str, subject: &str, to: &str, attachments: bool) -> RuleMessage {
        RuleMessage {
            id: "m1".into(),
            account_id: "a".into(),
            thread_id: "t1".into(),
            from_email: from.into(),
            to_json: format!(r#"[{{"n":null,"e":"{to}"}}]"#),
            cc_json: "[]".into(),
            bcc_json: "[]".into(),
            subject: subject.into(),
            has_attachments: attachments,
            label_ids: vec!["INBOX".into()],
        }
    }

    fn rule(mode: &str, conditions: Vec<RuleCondition>, actions: Vec<RuleAction>) -> MailRule {
        MailRule {
            account_id: "a".into(),
            id: "r1".into(),
            name: "Rule".into(),
            enabled: true,
            match_mode: mode.into(),
            conditions,
            actions,
            sort_order: 0,
            revision: 3,
            last_error: None,
        }
    }

    #[test]
    fn p8_3_matching_covers_the_closed_vocabulary() {
        let r = rule(
            "all",
            vec![RuleCondition::new("sender", "domain", "example.com")],
            vec![RuleAction::new("junk", None)],
        );
        assert!(matches(&r, &msg("ada@example.com", "Hi", "me@x", false)));
        assert!(!matches(&r, &msg("ada@other.com", "Hi", "me@x", false)));

        let r = rule(
            "all",
            vec![RuleCondition::new("recipient", "contains", "team@")],
            vec![RuleAction::new("archive", None)],
        );
        assert!(matches(&r, &msg("a@b", "Hi", "team@acme.com", false)));
        assert!(!matches(&r, &msg("a@b", "Hi", "me@acme.com", false)));

        let r = rule(
            "all",
            vec![RuleCondition::new("subject", "contains", "invoice")],
            vec![RuleAction::new("star", None)],
        );
        assert!(matches(
            &r,
            &msg("a@b", "Your INVOICE is ready", "me@x", false)
        ));
        assert!(!matches(&r, &msg("a@b", "Receipt", "me@x", false)));

        let r = rule(
            "all",
            vec![RuleCondition::new("hasAttachment", "isTrue", "")],
            vec![RuleAction::new("markRead", None)],
        );
        assert!(matches(&r, &msg("a@b", "x", "me@x", true)));
        assert!(!matches(&r, &msg("a@b", "x", "me@x", false)));

        // all vs any.
        let r = rule(
            "any",
            vec![
                RuleCondition::new("subject", "contains", "invoice"),
                RuleCondition::new("hasAttachment", "isTrue", ""),
            ],
            vec![RuleAction::new("star", None)],
        );
        assert!(matches(&r, &msg("a@b", "Receipt", "me@x", true)));
        assert!(matches(&r, &msg("a@b", "Invoice", "me@x", false)));
        assert!(!matches(&r, &msg("a@b", "Receipt", "me@x", false)));
    }

    #[test]
    fn p8_3_operation_key_is_stable_across_processes() {
        let a = rule_operation_key("acct", "r1", 2, &["m1".into(), "m2".into()]);
        let b = rule_operation_key("acct", "r1", 2, &["m1".into(), "m2".into()]);
        assert_eq!(a, b);
        // A different revision, account or message set is a different
        // operation.
        assert_ne!(
            a,
            rule_operation_key("acct", "r1", 3, &["m1".into(), "m2".into()])
        );
        assert_ne!(a, rule_operation_key("acct2", "r1", 2, &["m1".into()]));
        assert_ne!(a, rule_operation_key("acct", "r1", 2, &["m1".into()]));
    }

    #[test]
    fn p8_3_block_sender_rule_is_valid_and_deterministic() {
        let id = block_rule_id("a", "spam@example.com");
        assert_eq!(id, block_rule_id("a", "spam@example.com"));
        assert_ne!(id, block_rule_id("b", "spam@example.com"));
        let rule = crate::dto::MailRule {
            account_id: "a".into(),
            id,
            name: "Block spam@example.com in Sift".into(),
            enabled: true,
            match_mode: "all".into(),
            conditions: vec![RuleCondition::new("sender", "is", "spam@example.com")],
            actions: vec![RuleAction::new("junk", None)],
            sort_order: 0,
            revision: 1,
            last_error: None,
        };
        assert!(validate(&rule).is_ok());
        assert!(matches(
            &rule,
            &msg("spam@example.com", "Buy", "me@x", false)
        ));
    }
}
