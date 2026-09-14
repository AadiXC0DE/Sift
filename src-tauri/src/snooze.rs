//! Snooze: local scheduling with an optional matching remote label (P6.5).
//!
//! Snooze is **not** a cross-device scheduler. The timer is local and the
//! label is the only part that travels; a device that never sees the label
//! still wakes the thread at its local deadline, and a device that does see it
//! shows the same grouping. Everything a wake needs — the timer, the label
//! change and the queued provider operation — is written in ONE transaction,
//! so a timer can never disappear before the operation that restores the
//! thread is durable.
//!
//! The label is addressed by *name*, and the provider's own id for that name
//! is resolved when the operation is claimed. While the label does not exist
//! remotely, an operation that creates it is queued first and the label
//! operations depend on it — an invented provider id is never sent.

use crate::actions::{GestureFailure, PreviousState, PriorMessage};
use crate::dto::GestureTarget;
use crate::db::outbox::{NewOp, Op};
use crate::db::Db;
use crate::errors::SiftError;

/// The one label name Sift uses for snoozed mail.
pub const SNOOZE_LABEL_NAME: &str = "Sift/Snoozed";

/// How long the watcher is willing to sleep before re-checking the clock.
/// The nearest deadline wakes it earlier; this cap keeps a DST or manual clock
/// change from stranding a due thread.
pub const WATCH_FALLBACK_MS: i64 = 30_000;

#[derive(Debug, Clone, Default)]
pub struct SnoozeOutcome {
    pub operations: Vec<Op>,
    pub thread_ids: Vec<String>,
}

