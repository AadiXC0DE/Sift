//! The account-qualified action service (P6.3, P6.4).
//!
//! One UI gesture — however many accounts and threads it covers — is one call.
//! Per account it runs exactly one write transaction that
//!
//! 1. captures each touched message's affected label/read/star bits,
//! 2. applies only the *real* diffs (an add for a label that is already there
//!    is not a change, and neither is removing one that is absent),
//! 3. recomputes each affected thread once,
//! 4. queues the exact provider operation, carrying the per-message prior
//!    values an Undo has to restore.
//!
//! The store event is emitted by the command after this returns, i.e. after
//! the commit — never before an inverse has been written.
//!
//! Undo rules (P6.3):
//!
//! * a `pending` operation is cancelled and its prior values applied
//!   atomically — nothing left the machine;
//! * a `done` operation is compensated with a new operation carrying the exact
//!   inverse diffs, computed against the *current* state so a change that
//!   arrived from another client in the meantime is not overwritten;
//! * an `inflight`/`uncertain` operation cannot be compensated yet: the
//!   compensation is queued behind it and waits for its resolution.

use crate::db::outbox::{NewOp, Op, STATE_PENDING};
use crate::db::Db;
use crate::dto::{ActionKind, GestureTarget};
use crate::errors::SiftError;

/// The labels a gesture touches, in the vocabulary the provider understands.
///
/// `add`/`remove` are label ids (or `name:<name>` references resolved at claim
/// time); `trash`/`spam`/`delete` select a different provider path.
#[derive(Debug, Clone, Default)]
pub struct LabelSets {
    pub add: Vec<String>,
    pub remove: Vec<String>,
    pub delete_forever: bool,
}

impl LabelSets {
    /// Labels this gesture may touch, used to bound what an Undo restores.
    pub fn touched(&self) -> Vec<String> {
        let mut out: Vec<String> = self.add.clone();
        for l in &self.remove {
            if !out.contains(l) {
                out.push(l.clone());
            }
        }
        out
    }
}

pub fn label_sets(action: &ActionKind) -> LabelSets {
    let (add, remove, delete) = match action {
        ActionKind::Archive => (vec![], vec!["INBOX".into()], false),
        ActionKind::Unarchive => (vec!["INBOX".into()], vec![], false),
        ActionKind::Trash => (vec!["TRASH".into()], vec!["INBOX".into()], false),
        ActionKind::Untrash => (vec!["INBOX".into()], vec!["TRASH".into()], false),
        ActionKind::Spam => (vec!["SPAM".into()], vec!["INBOX".into()], false),
        ActionKind::Unspam => (vec!["INBOX".into()], vec!["SPAM".into()], false),
        ActionKind::DeleteForever => (vec![], vec![], true),
        ActionKind::Star { on } => (
            if *on { vec!["STARRED".into()] } else { vec![] },
            if *on { vec![] } else { vec!["STARRED".into()] },
            false,
        ),
        ActionKind::Read { on } => (
            if *on { vec![] } else { vec!["UNREAD".into()] },
            if *on { vec!["UNREAD".into()] } else { vec![] },
            false,
        ),
        ActionKind::AddLabel { label_id } => (vec![label_id.clone()], vec![], false),
        ActionKind::RemoveLabel { label_id } => (vec![], vec![label_id.clone()], false),
        ActionKind::MoveTo { label_id } => (vec![label_id.clone()], vec!["INBOX".into()], false),
    };
    LabelSets {
        add,
        remove,
        delete_forever: delete,
    }
}

/// A message's prior affected values, captured before a gesture changed them.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PriorMessage {
    pub id: String,
    pub labels: Vec<String>,
    pub unread: bool,
    pub starred: bool,
}

/// Per-message previous state for one operation (P6.3). Stored as JSON on the
/// operation, so an Undo restores exactly what the gesture changed instead of
/// a broad inverse.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct PreviousState {
    #[serde(default)]
    pub touched: Vec<String>,
    #[serde(default)]
    pub messages: Vec<PriorMessage>,
}

impl PreviousState {
    pub fn to_json(&self) -> Option<String> {
        if self.messages.is_empty() {
            return None;
        }
        serde_json::to_string(self).ok()
    }

