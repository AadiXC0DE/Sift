//! Rule storage (P8.3).
//!
//! The rules themselves are plain rows; the vocabulary, the matching and the
//! decision to act live in [`crate::rules`]. Keeping the SQL here means the
//! engine can be tested against a temp database without a provider.
//!
//! Two idempotency mechanisms are deliberate:
//!
//! * `rule_applications` is keyed by `(rule_id, revision, account_id,
//!   message_id)`, so re-running sync, restarting the app or replaying the
//!   ingest queue can never apply the same revision of a rule to the same
//!   message twice;
//! * `rule_queue` holds newly ingested messages in the same transaction as the
//!   message row itself, so evaluation always sees committed metadata and can
//!   never run ahead of it.

use super::Db;
use crate::dto::{MailRule, RuleAction, RuleCondition};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// A message waiting for rule evaluation.
#[derive(Debug, Clone)]
pub struct QueuedMessage {
    pub account_id: String,
    pub message_id: String,
}

/// The metadata a rule is evaluated against. Read after the ingest commit and
/// never mutated by the rules themselves, so ordered rules all decide on the
/// original metadata.
#[derive(Debug, Clone, Default)]
pub struct RuleMessage {
    pub id: String,
    pub account_id: String,
    pub thread_id: String,
    pub from_email: String,
    pub to_json: String,
    pub cc_json: String,
    pub bcc_json: String,
    pub subject: String,
    pub has_attachments: bool,
    pub label_ids: Vec<String>,
}

fn rule_from_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<MailRule> {
    let conditions_json: String = r.get(5)?;
    let actions_json: String = r.get(6)?;
    let enabled: i64 = r.get(3)?;
    Ok(MailRule {
        account_id: r.get(0)?,
        id: r.get(1)?,
        name: r.get(2)?,
        enabled: enabled != 0,
        match_mode: r.get(4)?,
        conditions: serde_json::from_str(&conditions_json).unwrap_or_default(),
        actions: serde_json::from_str(&actions_json).unwrap_or_default(),
        sort_order: r.get(7)?,
        revision: r.get(8)?,
        last_error: r.get(9)?,
    })
}

const RULE_COLUMNS: &str =
    "account_id,id,name,enabled,match_mode,conditions_json,actions_json,sort_order,revision,last_error";

/// Queue a newly ingested message for rule evaluation.
///
/// Only when the account already finished its first sync **and** at least one
/// rule is enabled: an initial sync must not act on history, and a user who
/// never wrote a rule should pay nothing for the feature.
pub(crate) fn queue_new_message_conn(
    c: &Connection,
    account_id: &str,
    message_id: &str,
    now: i64,
) -> Result<()> {
    c.execute(
        "INSERT OR IGNORE INTO rule_queue (account_id, message_id, queued_at) \
         SELECT ?1, ?2, ?3 WHERE EXISTS ( \
             SELECT 1 FROM accounts a WHERE a.id=?1 AND a.sync_state<>'new' \
           ) AND EXISTS ( \
             SELECT 1 FROM mail_rules r WHERE r.account_id=?1 AND r.enabled=1 \
           )",
        params![account_id, message_id, now],
    )?;
    Ok(())
}