fn db_error(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

fn message_ids_conn(
    conn: &rusqlite::Connection,
    account_id: &str,
    thread_id: &str,
) -> anyhow::Result<Vec<String>> {
    let mut s = conn.prepare(
        "SELECT id FROM messages WHERE account_id=? AND thread_id=? ORDER BY internal_date, id",
    )?;
    let rows: Vec<String> = s
        .query_map(rusqlite::params![account_id, thread_id], |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(rows)
}

fn prior_of(
    conn: &rusqlite::Connection,
    account_id: &str,
    message_id: &str,
) -> anyhow::Result<PriorMessage> {
    let (labels, unread, starred): (String, i64, i64) = conn.query_row(
        "SELECT label_ids, is_unread, is_starred FROM messages WHERE account_id=? AND id=?",
        rusqlite::params![account_id, message_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(PriorMessage {
        id: message_id.to_string(),
        labels: serde_json::from_str(&labels).unwrap_or_default(),
        unread: unread != 0,
        starred: starred != 0,
    })
}

/// Apply add/remove to one message inside the caller's transaction.
fn apply_labels(
    conn: &rusqlite::Connection,
    account_id: &str,
    message_id: &str,
    add: &[String],
    remove: &[String],
) -> anyhow::Result<()> {
    for l in add {
        conn.execute(
            "INSERT OR IGNORE INTO message_labels (account_id,message_id,label_id) VALUES (?,?,?)",
            rusqlite::params![account_id, message_id, l],
        )?;
    }
    for l in remove {
        conn.execute(
            "DELETE FROM message_labels WHERE account_id=? AND message_id=? AND label_id=?",
            rusqlite::params![account_id, message_id, l],
        )?;
    }
    let has = |lid: &str| -> rusqlite::Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM message_labels WHERE account_id=? AND message_id=? AND label_id=?)",
            rusqlite::params![account_id, message_id, lid],
            |r| r.get(0),
        )
    };
    let labels: Vec<String> = conn
        .prepare("SELECT label_id FROM message_labels WHERE account_id=? AND message_id=? ORDER BY label_id")?
        .query_map(rusqlite::params![account_id, message_id], |r| r.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    conn.execute(
        "UPDATE messages SET is_unread=?, is_starred=?, is_draft=?, label_ids=? \
         WHERE account_id=? AND id=?",
        rusqlite::params![
            has("UNREAD")? as i32,
            has("STARRED")? as i32,
            has("DRAFT")? as i32,
            serde_json::to_string(&labels)?,
            account_id,
            message_id
        ],
    )?;
    Ok(())
}

/// Resolve (and if needed schedule the creation of) the account's snooze
/// label. Returns the local id to attach locally and the operation a label
/// change must wait for, if any.
fn ensure_label(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
) -> anyhow::Result<(String, Option<i64>)> {
    let existing: Option<String> = tx
        .query_row(
            "SELECT id FROM labels WHERE account_id=? AND name=? LIMIT 1",
            rusqlite::params![account_id, SNOOZE_LABEL_NAME],
            |r| r.get(0),
        )
        .ok();
    if let Some(id) = existing {
        return Ok((id, None));
    }
    let placeholder = format!("sift-local:{account_id}:{SNOOZE_LABEL_NAME}");
    tx.execute(
        "INSERT OR REPLACE INTO labels (account_id,id,name,kind,color_bg,color_fg,visible,unread_count,total_count,sort_order) \
         VALUES (?,?,?,?,NULL,NULL,1,0,0,200)",
        rusqlite::params![account_id, placeholder, SNOOZE_LABEL_NAME, "user"],
    )?;
    // The remote label is created by an operation of its own; every label
    // change waits for it (P6.5: a queued create-label dependency offline).
    let key = crate::outbox::label_operation_key(account_id, SNOOZE_LABEL_NAME);
    let existing_op: Option<(i64, String)> = tx
        .query_row(
            "SELECT id,state FROM outbox_ops WHERE operation_key=?",
            rusqlite::params![key],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .ok();
    let dep = match existing_op {
        Some((id, state))
            if !matches!(
                state.as_str(),
                crate::db::outbox::STATE_FAILED | crate::db::outbox::STATE_CANCELLED
            ) =>
        {
            Some(id)
        }
        _ => {
            let op = NewOp {
                account_id: account_id.to_string(),
                kind: "create_label".into(),
                payload: serde_json::json!({"name": SNOOZE_LABEL_NAME}).to_string(),
                operation_key: Some(key),
                summary_action: Some("Creating a label".into()),
                ..Default::default()
            };
            Some(crate::db::outbox::insert_op(tx, &op)?.id)
        }
    };
    Ok((placeholder, dep))
}

/// Snooze a set of account-qualified threads.
///
/// Per account: set the timer, remove Inbox from the messages, add the
/// resolved Snoozed label and queue the provider operation — all in one
/// transaction, with the pre-snooze membership saved for an exact Undo.
pub async fn snooze_set(
    db: &Db,
    gesture_id: &str,
    targets: &[GestureTarget],
    wake_at: i64,
    wake_unread: bool,
) -> Result<(SnoozeOutcome, Vec<GestureFailure>), SiftError> {
    let mut accounts: Vec<String> = targets.iter().map(|t| t.account_id.clone()).collect();
    accounts.sort();
    accounts.dedup();
    let mut outcome = SnoozeOutcome::default();
    let mut failures: Vec<GestureFailure> = vec![];
    for account_id in accounts {
        let scoped: Vec<GestureTarget> = targets
            .iter()
            .filter(|t| t.account_id == account_id)
            .cloned()
            .collect();
        let account = account_id.clone();
        let group = gesture_id.to_string();
        let result = db
            .write_tx(move |tx| snooze_set_account(tx, &account, &scoped, &group, wake_at, wake_unread))
            .await;
        match result {
            Ok((op_id, threads)) => {
                outcome.thread_ids.extend(threads);
                if let Some(op) = db.outbox_get(op_id).await.map_err(db_error)? {
                    outcome.operations.push(op);
                }
            }
            Err(e) => failures.push(GestureFailure {
                account_id,
                code: "db".into(),
                message: e.to_string(),
            }),
        }
    }
    Ok((outcome, failures))
}

fn snooze_set_account(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
    targets: &[GestureTarget],
    gesture_id: &str,
    wake_at: i64,
    wake_unread: bool,
) -> anyhow::Result<(i64, Vec<String>)> {
    let (label_id, dependency) = ensure_label(tx, account_id)?;
    let mut ids: Vec<String> = vec![];
    let mut previous = PreviousState {
        touched: vec!["INBOX".into(), "UNREAD".into(), SNOOZE_LABEL_NAME.into()],
        messages: vec![],
    };
    let mut threads: Vec<String> = vec![];
    let now = crate::db::now_ms();
    for target in targets {
        let mids = message_ids_conn(tx, account_id, &target.thread_id)?;
        if mids.is_empty() {
            continue;
        }
        let mut thread_previous: Vec<serde_json::Value> = vec![];
        for id in &mids {
            let prior = prior_of(tx, account_id, id)?;
            thread_previous.push(serde_json::to_value(&prior)?);
            let mut add: Vec<String> = vec![];
            let mut remove: Vec<String> = vec![];
            if !prior.labels.iter().any(|l| l == SNOOZE_LABEL_NAME) {
                add.push(label_id.clone());
            }
            if prior.labels.iter().any(|l| l == "INBOX") {
                remove.push("INBOX".into());
            }
            if !add.is_empty() || !remove.is_empty() {
                apply_labels(tx, account_id, id, &add, &remove)?;
            }
            ids.push(id.clone());
            previous.messages.push(prior);
        }
        // The saved membership is what Unsnooze (and an Undo) restores exactly.
        let saved = serde_json::json!({ "messages": thread_previous }).to_string();
        tx.execute(
            "INSERT INTO snoozes (account_id,thread_id,wake_at,label_id,previous_json,wake_unread,gesture_id,created_at,state) \
             VALUES (?,?,?,?,?,?,?,?,'sleeping') \
             ON CONFLICT(account_id,thread_id) DO UPDATE SET \
               wake_at=excluded.wake_at, label_id=excluded.label_id, previous_json=excluded.previous_json, \
               wake_unread=excluded.wake_unread, gesture_id=excluded.gesture_id, state='sleeping'",
            rusqlite::params![
                account_id,
                target.thread_id,
                wake_at,
                label_id,
                saved,
                wake_unread as i32,
                gesture_id,
                now
            ],
        )?;
        tx.execute(
            "UPDATE threads SET snoozed_until=? WHERE account_id=? AND id=?",
            rusqlite::params![wake_at, account_id, target.thread_id],
        )?;
        crate::db::threads::recompute_thread_conn(tx, account_id, &target.thread_id)?;
        threads.push(target.thread_id.clone());
    }
    if ids.is_empty() {
        anyhow::bail!("nothing to snooze");
    }
    // The provider operation addresses the label by name; the id is resolved
    // when it is claimed, once the create-label dependency has run.
    let payload = serde_json::json!({
        "ids": ids,
        "add": [format!("name:{SNOOZE_LABEL_NAME}")],
        "remove": ["INBOX"],
    })
    .to_string();
    let op = NewOp {
        account_id: account_id.to_string(),
        kind: "modify_labels".into(),
        payload,
        undo_group: Some(gesture_id.to_string()),
        not_before: 0,
        previous_state_json: previous.to_json(),
        depends_on_op_id: dependency,
        summary_action: Some("Snoozing".into()),
        ..Default::default()
    };
    let id = crate::db::outbox::insert_op(tx, &op)?.id;
    Ok((id, threads))
}

/// Unsnooze: remove the timer and the label and restore the saved Inbox policy
/// (P6.5). This is not "cancel the timer and hope".
pub async fn snooze_clear(
    db: &Db,
    gesture_id: &str,
    targets: &[GestureTarget],
) -> Result<(SnoozeOutcome, Vec<GestureFailure>), SiftError> {
    let mut accounts: Vec<String> = targets.iter().map(|t| t.account_id.clone()).collect();
    accounts.sort();
    accounts.dedup();
    let mut outcome = SnoozeOutcome::default();
    let mut failures: Vec<GestureFailure> = vec![];
    for account_id in accounts {
        let scoped: Vec<GestureTarget> = targets
            .iter()
            .filter(|t| t.account_id == account_id)
            .cloned()
            .collect();
        let account = account_id.clone();
        let group = gesture_id.to_string();
        let result = db
            .write_tx(move |tx| snooze_clear_account(tx, &account, &scoped, &group))
            .await;
        match result {
            Ok((op_id, threads)) => {
                outcome.thread_ids.extend(threads);
                if let Some(op_id) = op_id {
                    if let Some(op) = db.outbox_get(op_id).await.map_err(db_error)? {
                        outcome.operations.push(op);
                    }
                }
            }
            Err(e) => failures.push(GestureFailure {
                account_id,
                code: "db".into(),
                message: e.to_string(),
            }),
        }
    }
    Ok((outcome, failures))
}

fn snooze_clear_account(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
    targets: &[GestureTarget],
    gesture_id: &str,
) -> anyhow::Result<(Option<i64>, Vec<String>)> {
    let label_id: Option<String> = tx
        .query_row(
            "SELECT id FROM labels WHERE account_id=? AND name=? LIMIT 1",
            rusqlite::params![account_id, SNOOZE_LABEL_NAME],
            |r| r.get(0),
        )
        .ok();
    let mut ids: Vec<String> = vec![];
    let mut add: Vec<String> = vec![];
    let mut remove: Vec<String> = vec![];
    let mut previous = PreviousState {
        touched: vec!["INBOX".into(), SNOOZE_LABEL_NAME.into()],
        messages: vec![],
    };
    let mut threads: Vec<String> = vec![];
    for target in targets {
        let saved: Option<String> = tx
            .query_row(
                "SELECT previous_json FROM snoozes WHERE account_id=? AND thread_id=?",
                rusqlite::params![account_id, target.thread_id],
                |r| r.get(0),
            )
            .ok();
        let saved = PreviousState::parse(saved.as_deref());
        for id in message_ids_conn(tx, account_id, &target.thread_id)? {
            let prior = prior_of(tx, account_id, &id)?;
            let saved_msg = saved.messages.iter().find(|m| m.id == id);
            // Restore the Inbox policy exactly as it was when snoozed.
            let want_inbox = saved_msg
                .map(|m| m.labels.iter().any(|l| l == "INBOX"))
                .unwrap_or(false);
            let has_inbox = prior.labels.iter().any(|l| l == "INBOX");
            if want_inbox && !has_inbox && !add.contains(&"INBOX".to_string()) {
                add.push("INBOX".into());
            }
            if let Some(label_id) = &label_id {
                if prior.labels.iter().any(|l| l == label_id) && !remove.contains(label_id) {
                    remove.push(label_id.clone());
                }
            }
            previous.messages.push(prior);
            ids.push(id);
        }
        tx.execute(
            "DELETE FROM snoozes WHERE account_id=? AND thread_id=?",
            rusqlite::params![account_id, target.thread_id],
        )?;
        tx.execute(
            "UPDATE threads SET snoozed_until=NULL WHERE account_id=? AND id=?",
            rusqlite::params![account_id, target.thread_id],
        )?;
        threads.push(target.thread_id.clone());
    }
    for id in &ids {
        apply_labels(tx, account_id, id, &add, &remove)?;
    }
    for thread in &threads {
        crate::db::threads::recompute_thread_conn(tx, account_id, thread)?;
    }
    if ids.is_empty() || (add.is_empty() && remove.is_empty()) {
        return Ok((None, threads));
    }
    let payload = serde_json::json!({"ids": ids, "add": add, "remove": remove}).to_string();
    let op = NewOp {
        account_id: account_id.to_string(),
        kind: "modify_labels".into(),
        payload,
        undo_group: Some(gesture_id.to_string()),
        not_before: 0,
        previous_state_json: previous.to_json(),
        summary_action: Some("Unsnoozing".into()),
        ..Default::default()
    };
    let id = crate::db::outbox::insert_op(tx, &op)?.id;
    Ok((Some(id), threads))
}

/// The next local deadline, if any thread is asleep.
pub async fn next_deadline(db: &Db) -> Result<Option<i64>, SiftError> {
    db.read(|c| {
        Ok(c.query_row(
            "SELECT MIN(wake_at) FROM snoozes WHERE state='sleeping'",
            [],
            |r| r.get::<_, Option<i64>>(0),
        )?)
    })
    .await
    .map_err(db_error)
}

/// Wake every thread whose deadline has passed.
///
/// The timer removal, the label change and the queued operation share one
/// transaction per account: the wake cannot be lost between them.
pub async fn wake_due(db: &Db, now: i64) -> Result<Vec<(String, String)>, SiftError> {
    let due: Vec<(String, String, bool)> = db
        .read(move |c| {
            Ok(c.prepare(
                "SELECT account_id,thread_id,wake_unread FROM snoozes \
                 WHERE state='sleeping' AND wake_at<=?",
            )?
            .query_map(rusqlite::params![now], |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)? != 0))
            })?
            .collect::<Result<Vec<_>, _>>()?)
        })
        .await
        .map_err(db_error)?;
    if due.is_empty() {
        return Ok(vec![]);
    }
    let mut accounts: Vec<String> = due.iter().map(|(a, _, _)| a.clone()).collect();
    accounts.sort();
    accounts.dedup();
    let mut woken: Vec<(String, String)> = vec![];
    for account_id in accounts {
        let scoped: Vec<(String, bool)> = due
            .iter()
            .filter(|(a, _, _)| *a == account_id)
            .map(|(_, t, unread)| (t.clone(), *unread))
            .collect();
        let account = account_id.clone();
        let result = db
            .write_tx(move |tx| wake_account(tx, &account, &scoped))
            .await;
        match result {
            Ok(threads) => {
                for thread in threads {
                    woken.push((account_id.clone(), thread));
                }
            }
            Err(e) => {
                log::warn!("snooze wake for {account_id} failed: {e}");
            }
        }
    }
    Ok(woken)
}