    pub fn parse(raw: Option<&str>) -> Self {
        raw.and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or_default()
    }

    /// Was `label` present on this message before the gesture?
    pub fn had(label: &str, msg: &PriorMessage) -> bool {
        msg.labels.iter().any(|l| l == label)
    }
}

fn db_error(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
}

/// Read `(id, thread_id)` for a target thread. Star gestures touch only the
/// newest message, which is the Gmail "star this conversation" semantic.
fn message_ids_conn(
    conn: &rusqlite::Connection,
    account_id: &str,
    thread_id: &str,
    latest_only: bool,
) -> anyhow::Result<Vec<String>> {
    if latest_only {
        let one: Option<String> = conn
            .query_row(
                "SELECT id FROM messages WHERE account_id=? AND thread_id=? \
                 ORDER BY internal_date DESC, id DESC LIMIT 1",
                rusqlite::params![account_id, thread_id],
                |r| r.get(0),
            )
            .ok();
        return Ok(one.into_iter().collect());
    }
    let mut s = conn.prepare(
        "SELECT id FROM messages WHERE account_id=? AND thread_id=? ORDER BY internal_date, id",
    )?;
    let rows: Vec<String> = s
        .query_map(rusqlite::params![account_id, thread_id], |r| r.get(0))?
        .collect::<Result<Vec<String>, _>>()?;
    Ok(rows)
}

/// Apply a label diff to one message and refresh its denormalized flags.
fn apply_diff_conn(
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
    refresh_message_flags(conn, account_id, message_id)
}