impl Db {
    pub async fn rules_list(&self, account_id: &str) -> Result<Vec<MailRule>> {
        let a = account_id.to_string();
        self.read(move |c| {
            let sql = format!(
                "SELECT {RULE_COLUMNS} FROM mail_rules WHERE account_id=? \
                 ORDER BY sort_order, id"
            );
            let rows = c
                .prepare(&sql)?
                .query_map(params![a], rule_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    pub async fn rule_get(&self, account_id: &str, rule_id: &str) -> Result<Option<MailRule>> {
        let (a, r) = (account_id.to_string(), rule_id.to_string());
        self.read(move |c| {
            let sql = format!("SELECT {RULE_COLUMNS} FROM mail_rules WHERE account_id=? AND id=?");
            Ok(c.query_row(&sql, params![a, r], rule_from_row).optional()?)
        })
        .await
    }

    /// Insert or update one rule. `expected_revision` implements the same
    /// optimistic check drafts use: a stale editor cannot silently overwrite a
    /// newer edit, and every accepted edit bumps `revision` so rule
    /// applications are bound to the exact version that produced them.
    pub async fn rule_upsert(
        &self,
        rule: MailRule,
        expected_revision: Option<i64>,
    ) -> Result<MailRule> {
        let written = rule.clone();
        self.write_tx(move |tx| {
            let now = super::now_ms();
            let existing: Option<(i64, i64)> = tx
                .query_row(
                    "SELECT revision, sort_order FROM mail_rules WHERE account_id=? AND id=?",
                    params![written.account_id, written.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let revision = match existing {
                None => 1,
                Some((current, _)) => {
                    if let Some(expected) = expected_revision {
                        if expected != current {
                            return Err(anyhow::anyhow!(crate::errors::SiftError::app(
                                "rule_conflict",
                                "This rule changed since you opened it. Reopen it and edit again.",
                                false,
                            )));
                        }
                    }
                    current + 1
                }
            };
            let conditions = serde_json::to_string(&written.conditions)?;
            let actions = serde_json::to_string(&written.actions)?;
            let sort_order = existing.map(|(_, s)| s).unwrap_or(written.sort_order);
            tx.execute(
                "INSERT INTO mail_rules \
                 (account_id,id,name,enabled,match_mode,conditions_json,actions_json,sort_order,revision,last_error,created_at,updated_at) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,NULL,?10,?10) \
                 ON CONFLICT(account_id,id) DO UPDATE SET \
                   name=excluded.name, enabled=excluded.enabled, match_mode=excluded.match_mode, \
                   conditions_json=excluded.conditions_json, actions_json=excluded.actions_json, \
                   revision=excluded.revision, last_error=NULL, updated_at=excluded.updated_at",
                params![
                    written.account_id,
                    written.id,
                    written.name,
                    written.enabled as i32,
                    written.match_mode,
                    conditions,
                    actions,
                    sort_order,
                    revision,
                    now,
                ],
            )?;
            Ok(revision)
        })
        .await
        .map(|revision| MailRule { revision, ..rule })
    }

    pub async fn rule_delete(&self, account_id: &str, rule_id: &str) -> Result<()> {
        let (a, r) = (account_id.to_string(), rule_id.to_string());
        self.write(move |c| {
            c.execute(
                "DELETE FROM mail_rules WHERE account_id=? AND id=?",
                params![a, r],
            )?;
            c.execute(
                "DELETE FROM rule_applications WHERE account_id=? AND rule_id=?",
                params![a, r],
            )?;
            Ok(())
        })
        .await
    }

    /// Disable a rule that failed terminally, recording why. Sync is never
    /// blocked by a rule: the account keeps working, the rule stops.
    pub async fn rule_disable(&self, account_id: &str, rule_id: &str, error: &str) -> Result<()> {
        let (a, r, e) = (
            account_id.to_string(),
            rule_id.to_string(),
            error.to_string(),
        );
        self.write(move |c| {
            c.execute(
                "UPDATE mail_rules SET enabled=0, last_error=?3, updated_at=?4 \
                 WHERE account_id=?1 AND id=?2",
                params![a, r, e, super::now_ms()],
            )?;
            Ok(())
        })
        .await
    }

    /// Enabled rules in application order.
    pub async fn rules_enabled(&self, account_id: &str) -> Result<Vec<MailRule>> {
        let a = account_id.to_string();
        self.read(move |c| {
            let sql = format!(
                "SELECT {RULE_COLUMNS} FROM mail_rules WHERE account_id=? AND enabled=1 \
                 ORDER BY sort_order, id"
            );
            let rows = c
                .prepare(&sql)?
                .query_map(params![a], rule_from_row)?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Up to `limit` queued messages for one account, oldest first.
    pub async fn rule_queue_take(
        &self,
        account_id: &str,
        limit: i64,
    ) -> Result<Vec<QueuedMessage>> {
        let a = account_id.to_string();
        self.read(move |c| {
            let mut s = c.prepare(
                "SELECT account_id, message_id FROM rule_queue WHERE account_id=? \
                 ORDER BY queued_at, message_id LIMIT ?",
            )?;
            let rows = s
                .query_map(params![a, limit], |r| {
                    Ok(QueuedMessage {
                        account_id: r.get(0)?,
                        message_id: r.get(1)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Drop queue entries that are no longer connected to a local message (a
    /// message can be deleted before the queue drains).
    pub async fn rule_queue_prune(&self, account_id: &str) -> Result<usize> {
        let a = account_id.to_string();
        self.write(move |c| {
            let n = c.execute(
                "DELETE FROM rule_queue WHERE account_id=?1 AND NOT EXISTS ( \
                   SELECT 1 FROM messages m WHERE m.account_id=?1 AND m.id=rule_queue.message_id)",
                params![a],
            )?;
            Ok(n)
        })
        .await
    }

    pub async fn rule_queue_done(&self, entries: &[QueuedMessage]) -> Result<()> {
        let entries = entries.to_vec();
        self.write_tx(move |tx| {
            for e in &entries {
                tx.execute(
                    "DELETE FROM rule_queue WHERE account_id=? AND message_id=?",
                    params![e.account_id, e.message_id],
                )?;
            }
            Ok(())
        })
        .await
    }

    pub async fn rule_queue_len(&self, account_id: &str) -> Result<i64> {
        let a = account_id.to_string();
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT count(*) FROM rule_queue WHERE account_id=?",
                params![a],
                |r| r.get(0),
            )?)
        })
        .await
    }

    pub async fn rule_message(
        &self,
        account_id: &str,
        message_id: &str,
    ) -> Result<Option<RuleMessage>> {
        type Row = (
            String,
            String,
            Option<String>,
            String,
            String,
            String,
            String,
            i64,
            String,
        );
        let (a, m) = (account_id.to_string(), message_id.to_string());
        self.read(move |c| {
            let row: Option<Row> = c
                .query_row(
                    "SELECT id, account_id, from_email, to_json, cc_json, bcc_json, subject, has_attachments, thread_id \
                     FROM messages WHERE account_id=? AND id=?",
                    params![a, m],
                    |r| {
                        Ok((
                            r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?,
                            r.get(5)?, r.get(6)?, r.get(7)?, r.get(8)?,
                        ))
                    },
                )
                .optional()?;
            let Some((id, account_id, from_email, to_json, cc_json, bcc_json, subject, has_attachments, thread_id)) = row else {
                return Ok(None);
            };
            let label_ids: Vec<String> = c
                .prepare("SELECT label_id FROM message_labels WHERE account_id=? AND message_id=? ORDER BY label_id")?
                .query_map(params![account_id, id], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(Some(RuleMessage {
                id,
                account_id,
                thread_id,
                from_email: from_email.unwrap_or_default(),
                to_json,
                cc_json,
                bcc_json,
                subject,
                has_attachments: has_attachments != 0,
                label_ids,
            }))
        })
        .await
    }

    /// Claim the right to apply one revision of a rule to one message.
    /// `false` means another run already did.
    pub async fn rule_application_claim(
        &self,
        account_id: &str,
        rule_id: &str,
        revision: i64,
        message_id: &str,
    ) -> Result<bool> {
        let (a, r, m) = (
            account_id.to_string(),
            rule_id.to_string(),
            message_id.to_string(),
        );
        let now = super::now_ms();
        self.write(move |c| {
            let n = c.execute(
                "INSERT OR IGNORE INTO rule_applications (rule_id,revision,account_id,message_id,applied_at) \
                 VALUES (?,?,?,?,?)",
                params![r, revision, a, m, now],
            )?;
            Ok(n == 1)
        })
        .await
    }

    /// Count how many messages in the mailbox a rule would match right now.
    /// This is the number the preview shows before anything is applied.
    pub async fn rule_preview_count(&self, account_id: &str, rule: &MailRule) -> Result<i64> {
        let rows = self.rule_candidate_ids(account_id, 20_000).await?;
        let mut count = 0;
        for id in rows {
            if let Some(msg) = self.rule_message(account_id, &id).await? {
                if crate::rules::matches(rule, &msg) {
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    /// Message ids in the account, newest first, bounded.
    pub async fn rule_candidate_ids(&self, account_id: &str, limit: i64) -> Result<Vec<String>> {
        let a = account_id.to_string();
        self.read(move |c| {
            let mut s = c.prepare(
                "SELECT id FROM messages WHERE account_id=? AND is_draft=0 \
                 ORDER BY internal_date DESC, id DESC LIMIT ?",
            )?;
            let rows = s
                .query_map(params![a, limit], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Ids of messages already carrying `label` in the account.
    pub async fn rule_label_members(&self, account_id: &str, label: &str) -> Result<Vec<String>> {
        let (a, l) = (account_id.to_string(), label.to_string());
        self.read(move |c| {
            let mut s = c.prepare(
                "SELECT message_id FROM message_labels WHERE account_id=? AND label_id=?",
            )?;
            let rows = s
                .query_map(params![a, l], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(rows)
        })
        .await
    }
}

/// The condition/action vocabulary, kept here next to the storage so the two
/// cannot drift. Anything outside it is rejected before a rule is stored.
pub const CONDITION_FIELDS: [&str; 4] = ["sender", "recipient", "subject", "hasAttachment"];
pub const ACTION_KINDS: [&str; 5] = ["addLabel", "archive", "markRead", "star", "junk"];

/// Validate one rule. Returns the reason a rule may not be stored.
pub fn validate(rule: &MailRule) -> std::result::Result<(), String> {
    if rule.name.trim().is_empty() {
        return Err("A rule needs a name.".into());
    }
    if rule.conditions.is_empty() {
        return Err("A rule needs at least one condition.".into());
    }
    if rule.actions.is_empty() {
        return Err("A rule needs at least one action.".into());
    }
    if rule.match_mode != "all" && rule.match_mode != "any" {
        return Err("A rule matches either all or any of its conditions.".into());
    }
    for c in &rule.conditions {
        if !CONDITION_FIELDS.contains(&c.field.as_str()) {
            return Err(format!(
                "\"{}\" is not one of the conditions Sift can check.",
                c.field
            ));
        }
        let op_ok = match c.field.as_str() {
            "sender" | "recipient" => matches!(c.op.as_str(), "contains" | "is" | "domain"),
            "subject" => c.op == "contains",
            "hasAttachment" => c.op == "isTrue",
            _ => false,
        };
        if !op_ok {
            return Err(format!(
                "\"{}\" is not something Sift can check for {}.",
                c.op, c.field
            ));
        }
        if c.field != "hasAttachment" && c.value.trim().is_empty() {
            return Err("A condition needs a value to compare against.".into());
        }
        if c.field == "subject" && c.value.len() > 512 {
            return Err("A subject match is limited to 512 characters.".into());
        }
        if c.field == "sender" || c.field == "recipient" {
            if c.value.contains('\n') || c.value.contains('\r') {
                return Err("An address match cannot contain line breaks.".into());
            }
            if c.value.len() > 320 {
                return Err("An address match is limited to 320 characters.".into());
            }
        }
    }
    for a in &rule.actions {
        if !ACTION_KINDS.contains(&a.kind.as_str()) {
            return Err(format!("\"{}\" is not an action Sift can take.", a.kind));
        }
        if a.kind == "addLabel" && a.label_id.as_deref().unwrap_or("").trim().is_empty() {
            return Err("Adding a label needs a label.".into());
        }
    }
    Ok(())
}

/// Where a rule action sends a message, expressed as the label diff the
/// existing action service already understands.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RuleDiff {
    pub add: Vec<String>,
    pub remove: Vec<String>,
}

/// Translate ordered actions into one label diff. Later actions do not undo
/// earlier ones, and an action already satisfied by the original metadata is
/// dropped rather than queued as a no-op.
pub fn diff_for(actions: &[RuleAction], current: &[String]) -> RuleDiff {
    let mut add: Vec<String> = vec![];
    let mut remove: Vec<String> = vec![];
    let mut add_l = |l: &str| {
        if !current.iter().any(|x| x == l) && !add.iter().any(|x: &String| x == l) {
            add.push(l.to_string());
        }
    };
    let mut remove_l = |l: &str| {
        if current.iter().any(|x| x == l) && !remove.iter().any(|x: &String| x == l) {
            remove.push(l.to_string());
        }
    };
    for a in actions {
        match a.kind.as_str() {
            "addLabel" => {
                if let Some(l) = a.label_id.as_deref() {
                    add_l(l);
                }
            }
            "archive" => remove_l("INBOX"),
            "junk" => {
                add_l("SPAM");
                remove_l("INBOX");
            }
            "markRead" => remove_l("UNREAD"),
            "star" => add_l("STARRED"),
            _ => {}
        }
    }
    RuleDiff { add, remove }
}

impl RuleCondition {
    pub fn new(field: &str, op: &str, value: &str) -> Self {
        Self {
            field: field.into(),
            op: op.into(),
            value: value.into(),
        }
    }
}

impl RuleAction {
    pub fn new(kind: &str, label_id: Option<&str>) -> Self {
        Self {
            kind: kind.into(),
            label_id: label_id.map(str::to_string),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(mode: &str) -> MailRule {
        MailRule {
            account_id: "a".into(),
            id: "r1".into(),
            name: "Client".into(),
            enabled: true,
            match_mode: mode.into(),
            conditions: vec![RuleCondition::new("sender", "domain", "example.com")],
            actions: vec![RuleAction::new("junk", None)],
            sort_order: 0,
            revision: 1,
            last_error: None,
        }
    }

    #[test]
    fn p8_3_validation_closes_the_vocabulary() {
        assert!(validate(&rule("all")).is_ok());
        let mut bad = rule("all");
        bad.conditions = vec![RuleCondition::new("body", "contains", "x")];
        assert!(validate(&bad).is_err());
        let mut regex = rule("all");
        regex.conditions = vec![RuleCondition::new("subject", "matches", "^a.*")];
        assert!(validate(&regex).is_err());
        let mut forward = rule("all");
        forward.actions = vec![RuleAction::new("forward", Some("x@y"))];
        assert!(validate(&forward).is_err());
        let mut del = rule("all");
        del.actions = vec![RuleAction::new("deleteForever", None)];
        assert!(validate(&del).is_err());
        let mut any = rule("any");
        any.conditions = vec![];
        assert!(validate(&any).is_err());
    }

    #[test]
    fn p8_3_diff_is_ordered_and_skips_what_is_already_true() {
        let actions = vec![
            RuleAction::new("junk", None),
            RuleAction::new("star", None),
            RuleAction::new("addLabel", Some("Label_1")),
        ];
        let d = diff_for(&actions, &["INBOX".into()]);
        assert_eq!(
            d.add,
            vec!["SPAM".to_string(), "STARRED".into(), "Label_1".into()]
        );
        assert_eq!(d.remove, vec!["INBOX".to_string()]);
        // Already junked and starred: only the label is new.
        let d = diff_for(&actions, &["SPAM".into(), "STARRED".into()]);
        assert_eq!(d.add, vec!["Label_1".to_string()]);
        assert!(d.remove.is_empty());
    }
}