fn wake_account(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
    due: &[(String, bool)],
) -> anyhow::Result<Vec<String>> {
    let label_id: Option<String> = tx
        .query_row(
            "SELECT id FROM labels WHERE account_id=? AND name=? LIMIT 1",
            rusqlite::params![account_id, SNOOZE_LABEL_NAME],
            |r| r.get(0),
        )
        .ok();
    let mut woken: Vec<String> = vec![];
    for (thread_id, wake_unread) in due {
        let mut add: Vec<String> = vec!["INBOX".into()];
        if *wake_unread {
            add.push("UNREAD".into());
        }
        let mut remove: Vec<String> = vec![];
        if let Some(label_id) = &label_id {
            remove.push(label_id.clone());
        }
        let mut ids: Vec<String> = vec![];
        let mut previous = PreviousState {
            touched: vec!["INBOX".into(), "UNREAD".into(), SNOOZE_LABEL_NAME.into()],
            messages: vec![],
        };
        for id in message_ids_conn(tx, account_id, thread_id)? {
            previous.messages.push(prior_of(tx, account_id, &id)?);
            apply_labels(tx, account_id, &id, &add, &remove)?;
            ids.push(id);
        }
        tx.execute(
            "DELETE FROM snoozes WHERE account_id=? AND thread_id=?",
            rusqlite::params![account_id, thread_id],
        )?;
        tx.execute(
            "UPDATE threads SET snoozed_until=NULL WHERE account_id=? AND id=?",
            rusqlite::params![account_id, thread_id],
        )?;
        crate::db::threads::recompute_thread_conn(tx, account_id, thread_id)?;
        if !ids.is_empty() {
            // The exact label set the local rows now have: Inbox comes back
            // (plus Unread when the policy asks for it) and the Snoozed label
            // goes away.
            let provider_add: Vec<String> = add.clone();
            let provider_remove: Vec<String> = vec![format!("name:{SNOOZE_LABEL_NAME}")];
            let payload = serde_json::json!({
                "ids": ids,
                "add": provider_add,
                "remove": provider_remove,
            })
            .to_string();
            let op = NewOp {
                account_id: account_id.to_string(),
                kind: "modify_labels".into(),
                payload,
                not_before: 0,
                previous_state_json: previous.to_json(),
                summary_action: Some("Waking snoozed mail".into()),
                ..Default::default()
            };
            crate::db::outbox::insert_op(tx, &op)?;
        }
        woken.push(thread_id.clone());
    }
    Ok(woken)
}