/// Recompute the denormalized flags and the label list of one message.
fn refresh_message_flags(
    conn: &rusqlite::Connection,
    account_id: &str,
    message_id: &str,
) -> anyhow::Result<()> {
    let has = |lid: &str| -> rusqlite::Result<bool> {
        conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM message_labels WHERE account_id=? AND message_id=? AND label_id=?)",
            rusqlite::params![account_id, message_id, lid],
            |r| r.get(0),
        )
    };
    let unread = has("UNREAD")?;
    let starred = has("STARRED")?;
    let draft = has("DRAFT")?;
    let labels: Vec<String> = conn
        .prepare("SELECT label_id FROM message_labels WHERE account_id=? AND message_id=? ORDER BY label_id")?
        .query_map(rusqlite::params![account_id, message_id], |r| r.get(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let lj = serde_json::to_string(&labels).unwrap_or_else(|_| "[]".into());
    conn.execute(
        "UPDATE messages SET is_unread=?, is_starred=?, is_draft=?, label_ids=? \
         WHERE account_id=? AND id=?",
        rusqlite::params![unread as i32, starred as i32, draft as i32, lj, account_id, message_id],
    )?;
    Ok(())
}

/// The prior values of one message, read inside the gesture transaction.
fn read_prior(
    conn: &rusqlite::Connection,
    account_id: &str,
    message_id: &str,
) -> anyhow::Result<PriorMessage> {
    let (labels_json, unread, starred): (String, i64, i64) = conn.query_row(
        "SELECT label_ids, is_unread, is_starred FROM messages WHERE account_id=? AND id=?",
        rusqlite::params![account_id, message_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    Ok(PriorMessage {
        id: message_id.to_string(),
        labels: serde_json::from_str(&labels_json).unwrap_or_default(),
        unread: unread != 0,
        starred: starred != 0,
    })
}

/// What one account's slice of a gesture did.
pub struct AccountOutcome {
    pub op: Option<Op>,
    pub thread_ids: Vec<String>,
}

/// Apply one gesture for one account inside a single transaction.
fn apply_account_gesture(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
    targets: &[GestureTarget],
    sets: &LabelSets,
    gesture_id: &str,
    latest_only: bool,
) -> anyhow::Result<Option<i64>> {
    let touched = sets.touched();
    let mut real_ids: Vec<String> = vec![];
    let mut previous = PreviousState {
        touched: touched.clone(),
        messages: vec![],
    };
    let mut threads: Vec<String> = vec![];
    for target in targets {
        let ids = message_ids_conn(tx, account_id, &target.thread_id, latest_only)?;
        let mut changed_thread = false;
        for id in ids {
            let prior = read_prior(tx, account_id, &id)?;
            let mut add: Vec<String> = vec![];
            let mut remove: Vec<String> = vec![];
            for l in &sets.add {
                if !prior.labels.iter().any(|x| x == l) {
                    add.push(l.clone());
                }
            }
            for l in &sets.remove {
                if prior.labels.iter().any(|x| x == l) {
                    remove.push(l.clone());
                }
            }
            if add.is_empty() && remove.is_empty() {
                continue; // already in the requested state: not a change
            }
            apply_diff_conn(tx, account_id, &id, &add, &remove)?;
            if !real_ids.contains(&id) {
                real_ids.push(id.clone());
            }
            previous.messages.push(prior);
            changed_thread = true;
        }
        if changed_thread {
            threads.push(target.thread_id.clone());
        }
    }
    threads.sort();
    threads.dedup();
    for thread_id in &threads {
        crate::db::threads::recompute_thread_conn(tx, account_id, thread_id)?;
    }
    if real_ids.is_empty() {
        return Ok(None);
    }
    // The provider operation carries only the real diff, so a replay can never
    // touch a message the gesture did not change.
    let mut add: Vec<String> = vec![];
    let mut remove: Vec<String> = vec![];
    for l in sets.add.iter().chain(sets.remove.iter()) {
        let present_before = previous
            .messages
            .iter()
            .any(|m| PreviousState::had(l, m));
        if sets.add.contains(l) && !present_before {
            if !add.contains(l) {
                add.push(l.clone());
            }
        } else if sets.remove.contains(l) && present_before && !remove.contains(l) {
            remove.push(l.clone());
        }
    }
    let payload = serde_json::json!({"ids": real_ids, "add": add, "remove": remove}).to_string();
    let op = NewOp {
        account_id: account_id.to_string(),
        kind: "modify_labels".into(),
        payload,
        undo_group: Some(gesture_id.to_string()),
        not_before: 0,
        previous_state_json: previous.to_json(),
        ..Default::default()
    };
    let queued = match crate::db::outbox::coalesce_label_op(tx, &op)? {
        Some(id) => id,
        None => crate::db::outbox::insert_op(tx, &op)?.id,
    };
    Ok(Some(queued))
}

/// The result of one gesture: one operation per account that had real work.
#[derive(Debug, Clone, Default)]
pub struct GestureOutcome {
    pub operations: Vec<Op>,
    pub thread_ids: Vec<String>,
}

/// Apply a multi-account gesture. Every account commits on its own, so one
/// account's failure cannot roll back another's already-committed change; the
/// caller reports the partial failure (P6.3).
pub async fn apply_gesture(
    db: &Db,
    gesture_id: &str,
    targets: &[GestureTarget],
    action: &ActionKind,
) -> Result<(GestureOutcome, Vec<GestureFailure>), SiftError> {
    let sets = label_sets(action);
    let latest_only = matches!(action, ActionKind::Star { .. });
    if sets.delete_forever {
        return delete_forever(db, gesture_id, targets).await;
    }
    let mut accounts: Vec<String> = targets.iter().map(|t| t.account_id.clone()).collect();
    accounts.sort();
    accounts.dedup();

    let mut outcome = GestureOutcome::default();
    let mut failures: Vec<GestureFailure> = vec![];
    for account_id in accounts {
        let scoped: Vec<GestureTarget> = targets
            .iter()
            .filter(|t| t.account_id == account_id)
            .cloned()
            .collect();
        let account = account_id.clone();
        let group = gesture_id.to_string();
        let sets = sets.clone();
        let result = db
            .write_tx(move |tx| {
                let ids: Vec<String> = scoped.iter().map(|t| t.thread_id.clone()).collect();
                let op = apply_account_gesture(tx, &account, &scoped, &sets, &group, latest_only)?;
                Ok((op, ids))
            })
            .await;
        match result {
            Ok((op_id, thread_ids)) => {
                outcome.thread_ids.extend(thread_ids);
                if let Some(op_id) = op_id {
                    if let Some(op) = db.outbox_get(op_id).await.map_err(db_error)? {
                        outcome.operations.push(op);
                    }
                }
            }
            // A database error before the enqueue leaves no optimistic
            // mutation: the transaction rolled back everything it wrote.
            Err(e) => failures.push(GestureFailure {
                account_id,
                code: "db".into(),
                message: e.to_string(),
            }),
        }
    }
    Ok((outcome, failures))
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct GestureFailure {
    #[serde(rename = "accountId")]
    pub account_id: String,
    pub code: String,
    pub message: String,
}

/// Undo one gesture across every account it touched.
///
/// Returns the compensating/restored operations plus the per-account failures,
/// so the UI can say "undone for 2 of 3 accounts" instead of lying about all
/// of them.
pub async fn undo_gesture(
    db: &Db,
    gesture_id: &str,
) -> Result<(Vec<Op>, Vec<GestureFailure>), SiftError> {
    let open = db.outbox_open_group(gesture_id).await.map_err(db_error)?;
    let done = db.outbox_done_group(gesture_id).await.map_err(db_error)?;

    let mut operations: Vec<Op> = vec![];
    let mut failures: Vec<GestureFailure> = vec![];

    // Pending: cancel and restore the exact previous values, atomically. This
    // also clears any snooze timer the same gesture set, so an undone snooze
    // is fully gone.
    for op in open.iter().filter(|o| o.state == STATE_PENDING) {
        let op = op.clone();
        let group = gesture_id.to_string();
        let account_id_for_failure = op.account_id.clone();
        let result = db
            .write_tx(move |tx| {
                let changed = tx.execute(
                    "UPDATE outbox_ops SET state='cancelled', completed_at=?1, failure_code='undone' \
                     WHERE id=?2 AND state='pending'",
                    rusqlite::params![crate::db::now_ms(), op.id],
                )?;
                if changed == 0 {
                    // Another actor claimed it first: leave it to the
                    // inflight branch below.
                    return Ok(false);
                }
                restore_previous_state(tx, &op.account_id, op.previous_state_json.as_deref())?;
                tx.execute(
                    "UPDATE outbox_ops SET state='cancelled', completed_at=?1, failure_code='dependency_failed' \
                     WHERE depends_on_op_id=?2 AND state='pending'",
                    rusqlite::params![crate::db::now_ms(), op.id],
                )?;
                let _ = &group;
                Ok(true)
            })
            .await;
        match result {
            Ok(true) => {
                if let Some(row) = db.outbox_get(op.id).await.map_err(db_error)? {
                    operations.push(row);
                }
            }
            Ok(false) => {}
            Err(e) => failures.push(GestureFailure {
                account_id: account_id_for_failure,
                code: "db".into(),
                message: e.to_string(),
            }),
        }
    }

    // In flight or uncertain: the compensation waits for the resolution of the
    // operation it depends on. It is not applied locally yet — the operation
    // may still land, and applying the inverse early would show a state that
    // is not true.
    for op in open
        .iter()
        .filter(|o| o.state == "inflight" || o.state == "uncertain")
    {
        let op = op.clone();
        let prior = PreviousState::parse(op.previous_state_json.as_deref());
        if prior.messages.is_empty() {
            continue;
        }
        let compensation = compensation_payload(db, &op.account_id, &prior).await?;
        let Some((payload, inverse_previous)) = compensation else {
            continue;
        };
        let queued = NewOp {
            account_id: op.account_id.clone(),
            kind: "modify_labels".into(),
            payload,
            not_before: 0,
            previous_state_json: inverse_previous,
            depends_on_op_id: Some(op.id),
            summary_action: Some("Undoing".into()),
            ..Default::default()
        };
        let id = db.outbox_enqueue_label_diff(&queued).await.map_err(db_error)?.id;
        if let Some(row) = db.outbox_get(id).await.map_err(db_error)? {
            operations.push(row);
        }
    }

    // Done: enqueue the exact inverse diffs and apply them locally in the same
    // transaction. The store event is emitted by the caller, after this commit.
    for op in done.iter() {
        let op = op.clone();
        let prior = PreviousState::parse(op.previous_state_json.as_deref());
        if prior.messages.is_empty() {
            failures.push(GestureFailure {
                account_id: op.account_id.clone(),
                code: "no_previous_state".into(),
                message: "this operation does not record what it changed, so Sift will not guess".into(),
            });
            continue;
        }
        let compensation = compensation_payload(db, &op.account_id, &prior).await?;
        let Some((payload, inverse_previous)) = compensation else {
            continue; // nothing left to undo: the state already matches
        };
        let queued = NewOp {
            account_id: op.account_id.clone(),
            kind: "modify_labels".into(),
            payload: payload.clone(),
            not_before: 0,
            previous_state_json: inverse_previous,
            summary_action: Some("Undoing".into()),
            ..Default::default()
        };
        let account = op.account_id.clone();
        let result = db
            .write_tx(move |tx| {
                apply_payload_locally(tx, &account, &payload)?;
                Ok(crate::db::outbox::insert_op(tx, &queued)?.id)
            })
            .await;
        match result {
            Ok(id) => {
                if let Some(row) = db.outbox_get(id).await.map_err(db_error)? {
                    operations.push(row);
                }
            }
            Err(e) => failures.push(GestureFailure {
                account_id: op.account_id.clone(),
                code: "db".into(),
                message: e.to_string(),
            }),
        }
    }

    // An undone snooze has no timer either.
    clear_gesture_snoozes(db, gesture_id).await?;
    Ok((operations, failures))
}

/// Delete the timers a gesture created (Undo of a snooze).
async fn clear_gesture_snoozes(db: &Db, gesture_id: &str) -> Result<(), SiftError> {
    let group = gesture_id.to_string();
    let _ = db
        .write_tx(move |tx| {
            let threads: Vec<(String, String)> = tx
                .prepare("SELECT account_id,thread_id FROM snoozes WHERE gesture_id=?")?
                .query_map(rusqlite::params![group], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            for (account_id, thread_id) in threads {
                tx.execute(
                    "DELETE FROM snoozes WHERE account_id=? AND thread_id=?",
                    rusqlite::params![account_id, thread_id],
                )?;
                tx.execute(
                    "UPDATE threads SET snoozed_until=NULL WHERE account_id=? AND id=?",
                    rusqlite::params![account_id, thread_id],
                )?;
            }
            Ok(())
        })
        .await;
    Ok(())
}

/// Restore the exact previous values recorded for a cancelled operation.
fn restore_previous_state(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
    raw: Option<&str>,
) -> anyhow::Result<()> {
    let prior = PreviousState::parse(raw);
    if prior.messages.is_empty() {
        return Ok(());
    }
    let mut threads: Vec<String> = vec![];
    for msg in &prior.messages {
        let Some(current) = current_labels(tx, account_id, &msg.id)? else {
            continue; // the message is gone: nothing to restore
        };
        let mut add: Vec<String> = vec![];
        let mut remove: Vec<String> = vec![];
        for label in &prior.touched {
            let had = msg.labels.iter().any(|l| l == label);
            let has = current.iter().any(|l| l == label);
            if had && !has {
                add.push(label.clone());
            } else if !had && has {
                remove.push(label.clone());
            }
        }
        if add.is_empty() && remove.is_empty() {
            continue;
        }
        apply_diff_conn(tx, account_id, &msg.id, &add, &remove)?;
        if let Some(thread) = thread_of(tx, account_id, &msg.id)? {
            threads.push(thread);
        }
    }
    threads.sort();
    threads.dedup();
    for thread in &threads {
        crate::db::threads::recompute_thread_conn(tx, account_id, thread)?;
    }
    Ok(())
}

fn current_labels(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
    message_id: &str,
) -> anyhow::Result<Option<Vec<String>>> {
    let raw: Option<String> = tx
        .query_row(
            "SELECT label_ids FROM messages WHERE account_id=? AND id=?",
            rusqlite::params![account_id, message_id],
            |r| r.get(0),
        )
        .ok();
    Ok(raw.map(|s| serde_json::from_str(&s).unwrap_or_default()))
}

fn thread_of(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
    message_id: &str,
) -> anyhow::Result<Option<String>> {
    Ok(tx
        .query_row(
            "SELECT thread_id FROM messages WHERE account_id=? AND id=?",
            rusqlite::params![account_id, message_id],
            |r| r.get(0),
        )
        .ok())
}

/// The inverse diff for one operation, computed against the *current* state:
/// only the labels the gesture actually touched are considered, so a change
/// another client made in the meantime survives (P6.3).
async fn compensation_payload(
    db: &Db,
    account_id: &str,
    prior: &PreviousState,
) -> Result<Option<(String, Option<String>)>, SiftError> {
    let account = account_id.to_string();
    let prior = prior.clone();
    db.read(move |c| {
        let mut ids: Vec<String> = vec![];
        let mut add: Vec<String> = vec![];
        let mut remove: Vec<String> = vec![];
        let mut applied: Vec<PriorMessage> = vec![];
        for msg in &prior.messages {
            let raw: Option<String> = c
                .query_row(
                    "SELECT label_ids FROM messages WHERE account_id=? AND id=?",
                    rusqlite::params![account, msg.id],
                    |r| r.get(0),
                )
                .ok();
            let Some(raw) = raw else { continue };
            let current: Vec<String> = serde_json::from_str(&raw).unwrap_or_default();
            let mut changed = false;
            for label in &prior.touched {
                let had = msg.labels.iter().any(|l| l == label);
                let has = current.iter().any(|l| l == label);
                if had && !has {
                    changed = true;
                    if !add.contains(label) {
                        add.push(label.clone());
                    }
                } else if !had && has {
                    changed = true;
                    if !remove.contains(label) {
                        remove.push(label.clone());
                    }
                }
            }
            if changed {
                ids.push(msg.id.clone());
                // The inverse's own previous state is the *current* one, so the
                // compensation is itself accurately undoable.
                applied.push(PriorMessage {
                    id: msg.id.clone(),
                    labels: current,
                    unread: false,
                    starred: false,
                });
            }
        }
        if ids.is_empty() {
            return Ok(None);
        }
        let payload = serde_json::json!({"ids": ids, "add": add, "remove": remove}).to_string();
        let previous = PreviousState {
            touched: prior.touched.clone(),
            messages: applied,
        };
        Ok(Some((payload, previous.to_json())))
    })
    .await
    .map_err(db_error)
}

/// Apply a `modify_labels` payload to the local rows (used for the optimistic
/// half of a compensation, in the same transaction as its enqueue).
fn apply_payload_locally(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
    payload: &str,
) -> anyhow::Result<()> {
    let v: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
    let ids: Vec<String> = serde_json::from_value(v["ids"].clone()).unwrap_or_default();
    let add: Vec<String> = serde_json::from_value(v["add"].clone()).unwrap_or_default();
    let remove: Vec<String> = serde_json::from_value(v["remove"].clone()).unwrap_or_default();
    let mut threads: Vec<String> = vec![];
    for id in &ids {
        apply_diff_conn(tx, account_id, id, &add, &remove)?;
        if let Some(thread) = thread_of(tx, account_id, id)? {
            threads.push(thread);
        }
    }
    threads.sort();
    threads.dedup();
    for thread in &threads {
        crate::db::threads::recompute_thread_conn(tx, account_id, thread)?;
    }
    Ok(())
}

/// Permanent deletion (P6.4).
///
/// The immutable identity list — every targeted message, its original
/// location and the UIDVALIDITY epoch that location belonged to — is queued
/// *before* the display rows go away. Removing the rows first is exactly how
/// the old implementation lost the remote ids and turned the deletion into a
/// no-op.
async fn delete_forever(
    db: &Db,
    gesture_id: &str,
    targets: &[GestureTarget],
) -> Result<(GestureOutcome, Vec<GestureFailure>), SiftError> {
    let mut accounts: Vec<String> = targets.iter().map(|t| t.account_id.clone()).collect();
    accounts.sort();
    accounts.dedup();
    let mut outcome = GestureOutcome::default();
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
            .write_tx(move |tx| delete_forever_account(tx, &account, &scoped, &group))
            .await;
        match result {
            Ok((op_id, thread_ids)) => {
                outcome.thread_ids.extend(thread_ids);
                if let Some(id) = op_id {
                    if let Some(op) = db.outbox_get(id).await.map_err(db_error)? {
                        outcome.operations.push(op);
                    }
                }
            }
            Err(e) => {
                let message = e.to_string();
                let code = if message.contains("not in Trash or Spam") {
                    "not_in_trash".into()
                } else {
                    "db".into()
                };
                failures.push(GestureFailure {
                    account_id,
                    code,
                    message,
                });
            }
        }
    }
    Ok((outcome, failures))
}

fn delete_forever_account(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
    targets: &[GestureTarget],
    gesture_id: &str,
) -> anyhow::Result<(Option<i64>, Vec<String>)> {
    let mut messages: Vec<serde_json::Value> = vec![];
    let mut cache_paths: Vec<String> = vec![];
    let mut thread_ids: Vec<String> = vec![];
    for target in targets {
        let ids = message_ids_conn(tx, account_id, &target.thread_id, false)?;
        if ids.is_empty() {
            continue;
        }
        let mut any = false;
        for id in &ids {
            let labels: Vec<String> = tx
                .query_row(
                    "SELECT label_ids FROM messages WHERE account_id=? AND id=?",
                    rusqlite::params![account_id, id],
                    |r| r.get::<_, String>(0),
                )
                .map(|s| serde_json::from_str(&s).unwrap_or_default())
                .unwrap_or_default();
            // Membership is validated per message: a thread can hold one
            // message in Trash and another in the inbox.
            if !labels.iter().any(|l| l == "TRASH" || l == "SPAM") {
                anyhow::bail!("a message is not in Trash or Spam");
            }
            let rfc: Option<String> = tx
                .query_row(
                    "SELECT rfc_message_id FROM messages WHERE account_id=? AND id=?",
                    rusqlite::params![account_id, id],
                    |r| r.get(0),
                )
                .unwrap_or(None);
            let mut attached: Vec<String> = tx
                .prepare("SELECT local_path FROM attachments WHERE account_id=? AND message_id=? AND local_path IS NOT NULL")?
                .query_map(rusqlite::params![account_id, id], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            for path in attached.drain(..) {
                if !cache_paths.contains(&path) {
                    cache_paths.push(path);
                }
            }
            // Original location and epoch, preferred Trash > Junk > anywhere.
            // These are hints: the provider re-resolves unless the epoch still
            // matches, so a stale UID can never address another message.
            let loc: Option<(String, i64, i64)> = tx
                .query_row(
                    "SELECT u.role, u.uid, f.uidvalidity FROM imap_uids u \
                     JOIN imap_folders f ON f.account_id=u.account_id AND f.role=u.role \
                     WHERE u.account_id=? AND u.message_id=? \
                     ORDER BY CASE u.role WHEN 'trash' THEN 0 WHEN 'junk' THEN 1 ELSE 2 END \
                     LIMIT 1",
                    rusqlite::params![account_id, id],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .ok();
            let (role, uid, uidvalidity) = match loc {
                Some((role, uid, epoch)) => (
                    serde_json::Value::String(role),
                    serde_json::Value::from(uid),
                    serde_json::Value::from(epoch),
                ),
                None => (
                    serde_json::Value::Null,
                    serde_json::Value::Null,
                    serde_json::Value::Null,
                ),
            };
            messages.push(serde_json::json!({
                "id": id,
                "rfcMessageId": rfc,
                "role": role,
                "uid": uid,
                "uidvalidity": uidvalidity,
            }));
            any = true;
        }
        if any {
            thread_ids.push(target.thread_id.clone());
        }
    }
    if messages.is_empty() {
        return Ok((None, vec![]));
    }
    let payload = serde_json::json!({
        "messages": messages,
        "cachePaths": cache_paths,
    })
    .to_string();
    let queued = NewOp {
        account_id: account_id.to_string(),
        kind: "delete".into(),
        payload,
        undo_group: None, // a confirmed permanent deletion promises no undo
        not_before: 0,
        summary_action: Some("Deleting permanently".into()),
        summary_subject: Some(format!("{} messages", messages.len())),
        ..Default::default()
    };
    let op_id = crate::db::outbox::insert_op(tx, &queued)?.id;
    // Only now are the display rows removed: the operation owns the identities.
    for target in targets {
        tx.execute(
            "DELETE FROM messages WHERE account_id=? AND thread_id=?",
            rusqlite::params![account_id, target.thread_id],
        )?;
        tx.execute(
            "DELETE FROM threads WHERE account_id=? AND id=?",
            rusqlite::params![account_id, target.thread_id],
        )?;
    }
    let _ = gesture_id;
    Ok((Some(op_id), thread_ids))
}

/// Remove the cached attachment files of a completed permanent deletion.
///
/// They were kept until the server acknowledged (P6.4): a failed or uncertain
/// deletion must stay recoverable, and the files are part of that.
pub async fn cleanup_deleted_cache(db: &Db, paths: &[String]) {
    for path in paths {
        let owned = path.clone();
        let referenced = db
            .read(move |c| {
                Ok(c.query_row(
                    "SELECT count(*) FROM attachments WHERE local_path=?",
                    rusqlite::params![owned],
                    |r| r.get::<_, i64>(0),
                )?)
            })
            .await
            .unwrap_or(1);
        if referenced == 0 {
            let _ = tokio::fs::remove_file(path).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::messages::MsgUpsert;
    use crate::dto::ActionKind;

    async fn seed(db: &Db, account: &str, thread: &str, ids: &[(&str, bool)]) {
        for (id, in_inbox) in ids {
            db.messages_upsert(MsgUpsert {
                id: (*id).to_string(),
                account_id: account.to_string(),
                thread_id: thread.to_string(),
                internal_date: 1,
                subject: "s".into(),
                is_unread: true,
                label_ids: if *in_inbox {
                    vec!["INBOX".into(), "UNREAD".into()]
                } else {
                    vec!["UNREAD".into()]
                },
                ..Default::default()
            })
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn p6_3_mixed_previous_state_is_restored_exactly() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let acc = db.new_account("a@x.com", None, None).await.unwrap();
        seed(&db, &acc.id, "t1", &[("m1", true), ("m2", false), ("m3", true)]).await;
        let targets = vec![GestureTarget {
            account_id: acc.id.clone(),
            thread_id: "t1".into(),
        }];
        let (outcome, failures) =
            apply_gesture(&db, "g1", &targets, &ActionKind::Archive).await.unwrap();
        assert!(failures.is_empty());
        assert_eq!(outcome.operations.len(), 1);
        // Only the two messages that were in the inbox were changed.
        let payload = outcome.operations[0].payload_value();
        let ids: Vec<String> = serde_json::from_value(payload["ids"].clone()).unwrap();
        assert_eq!(ids.len(), 2);
        assert!(!ids.contains(&"m2".to_string()), "already archived: no change");

        let (_, failures) = undo_gesture(&db, "g1").await.unwrap();
        assert!(failures.is_empty());
        let labels: String = db
            .read({
                let a = acc.id.clone();
                move |c| {
                    Ok(c.query_row(
                        "SELECT label_ids FROM messages WHERE account_id=? AND id='m1'",
                        rusqlite::params![a],
                        |r| r.get(0),
                    )?)
                }
            })
            .await
            .unwrap();
        let labels: Vec<String> = serde_json::from_str(&labels).unwrap();
        assert!(labels.contains(&"INBOX".to_string()), "prior state restored");
        // m2 was untouched by the gesture and stays untouched by the undo.
        let labels: String = db
            .read({
                let a = acc.id.clone();
                move |c| {
                    Ok(c.query_row(
                        "SELECT label_ids FROM messages WHERE account_id=? AND id='m2'",
                        rusqlite::params![a],
                        |r| r.get(0),
                    )?)
                }
            })
            .await
            .unwrap();
        assert!(!serde_json::from_str::<Vec<String>>(&labels)
            .unwrap()
            .contains(&"INBOX".to_string()));
    }

    #[tokio::test]
    async fn p6_4_permanent_delete_validates_each_message() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let acc = db.new_account("a@x.com", None, None).await.unwrap();
        seed(&db, &acc.id, "t1", &[("m1", true)]).await;
        let targets = vec![GestureTarget {
            account_id: acc.id.clone(),
            thread_id: "t1".into(),
        }];
        let (_, failures) = apply_gesture(&db, "g2", &targets, &ActionKind::DeleteForever)
            .await
            .unwrap();
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].code, "not_in_trash");
        // Nothing was removed locally.
        let count: i64 = db
            .read({
                let a = acc.id.clone();
                move |c| {
                    Ok(c.query_row(
                        "SELECT count(*) FROM messages WHERE account_id=?",
                        rusqlite::params![a],
                        |r| r.get(0),
                    )?)
                }
            })
            .await
            .unwrap();
        assert_eq!(count, 1);
    }
}