/// Wake threads that received a new incoming message while asleep (P6.5).
///
/// A reply is the strongest signal that a snoozed conversation is live again,
/// so the thread wakes by the same policy as a due timer.
pub async fn wake_threads_with_new_mail(
    db: &Db,
    account_id: &str,
    changed_threads: &[String],
) -> Result<Vec<String>, SiftError> {
    if changed_threads.is_empty() {
        return Ok(vec![]);
    }
    let account = account_id.to_string();
    let threads = changed_threads.to_vec();
    let due: Vec<(String, bool)> = db
        .read(move |c| {
            let mut s = c.prepare(
                "SELECT s.thread_id, s.wake_unread FROM snoozes s \
                 WHERE s.account_id=?1 AND s.state='sleeping' AND s.thread_id=?2 \
                   AND EXISTS (SELECT 1 FROM messages m WHERE m.account_id=?1 AND m.thread_id=s.thread_id \
                               AND m.is_draft=0 AND m.is_sent_by_me=0 \
                               AND m.internal_date > COALESCE(s.created_at, 0))",
            )?;
            let mut out = vec![];
            for thread in &threads {
                let row: Option<(String, i64)> = s
                    .query_row(rusqlite::params![account, thread], |r| {
                        Ok((r.get(0)?, r.get(1)?))
                    })
                    .ok();
                if let Some((thread_id, unread)) = row {
                    out.push((thread_id, unread != 0));
                }
            }
            Ok(out)
        })
        .await
        .map_err(db_error)?;
    if due.is_empty() {
        return Ok(vec![]);
    }
    let account = account_id.to_string();
    let result = db
        .write_tx(move |tx| wake_account(tx, &account, &due))
        .await
        .map_err(db_error)?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::messages::MsgUpsert;

    async fn seed(db: &Db, account: &str, thread: &str, id: &str, inbox: bool) {
        db.messages_upsert(MsgUpsert {
            id: id.to_string(),
            account_id: account.to_string(),
            thread_id: thread.to_string(),
            internal_date: 1,
            subject: "s".into(),
            is_unread: true,
            label_ids: if inbox {
                vec!["INBOX".into(), "UNREAD".into()]
            } else {
                vec!["UNREAD".into()]
            },
            ..Default::default()
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn p6_5_snooze_sets_the_timer_and_labels_in_one_step() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let acc = db.new_account("a@x.com", None, None).await.unwrap();
        seed(&db, &acc.id, "t1", "m1", true).await;
        let targets = vec![GestureTarget {
            account_id: acc.id.clone(),
            thread_id: "t1".into(),
        }];
        let (outcome, failures) = snooze_set(&db, "g1", &targets, 42, false)
            .await
            .unwrap();
        assert!(failures.is_empty());
        assert_eq!(outcome.operations.len(), 1);
        // The label change waits for the label to exist remotely.
        assert!(outcome.operations[0].depends_on_op_id.is_some());
        assert_eq!(next_deadline(&db).await.unwrap(), Some(42));

        // The label was actually attached, and Inbox was removed — the two
        // things the old implementation forgot.
        let raw: String = db
            .read({
                let a = acc.id.clone();
                move |c| {
                    Ok(c.query_row(
                        "SELECT label_ids FROM messages WHERE account_id=? AND id='m1'",
                        rusqlite::params![a],
                        |r| r.get::<_, String>(0),
                    )?)
                }
            })
            .await
            .unwrap();
        let labels: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
        assert!(labels.iter().any(|l| l.contains("Sift/Snoozed")), "{labels:?}");
        assert!(!labels.iter().any(|l| l == "INBOX"), "{labels:?}");
    }

    #[tokio::test]
    async fn p6_5_wake_restores_inbox_and_records_the_operation() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let acc = db.new_account("a@x.com", None, None).await.unwrap();
        seed(&db, &acc.id, "t1", "m1", true).await;
        let targets = vec![GestureTarget {
            account_id: acc.id.clone(),
            thread_id: "t1".into(),
        }];
        snooze_set(&db, "g1", &targets, 10, false).await.unwrap();
        let woken = wake_due(&db, 11).await.unwrap();
        assert_eq!(woken.len(), 1);
        assert_eq!(next_deadline(&db).await.unwrap(), None);
        let raw: String = db
            .read({
                let a = acc.id.clone();
                move |c| {
                    Ok(c.query_row(
                        "SELECT label_ids FROM messages WHERE account_id=? AND id='m1'",
                        rusqlite::params![a],
                        |r| r.get::<_, String>(0),
                    )?)
                }
            })
            .await
            .unwrap();
        let labels: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
        assert!(labels.iter().any(|l| l == "INBOX"), "{labels:?}");
    }
}
