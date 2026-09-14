//! Durable operation state machine for the outbox (P6.1, P6.2, P6.6).
//!
//! Every queued mutation is one row in `outbox_ops` with an explicit state.
//! The transitions are the only way a row moves, and each one is a single
//! conditional `UPDATE` executed on the write lane, so two drainers can never
//! claim one operation and a cancel racing a claim has exactly one winner.
//!
//! ```text
//! Draft editing / Queue      -> pending    freeze revision/MIME, keep draft
//! pending / Undo             -> cancelled  restore editable draft / prior bits
//! pending / atomic claim     -> inflight   started_at persisted before network
//! inflight / provider accept -> done       result ids, dependents released
//! inflight / transient fail  -> pending    backoff, attempts retained
//! inflight / terminal fail   -> failed     recoverable content + error code
//! inflight send / unknown    -> uncertain  reconcile Sent by RFC Message-ID
//! uncertain / sent record    -> done       provider id + evidence
//! uncertain / proof exhausted-> uncertain  user decides (acknowledged retry)
//! failed / user Retry        -> pending    same identity, new explicit attempt
//! ```
//!
//! A cancellation of an in-flight network request is *not* proof the provider
//! cancelled the side effect, which is why `uncertain` exists and why nothing
//! here ever resubmits a send on its own.

use super::Db;
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension, Transaction};

/// How long remote draft synchronization waits for the user to stop typing
/// (P5.2: "debounce remote synchronization to 2 seconds idle").
pub const DRAFT_SYNC_DEBOUNCE_MS: i64 = 2_000;

/// Terminal payload retention (P6.6). Done/cancelled operations keep their
/// summary and counters forever; their payload (which may hold a stale MIME
/// path or a per-message previous-state blob) is dropped after this long.
pub const OP_PAYLOAD_RETENTION_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// Coalescing window for identical idempotent label diffs inside one gesture.
pub const COALESCE_WINDOW_MS: i64 = 200;

pub const STATE_PENDING: &str = "pending";
pub const STATE_INFLIGHT: &str = "inflight";
pub const STATE_UNCERTAIN: &str = "uncertain";
pub const STATE_DONE: &str = "done";
pub const STATE_FAILED: &str = "failed";
pub const STATE_CANCELLED: &str = "cancelled";

/// Bounded reconciliation schedule for an uncertain send: 5s, 30s, 120s and
/// then the user decides (P6.1).
pub const RECONCILE_DELAYS_MS: [i64; 3] = [5_000, 30_000, 120_000];

/// Columns every [`Op`] read selects, in order.
const OP_COLUMNS: &str = "id,account_id,kind,payload,undo_group,state,attempts,not_before,\
     operation_key,draft_id,draft_revision,rfc_message_id,previous_state_json,\
     depends_on_op_id,reconcile_at,reconcile_attempts,failure_code,last_error,result_json,\
     created_at,started_at,completed_at,summary_action,summary_recipient,summary_subject";

#[derive(Debug, Clone)]
pub struct Op {
    pub id: i64,
    pub account_id: String,
    pub kind: String,
    pub payload: String,
    pub undo_group: Option<String>,
    pub state: String,
    pub attempts: i64,
    pub not_before: i64,
    pub operation_key: Option<String>,
    pub draft_id: Option<String>,
    pub draft_revision: Option<i64>,
    pub rfc_message_id: Option<String>,
    pub previous_state_json: Option<String>,
    pub depends_on_op_id: Option<i64>,
    pub reconcile_at: Option<i64>,
    pub reconcile_attempts: i64,
    pub failure_code: Option<String>,
    pub last_error: Option<String>,
    pub result_json: Option<String>,
    pub created_at: i64,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub summary_action: Option<String>,
    pub summary_recipient: Option<String>,
    pub summary_subject: Option<String>,
}

impl Op {
    /// A send whose acceptance is unknown and whose bounded reconciliation is
    /// exhausted: only an explicit, acknowledged retry may move it.
    pub fn requires_duplicate_ack(&self) -> bool {
        self.state == STATE_UNCERTAIN && self.reconcile_attempts >= RECONCILE_DELAYS_MS.len() as i64
    }

    pub fn payload_value(&self) -> serde_json::Value {
        serde_json::from_str(&self.payload).unwrap_or(serde_json::Value::Null)
    }
}

/// A row to insert. Only `account_id`, `kind` and `payload` are required; the
/// rest is the durable lifecycle metadata P6.1 added.
#[derive(Debug, Clone, Default)]
pub struct NewOp {
    pub account_id: String,
    pub kind: String,
    pub payload: String,
    pub undo_group: Option<String>,
    pub not_before: i64,
    pub operation_key: Option<String>,
    pub draft_id: Option<String>,
    pub draft_revision: Option<i64>,
    pub rfc_message_id: Option<String>,
    pub previous_state_json: Option<String>,
    pub depends_on_op_id: Option<i64>,
    pub summary_action: Option<String>,
    pub summary_recipient: Option<String>,
    pub summary_subject: Option<String>,
}

impl NewOp {
    pub fn new(account_id: &str, kind: &str, payload: &str) -> Self {
        Self {
            account_id: account_id.to_string(),
            kind: kind.to_string(),
            payload: payload.to_string(),
            ..Default::default()
        }
    }

    pub fn with_undo_group(mut self, group: Option<&str>) -> Self {
        self.undo_group = group.map(|g| g.to_string());
        self
    }

    pub fn with_not_before(mut self, not_before: i64) -> Self {
        self.not_before = not_before;
        self
    }

    pub fn with_key(mut self, key: Option<String>) -> Self {
        self.operation_key = key;
        self
    }

    pub fn with_draft(mut self, id: &str, revision: i64) -> Self {
        self.draft_id = Some(id.to_string());
        self.draft_revision = Some(revision);
        self
    }

    pub fn with_rfc_message_id(mut self, id: Option<String>) -> Self {
        self.rfc_message_id = id;
        self
    }

    pub fn with_previous_state(mut self, json: Option<String>) -> Self {
        self.previous_state_json = json;
        self
    }

    pub fn depends_on(mut self, op_id: Option<i64>) -> Self {
        self.depends_on_op_id = op_id;
        self
    }

    /// Fill the lightweight summary columns (P6.6) from the payload. The
    /// Outbox panel and the sidebar counter read these, never the payload.
    pub fn with_derived_summary(mut self) -> Self {
        if self.summary_action.is_none() {
            self.summary_action = Some(op_label(&self.kind, &self.payload).to_string());
        }
        let v: serde_json::Value =
            serde_json::from_str(&self.payload).unwrap_or(serde_json::Value::Null);
        if self.summary_recipient.is_none() {
            self.summary_recipient = v
                .get("recipientSummary")
                .and_then(|x| x.as_str())
                .map(str::to_string);
        }
        if self.summary_subject.is_none() {
            self.summary_subject = v
                .get("subject")
                .and_then(|x| x.as_str())
                .map(str::to_string);
        }
        self
    }
}

/// Outcome of an enqueue: the row id and whether this call actually created it
/// (a duplicate `operation_key` returns the existing operation instead).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueuedOp {
    pub id: i64,
    pub created: bool,
}

/// Per-state counts for one account selection (P6.6): counts are computed by
/// the database from indexed columns, never by grouping over payloads.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct OutboxCounts {
    pub pending: i64,
    pub inflight: i64,
    pub uncertain: i64,
    pub done: i64,
    pub failed: i64,
    pub cancelled: i64,
}

#[derive(Debug, Clone)]
pub struct OutboxList {
    pub operations: Vec<Op>,
    pub total: i64,
    pub counts: OutboxCounts,
}

fn row_to_op(r: &rusqlite::Row<'_>) -> rusqlite::Result<Op> {
    Ok(Op {
        id: r.get(0)?,
        account_id: r.get(1)?,
        kind: r.get(2)?,
        payload: r.get(3)?,
        undo_group: r.get(4)?,
        state: r.get(5)?,
        attempts: r.get(6)?,
        not_before: r.get(7)?,
        operation_key: r.get(8)?,
        draft_id: r.get(9)?,
        draft_revision: r.get(10)?,
        rfc_message_id: r.get(11)?,
        previous_state_json: r.get(12)?,
        depends_on_op_id: r.get(13)?,
        reconcile_at: r.get(14)?,
        reconcile_attempts: r.get(15)?,
        failure_code: r.get(16)?,
        last_error: r.get(17)?,
        result_json: r.get(18)?,
        created_at: r.get(19)?,
        started_at: r.get(20)?,
        completed_at: r.get(21)?,
        summary_action: r.get(22)?,
        summary_recipient: r.get(23)?,
        summary_subject: r.get(24)?,
    })
}

/// Insert one operation inside an existing transaction.
///
/// `operation_key`, when present, is a unique index: a second enqueue of the
/// same logical work returns the row that already exists instead of creating a
/// duplicate. That is what makes a double Send produce one operation, and it
/// is honoured here rather than at the call site so a race cannot slip past.
pub fn insert_op(tx: &Transaction<'_>, op: &NewOp) -> Result<QueuedOp> {
    let op = op.clone().with_derived_summary();
    let now = super::now_ms();
    let changed = tx.execute(
        "INSERT INTO outbox_ops \
         (account_id,kind,payload,undo_group,state,attempts,not_before,created_at,\
          operation_key,draft_id,draft_revision,rfc_message_id,previous_state_json,\
          depends_on_op_id,summary_action,summary_recipient,summary_subject) \
         VALUES (?1,?2,?3,?4,'pending',0,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15) \
         ON CONFLICT(operation_key) WHERE operation_key IS NOT NULL DO NOTHING",
        params![
            op.account_id,
            op.kind,
            op.payload,
            op.undo_group,
            op.not_before,
            now,
            op.operation_key,
            op.draft_id,
            op.draft_revision,
            op.rfc_message_id,
            op.previous_state_json,
            op.depends_on_op_id,
            op.summary_action,
            op.summary_recipient,
            op.summary_subject,
        ],
    )?;
    if changed == 1 {
        return Ok(QueuedOp {
            id: tx.last_insert_rowid(),
            created: true,
        });
    }
    // The key already exists: the caller gets the operation that owns it.
    let key = op.operation_key.clone().unwrap_or_else(|| String::from(""));
    let id: i64 = tx.query_row(
        "SELECT id FROM outbox_ops WHERE operation_key=?",
        params![key],
        |r| r.get(0),
    )?;
    Ok(QueuedOp { id, created: false })
}

/// Coalesce an identical idempotent label diff into the newest pending op of
/// the same gesture when it landed inside the 200 ms window (P6.6).
///
/// Only `modify_labels` participates: the ids are unioned (so the diff is
/// still applied exactly once per message) and the per-message before-state is
/// kept as the *first* one seen, which is the state an Undo has to restore.
/// Nothing is reordered: a newer, different action stays a separate operation
/// behind the older one.
pub fn coalesce_label_op(tx: &Transaction<'_>, op: &NewOp) -> Result<Option<i64>> {
    if op.kind != "modify_labels" {
        return Ok(None);
    }
    let Some(group) = op.undo_group.clone() else {
        return Ok(None);
    };
    let incoming: serde_json::Value =
        serde_json::from_str(&op.payload).unwrap_or(serde_json::Value::Null);
    let list = |v: &serde_json::Value, k: &str| -> Vec<String> {
        v.get(k)
            .and_then(|x| x.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|s| s.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let (add, remove) = (list(&incoming, "add"), list(&incoming, "remove"));
    let now = super::now_ms();
    let candidate: Option<(i64, String, Option<String>)> = tx
        .query_row(
            "SELECT id,payload,previous_state_json FROM outbox_ops \
             WHERE account_id=?1 AND kind='modify_labels' AND state='pending' AND undo_group=?2 \
               AND created_at >= ?3 \
             ORDER BY id DESC LIMIT 1",
            params![op.account_id, group, now - COALESCE_WINDOW_MS],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?;
    let Some((id, existing_payload, existing_prev)) = candidate else {
        return Ok(None);
    };
    let existing: serde_json::Value =
        serde_json::from_str(&existing_payload).unwrap_or(serde_json::Value::Null);
    if list(&existing, "add") != add || list(&existing, "remove") != remove {
        return Ok(None);
    }
    let mut ids = list(&existing, "ids");
    for message in list(&incoming, "ids") {
        if !ids.contains(&message) {
            ids.push(message);
        }
    }
    let merged_payload = serde_json::json!({"ids": ids, "add": add, "remove": remove}).to_string();
    let merged_prev =
        merge_previous_state(existing_prev.as_deref(), op.previous_state_json.as_deref());
    tx.execute(
        "UPDATE outbox_ops SET payload=?, previous_state_json=? WHERE id=?",
        params![merged_payload, merged_prev, id],
    )?;
    Ok(Some(id))
}

/// Union two per-message before-state blobs, keeping the earliest value for a
/// message that appears in both.
pub fn merge_previous_state(existing: Option<&str>, incoming: Option<&str>) -> Option<String> {
    let parse = |s: Option<&str>| -> Option<serde_json::Value> {
        s.and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
    };
    match (parse(existing), parse(incoming)) {
        (None, None) => None,
        (Some(v), None) | (None, Some(v)) => Some(v.to_string()),
        (Some(a), Some(b)) => {
            let mut messages = a
                .get("messages")
                .and_then(|m| m.as_array())
                .cloned()
                .unwrap_or_default();
            let seen: Vec<String> = messages
                .iter()
                .filter_map(|m| m.get("id").and_then(|i| i.as_str()).map(str::to_string))
                .collect();
            if let Some(extra) = b.get("messages").and_then(|m| m.as_array()) {
                for m in extra {
                    let id = m.get("id").and_then(|i| i.as_str()).unwrap_or_default();
                    if !seen.iter().any(|s| s == id) {
                        messages.push(m.clone());
                    }
                }
            }
            let mut out = a.clone();
            if let Some(obj) = out.as_object_mut() {
                obj.insert("messages".into(), serde_json::Value::Array(messages));
            }
            Some(out.to_string())
        }
    }
}

impl Db {
    /// Simple enqueue used by callers that have no lifecycle metadata to add
    /// (and by tests). Summaries are still derived, so the Outbox panel and the
    /// counters never have to read a payload.
    pub async fn outbox_enqueue(
        &self,
        account_id: &str,
        kind: &str,
        payload: &str,
        undo_group: Option<&str>,
        not_before: i64,
    ) -> Result<i64> {
        let op = NewOp::new(account_id, kind, payload)
            .with_undo_group(undo_group)
            .with_not_before(not_before);
        Ok(self.outbox_enqueue_op(&op).await?.id)
    }

    pub async fn outbox_enqueue_op(&self, op: &NewOp) -> Result<QueuedOp> {
        let op = op.clone();
        self.write(move |c| {
            let tx = c.unchecked_transaction()?;
            let queued = insert_op(&tx, &op)?;
            tx.commit()?;
            Ok(queued)
        })
        .await
    }

    /// Enqueue, coalescing an identical label diff from the same gesture.
    pub async fn outbox_enqueue_label_diff(&self, op: &NewOp) -> Result<QueuedOp> {
        let op = op.clone();
        self.write(move |c| {
            let tx = c.unchecked_transaction()?;
            let queued = match coalesce_label_op(&tx, &op)? {
                Some(id) => QueuedOp { id, created: false },
                None => insert_op(&tx, &op)?,
            };
            tx.commit()?;
            Ok(queued)
        })
        .await
    }

    /// Queue (or refresh) the debounced remote synchronization of one draft.
    ///
    /// A burst of keystrokes must produce **one** Gmail draft, so an already
    /// pending sync for the same draft is updated in place with the newest
    /// revision and deadline instead of adding another operation.
    pub async fn outbox_enqueue_draft_sync(
        &self,
        account_id: &str,
        local_id: &str,
        revision: i64,
    ) -> Result<i64> {
        let (a, l) = (account_id.to_string(), local_id.to_string());
        let payload = serde_json::json!({"localId": l.clone(), "revision": revision}).to_string();
        let not_before = super::now_ms() + DRAFT_SYNC_DEBOUNCE_MS;
        self.write(move |c| {
            let updated = c.execute(
                "UPDATE outbox_ops SET payload=?, not_before=?, attempts=0, last_error=NULL, \
                   summary_action=? \
                 WHERE id = (SELECT id FROM outbox_ops WHERE account_id=? AND kind='draft_sync' \
                 AND state='pending' AND json_extract(payload,'$.localId')=? \
                 ORDER BY id DESC LIMIT 1)",
                params![
                    payload,
                    not_before,
                    op_label("draft_sync", &payload),
                    a,
                    l
                ],
            )?;
            if updated > 0 {
                return Ok(c.query_row(
                    "SELECT id FROM outbox_ops WHERE account_id=? AND kind='draft_sync' \
                     AND state='pending' AND json_extract(payload,'$.localId')=? \
                     ORDER BY id DESC LIMIT 1",
                    params![a, l],
                    |r| r.get(0),
                )?);
            }
            c.execute(
                "INSERT INTO outbox_ops (account_id,kind,payload,not_before,created_at,summary_action) \
                 VALUES (?, 'draft_sync', ?, ?, ?, ?)",
                params![
                    a,
                    payload,
                    not_before,
                    super::now_ms(),
                    op_label("draft_sync", "{}")
                ],
            )?;
            Ok(c.last_insert_rowid())
        })
        .await
    }

    /// Drop pending remote-sync work for one draft (explicit discard, or a
    /// send that supersedes it).
    pub async fn outbox_cancel_draft_sync(
        &self,
        account_id: &str,
        local_id: &str,
    ) -> Result<usize> {
        let (a, l) = (account_id.to_string(), local_id.to_string());
        self.write(move |c| {
            Ok(c.execute(
                "UPDATE outbox_ops SET state='cancelled', completed_at=?1 \
                 WHERE account_id=?2 AND kind='draft_sync' \
                 AND state IN ('pending','inflight') AND json_extract(payload,'$.localId')=?3",
                params![super::now_ms(), a, l],
            )?)
        })
        .await
    }

    /// Pending sync count for one draft (diagnostics and tests).
    pub async fn outbox_draft_sync_count(&self, local_id: &str) -> Result<i64> {
        let l = local_id.to_string();
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT count(*) FROM outbox_ops WHERE kind='draft_sync' AND state='pending' \
                 AND json_extract(payload,'$.localId')=?",
                params![l],
                |r| r.get(0),
            )?)
        })
        .await
    }

    pub async fn outbox_get(&self, id: i64) -> Result<Option<Op>> {
        self.read(move |c| {
            let sql = format!("SELECT {OP_COLUMNS} FROM outbox_ops WHERE id=?");
            Ok(c.query_row(&sql, params![id], row_to_op).optional()?)
        })
        .await
    }

    /// The operation that owns a key, if any (double Send, replayed commands).
    pub async fn outbox_by_key(&self, key: &str) -> Result<Option<Op>> {
        let key = key.to_string();
        self.read(move |c| {
            let sql = format!("SELECT {OP_COLUMNS} FROM outbox_ops WHERE operation_key=?");
            Ok(c.query_row(&sql, params![key], row_to_op).optional()?)
        })
        .await
    }

    /// Kept for callers that only need the oldest pending row (tests and
    /// diagnostics); the drain uses [`Self::outbox_claim`].
    pub async fn outbox_next(&self, account_id: &str) -> Result<Option<Op>> {
        let a = account_id.to_string();
        let now = super::now_ms();
        self.read(move |c| {
            let sql = format!(
                "SELECT {OP_COLUMNS} FROM outbox_ops \
                 WHERE account_id=? AND state='pending' AND not_before<=? ORDER BY id LIMIT 1"
            );
            Ok(c.query_row(&sql, params![a, now], row_to_op).optional()?)
        })
        .await
    }

    /// Atomically claim the oldest runnable operation (P6.1).
    ///
    /// One statement on the write lane: the claim and the state transition are
    /// the same `UPDATE`, so two drainers racing for the same row produce one
    /// `inflight` row and one `None`. An op whose dependency has not finished
    /// is not runnable, and a label diff never overtakes an older pending label
    /// diff that touches the same message.
    pub async fn outbox_claim(&self, account_id: &str) -> Result<Option<Op>> {
        let a = account_id.to_string();
        let now = super::now_ms();
        self.write(move |c| {
            let sql = format!(
                "UPDATE outbox_ops SET state='{STATE_INFLIGHT}', started_at=?2, attempts=attempts+1 \
                 WHERE id = ( \
                   SELECT o.id FROM outbox_ops o \
                   WHERE o.account_id=?1 AND o.state='{STATE_PENDING}' AND o.not_before<=?2 \
                     AND (o.depends_on_op_id IS NULL OR EXISTS ( \
                          SELECT 1 FROM outbox_ops d WHERE d.id=o.depends_on_op_id AND d.state='{STATE_DONE}')) \
                     AND NOT EXISTS ( \
                       SELECT 1 FROM outbox_ops older \
                       WHERE older.account_id=o.account_id AND older.state='{STATE_PENDING}' \
                         AND older.id < o.id AND older.kind='modify_labels' AND o.kind='modify_labels' \
                         AND EXISTS (SELECT 1 FROM json_each(json_extract(older.payload,'$.ids')) a \
                                     JOIN json_each(json_extract(o.payload,'$.ids')) b ON a.value=b.value)) \
                   ORDER BY o.id LIMIT 1) \
                 RETURNING {OP_COLUMNS}"
            );
            Ok(c.query_row(&sql, params![a, now], row_to_op).optional()?)
        })
        .await
    }

    /// The next uncertain send whose bounded reconciliation is due, if any.
    /// The row stays `uncertain`: reconciliation never resubmits.
    pub async fn outbox_claim_reconcile(&self, account_id: &str) -> Result<Option<Op>> {
        let a = account_id.to_string();
        let now = super::now_ms();
        self.write(move |c| {
            let sql = format!(
                "UPDATE outbox_ops SET reconcile_at=NULL, reconcile_attempts=reconcile_attempts+1 \
                 WHERE id = ( \
                   SELECT id FROM outbox_ops \
                   WHERE account_id=?1 AND kind='send' AND state='{STATE_UNCERTAIN}' \
                     AND reconcile_attempts < ?2 AND (reconcile_at IS NULL OR reconcile_at<=?3) \
                   ORDER BY id LIMIT 1) \
                 RETURNING {OP_COLUMNS}"
            );
            Ok(c.query_row(
                &sql,
                params![a, RECONCILE_DELAYS_MS.len() as i64, now],
                row_to_op,
            )
            .optional()?)
        })
        .await
    }

    /// Retain/expose the raw update helper (tests and legacy callers).
    pub async fn outbox_set(
        &self,
        id: i64,
        state: &str,
        attempts: i64,
        not_before: i64,
        err: Option<String>,
    ) -> Result<()> {
        let state = state.to_string();
        self.write(move |c| {
            c.execute(
                "UPDATE outbox_ops SET state=?, attempts=?, not_before=?, last_error=? WHERE id=?",
                params![state, attempts, not_before, err, id],
            )?;
            Ok(())
        })
        .await
    }

    /// inflight -> done. Releases dependents (a send-and-archive that waited on
    /// this send becomes runnable) and records the provider result.
    pub async fn outbox_mark_done(&self, id: i64, result_json: Option<&str>) -> Result<()> {
        let result = result_json.map(str::to_string);
        self.write(move |c| {
            c.execute(
                "UPDATE outbox_ops SET state=?1, completed_at=?2, result_json=?3, last_error=NULL, \
                   failure_code=NULL, reconcile_at=NULL \
                 WHERE id=?4",
                params![STATE_DONE, super::now_ms(), result, id],
            )?;
            Ok(())
        })
        .await
    }

    /// inflight -> failed (definite terminal rejection, or an invalid payload
    /// that will never succeed). Dependents can never run and are cancelled.
    pub async fn outbox_mark_failed(&self, id: i64, code: &str, error: &str) -> Result<()> {
        let (code, error) = (code.to_string(), error.to_string());
        self.write(move |c| {
            let now = super::now_ms();
            let tx = c.unchecked_transaction()?;
            tx.execute(
                "UPDATE outbox_ops SET state=?1, completed_at=?2, failure_code=?3, last_error=?4 \
                 WHERE id=?5",
                params![STATE_FAILED, now, code, error, id],
            )?;
            cancel_dependents(&tx, id, now)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// inflight send -> uncertain: the acceptance is unknown. The next step is
    /// reconciliation by RFC Message-ID, never a resubmission.
    pub async fn outbox_mark_uncertain(&self, id: i64, code: &str, error: &str) -> Result<()> {
        let (code, error) = (code.to_string(), error.to_string());
        let delay = RECONCILE_DELAYS_MS[0];
        self.write(move |c| {
            let now = super::now_ms();
            let tx = c.unchecked_transaction()?;
            tx.execute(
                "UPDATE outbox_ops SET state=?1, failure_code=?2, last_error=?3, \
                   reconcile_at=?4, reconcile_attempts=0, started_at=COALESCE(started_at, ?5) \
                 WHERE id=?6",
                params![STATE_UNCERTAIN, code, error, now + delay, now, id],
            )?;
            cancel_dependents(&tx, id, now)?;
            tx.commit()?;
            Ok(())
        })
        .await
    }

    /// Schedule the next bounded reconciliation of an uncertain send, or leave
    /// it for the user when the schedule is exhausted.
    pub async fn outbox_reschedule_reconcile(&self, id: i64, attempts: i64) -> Result<()> {
        self.write(move |c| {
            let now = super::now_ms();
            let next = RECONCILE_DELAYS_MS
                .get(attempts as usize)
                .map(|delay| now + delay);
            c.execute(
                "UPDATE outbox_ops SET reconcile_at=? WHERE id=?",
                params![next, id],
            )?;
            Ok(())
        })
        .await
    }

    /// inflight -> pending: a definite, side-effect-free failure (connection or
    /// TLS establishment, a server rejection before the message was accepted).
    pub async fn outbox_requeue(
        &self,
        id: i64,
        attempts: i64,
        not_before: i64,
        code: &str,
        error: &str,
    ) -> Result<()> {
        let (code, error) = (code.to_string(), error.to_string());
        self.write(move |c| {
            c.execute(
                "UPDATE outbox_ops SET state=?1, attempts=?2, not_before=?3, failure_code=?4, \
                   last_error=?5, started_at=NULL \
                 WHERE id=?6",
                params![STATE_PENDING, attempts, not_before, code, error, id],
            )?;
            Ok(())
        })
        .await
    }

    /// pending -> cancelled. Exactly one winner: `false` means the operation
    /// was already claimed (or finished) and the mail must not be called back.
    pub async fn outbox_cancel(&self, id: i64, code: &str, error: &str) -> Result<bool> {
        let (code, error) = (code.to_string(), error.to_string());
        self.write(move |c| {
            let now = super::now_ms();
            let tx = c.unchecked_transaction()?;
            let changed = tx.execute(
                "UPDATE outbox_ops SET state=?1, completed_at=?2, failure_code=?3, last_error=?4 \
                 WHERE id=?5 AND state=?6",
                params![STATE_CANCELLED, now, code, error, id, STATE_PENDING],
            )?;
            if changed > 0 {
                cancel_dependents(&tx, id, now)?;
            }
            tx.commit()?;
            Ok(changed > 0)
        })
        .await
    }

    /// Replace an operation's payload (resolving symbolic label references at
    /// claim time, before the provider ever sees an id).
    pub async fn outbox_set_payload(&self, id: i64, payload: &str) -> Result<()> {
        let payload = payload.to_string();
        self.write(move |c| {
            c.execute(
                "UPDATE outbox_ops SET payload=? WHERE id=?",
                params![payload, id],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn outbox_cancel_group(&self, undo_group: &str) -> Result<Vec<Op>> {
        let group = undo_group.to_string();
        let now = super::now_ms();
        self.write(move |c| {
            let tx = c.unchecked_transaction()?;
            let sql = format!(
                "SELECT {OP_COLUMNS} FROM outbox_ops WHERE undo_group=? AND state='{STATE_PENDING}'"
            );
            let ops: Vec<Op> = tx
                .prepare(&sql)?
                .query_map(params![group], row_to_op)?
                .collect::<Result<Vec<_>, _>>()?;
            for op in &ops {
                tx.execute(
                    "UPDATE outbox_ops SET state=?1, completed_at=?2, failure_code='undo' \
                     WHERE id=?3 AND state=?4",
                    params![STATE_CANCELLED, now, op.id, STATE_PENDING],
                )?;
                cancel_dependents(&tx, op.id, now)?;
            }
            tx.commit()?;
            Ok(ops)
        })
        .await
    }

    pub async fn outbox_done_group(&self, undo_group: &str) -> Result<Vec<Op>> {
        let group = undo_group.to_string();
        self.read(move |c| {
            let sql = format!(
                "SELECT {OP_COLUMNS} FROM outbox_ops WHERE undo_group=? AND state='{STATE_DONE}' ORDER BY id"
            );
            Ok(c.prepare(&sql)?
                .query_map(params![group], row_to_op)?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    /// Every operation of a gesture that still needs a compensation decision:
    /// pending and inflight (done is handled by its own query, the terminals
    /// need nothing).
    pub async fn outbox_open_group(&self, undo_group: &str) -> Result<Vec<Op>> {
        let group = undo_group.to_string();
        self.read(move |c| {
            let sql = format!(
                "SELECT {OP_COLUMNS} FROM outbox_ops WHERE undo_group=? \
                 AND state IN ('{STATE_PENDING}','{STATE_INFLIGHT}','{STATE_UNCERTAIN}') ORDER BY id"
            );
            Ok(c.prepare(&sql)?
                .query_map(params![group], row_to_op)?
                .collect::<Result<Vec<_>, _>>()?)
        })
        .await
    }

    pub async fn outbox_state_counts(&self, account_id: &str) -> Result<OutboxCounts> {
        let a = account_id.to_string();
        self.read(move |c| counts_for(c, &[a])).await
    }

    pub async fn outbox_pending_count(&self, account_id: &str) -> Result<i64> {
        let a = account_id.to_string();
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT count(*) FROM outbox_ops WHERE account_id=? AND state IN ('pending','inflight')",
                params![a],
                |r| r.get(0),
            )?)
        })
        .await
    }

    /// Ops the user must see: terminal failures plus sends whose acceptance is
    /// still unknown (P4.6/P6.1). Never a hardcoded zero.
    pub async fn outbox_failed_count(&self, account_id: &str) -> Result<i64> {
        let a = account_id.to_string();
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT count(*) FROM outbox_ops WHERE account_id=? AND state IN ('failed','uncertain')",
                params![a],
                |r| r.get(0),
            )?)
        })
        .await
    }

    /// Unacknowledged local label intents for one message, oldest first
    /// (P4.5): sync re-applies them over the server snapshot as an overlay.
    pub async fn outbox_label_intents(
        &self,
        account_id: &str,
        message_id: &str,
    ) -> Result<Vec<(Vec<String>, Vec<String>)>> {
        let (a, m) = (account_id.to_string(), message_id.to_string());
        self.read(move |c| {
            let mut s = c.prepare(
                "SELECT payload FROM outbox_ops \
                 WHERE account_id=?1 AND kind='modify_labels' AND state IN ('pending','inflight') \
                   AND EXISTS (SELECT 1 FROM json_each(json_extract(payload,'$.ids')) WHERE value=?2) \
                 ORDER BY id",
            )?;
            let payloads: Vec<String> = s
                .query_map(params![a, m], |r| r.get(0))?
                .collect::<Result<Vec<_>, _>>()?;
            let mut out = vec![];
            for p in payloads {
                let v: serde_json::Value = serde_json::from_str(&p).unwrap_or_default();
                let list = |k: &str| -> Vec<String> {
                    v.get(k)
                        .and_then(|x| x.as_array())
                        .map(|a| {
                            a.iter()
                                .filter_map(|x| x.as_str().map(|s| s.to_string()))
                                .collect()
                        })
                        .unwrap_or_default()
                };
                out.push((list("add"), list("remove")));
            }
            Ok(out)
        })
        .await
    }

    /// Pending work grouped into human labels for the sidebar (P6.6). Reads
    /// the summary column written at enqueue, never the payload: queueing
    /// 10 000 label operations and a handful of large sends stays cheap.
    pub async fn outbox_summary(&self, account_id: &str) -> Result<Vec<(String, i64)>> {
        let a = account_id.to_string();
        self.read(move |c| {
            let mut s = c.prepare(
                "SELECT COALESCE(summary_action, kind), count(*) FROM outbox_ops \
                 WHERE account_id=? AND state IN ('pending','inflight','uncertain') \
                 GROUP BY 1 ORDER BY 1",
            )?;
            let rows: Vec<(String, i64)> = s
                .query_map(params![a], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// The compact Outbox panel page (P6.6): newest first, keyset on id.
    pub async fn outbox_list(
        &self,
        account_ids: &[String],
        states: &[String],
        cursor: Option<i64>,
        limit: i64,
    ) -> Result<OutboxList> {
        let accounts = account_ids.to_vec();
        let states = states.to_vec();
        let limit = limit.clamp(1, 200);
        self.read(move |c| {
            let mut where_sql = String::from("1=1");
            let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![];
            if !accounts.is_empty() {
                let holes = vec!["?"; accounts.len()].join(",");
                where_sql.push_str(&format!(" AND account_id IN ({holes})"));
                for a in &accounts {
                    binds.push(Box::new(a.clone()));
                }
            }
            if !states.is_empty() {
                let holes = vec!["?"; states.len()].join(",");
                where_sql.push_str(&format!(" AND state IN ({holes})"));
                for s in &states {
                    binds.push(Box::new(s.clone()));
                }
            }
            let total: i64 = c.query_row(
                &format!("SELECT count(*) FROM outbox_ops WHERE {where_sql}"),
                rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())),
                |r| r.get(0),
            )?;
            let mut page_sql = format!("SELECT {OP_COLUMNS} FROM outbox_ops WHERE {where_sql}");
            let mut page_binds: Vec<Box<dyn rusqlite::ToSql>> = Vec::new();
            for a in &accounts {
                page_binds.push(Box::new(a.clone()));
            }
            for s in &states {
                page_binds.push(Box::new(s.clone()));
            }
            if let Some(cursor) = cursor {
                page_sql.push_str(" AND id < ?");
                page_binds.push(Box::new(cursor));
            }
            page_sql.push_str(" ORDER BY id DESC LIMIT ?");
            page_binds.push(Box::new(limit));
            let operations = c
                .prepare(&page_sql)?
                .query_map(
                    rusqlite::params_from_iter(page_binds.iter().map(|b| b.as_ref())),
                    row_to_op,
                )?
                .collect::<Result<Vec<_>, _>>()?;
            let counts = counts_for(c, &accounts)?;
            Ok(OutboxList {
                operations,
                total,
                counts,
            })
        })
        .await
    }

    /// The next instant at which this account's queue can make progress, if
    /// any. The drain loop sleeps until exactly this moment instead of polling
    /// (P8.1): schedule precision comes from the persisted deadline, never
    /// from how often inbox polling happens to run.
    ///
    /// Three sources matter: a pending operation's `not_before` (a scheduled
    /// send, a retry backoff, a debounced draft sync), an uncertain send's
    /// bounded reconciliation and — implicitly — nothing else. A pending
    /// operation already due reports `now`, so the loop drains immediately.
    pub async fn outbox_next_deadline(&self, account_id: Option<String>) -> Result<Option<i64>> {
        let now = super::now_ms();
        self.read(move |c| {
            let filter = if account_id.is_some() {
                " AND account_id=?"
            } else {
                ""
            };
            let sql = format!(
                "SELECT MIN(x) FROM ( \
                   SELECT CASE WHEN not_before<=?2 THEN ?2 ELSE not_before END AS x \
                     FROM outbox_ops WHERE state='{STATE_PENDING}'{filter} \
                   UNION ALL \
                   SELECT reconcile_at FROM outbox_ops \
                     WHERE state='{STATE_UNCERTAIN}' AND reconcile_at IS NOT NULL{filter} \
                 )"
            );
            match &account_id {
                Some(a) => Ok(c.query_row(&sql, params![a, now], |r| r.get(0))?),
                None => Ok(c.query_row(&sql, params![rusqlite::types::Null, now], |r| r.get(0))?),
            }
        })
        .await
    }

    /// Accounts whose queue has work that is due now, so the scheduler can
    /// nudge exactly those.
    pub async fn outbox_due_accounts(&self, now: i64) -> Result<Vec<String>> {
        self.read(move |c| {
            let mut s = c.prepare(
                "SELECT DISTINCT account_id FROM outbox_ops \
                 WHERE (state='pending' AND not_before<=?1) \
                    OR (state='uncertain' AND reconcile_at IS NOT NULL AND reconcile_at<=?1)",
            )?;
            let rows = s
                .query_map(params![now], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Retry an operation the user explicitly asked for (P6.1/P6.6).
    ///
    /// Only `failed` and `uncertain` rows move. An uncertain send needs the
    /// duplicate-risk acknowledgement: Sift cannot prove the provider did not
    /// accept the message, so it never makes that decision for the user.
    pub async fn outbox_retry(&self, id: i64, acknowledge_duplicate_risk: bool) -> Result<Op> {
        self.write(move |c| {
            let now = super::now_ms();
            let tx = c.unchecked_transaction()?;
            let sql = format!("SELECT {OP_COLUMNS} FROM outbox_ops WHERE id=?");
            let op: Op = tx.query_row(&sql, params![id], row_to_op).optional()?.ok_or_else(|| {
                anyhow::anyhow!(crate::errors::SiftError::NotFound(format!("operation {id}")))
            })?;
            match op.state.as_str() {
                STATE_FAILED => {
                    tx.execute(
                        "UPDATE outbox_ops SET state=?1, attempts=0, not_before=?2, failure_code=NULL, \
                           last_error=NULL, completed_at=NULL, started_at=NULL \
                         WHERE id=?3 AND state=?4",
                        params![STATE_PENDING, now, id, STATE_FAILED],
                    )?;
                }
                STATE_UNCERTAIN => {
                    if !acknowledge_duplicate_risk {
                        return Err(anyhow::anyhow!(
                            crate::errors::SiftError::typed(
                                "acknowledge_duplicate_risk",
                                "Sift could not prove this message was not delivered. Sending it again may duplicate it.",
                                serde_json::json!({"state": STATE_UNCERTAIN, "opId": id, "notBefore": op.not_before}),
                            )
                        ));
                    }
                    tx.execute(
                        "UPDATE outbox_ops SET state=?1, attempts=0, not_before=?2, failure_code=NULL, \
                           last_error=NULL, reconcile_at=NULL, reconcile_attempts=0 \
                         WHERE id=?3 AND state=?4",
                        params![STATE_PENDING, now, id, STATE_UNCERTAIN],
                    )?;
                }
                STATE_PENDING | STATE_INFLIGHT => {}
                other => {
                    return Err(anyhow::anyhow!(crate::errors::SiftError::typed(
                        "not_retryable",
                        "That operation already finished.",
                        serde_json::json!({"state": other, "opId": id, "notBefore": op.not_before}),
                    )));
                }
            }
            let op = tx.query_row(&sql, params![id], row_to_op)?;
            tx.commit()?;
            Ok(op)
        })
        .await
    }

    /// Retain seven days of metadata and staged content, then drop the
    /// payloads of finished operations (P6.6).
    ///
    /// Pending, failed, uncertain and scheduled sends are never pruned, and
    /// neither are the files they reference: only work that is provably over
    /// loses its payload, and the caller removes the files it names.
    pub async fn outbox_prune(&self, retention_ms: i64) -> Result<Vec<String>> {
        self.write(move |c| {
            let now = super::now_ms();
            let cutoff = now - retention_ms;
            let rows: Vec<(i64, String)> = c
                .prepare(
                    "SELECT id,payload FROM outbox_ops \
                     WHERE state IN ('done','cancelled') AND completed_at IS NOT NULL \
                       AND completed_at < ?1 AND payload <> ''",
                )?
                .query_map(params![cutoff], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            let mut files = vec![];
            for (id, payload) in rows {
                let v: serde_json::Value =
                    serde_json::from_str(&payload).unwrap_or(serde_json::Value::Null);
                if let Some(path) = v.get("rawPath").and_then(|p| p.as_str()) {
                    files.push(path.to_string());
                }
                c.execute(
                    "UPDATE outbox_ops SET payload='', previous_state_json=NULL WHERE id=?",
                    params![id],
                )?;
            }
            Ok(files)
        })
        .await
    }

    /// Drop every op for an account (account removal).
    pub async fn outbox_delete_account(&self, account_id: &str) -> Result<()> {
        let a = account_id.to_string();
        self.write(move |c| {
            c.execute("DELETE FROM outbox_ops WHERE account_id=?", params![a])?;
            Ok(())
        })
        .await
    }

    /// Cancel every operation that waited on an operation which can no longer
    /// succeed. Called from the terminal transitions so a dependent (an
    /// archive-after-send, a label apply behind a create) can never run alone.
    pub async fn outbox_cancel_dependents_of(&self, id: i64) -> Result<usize> {
        self.write(move |c| {
            let now = super::now_ms();
            Ok(cancel_dependents(c, id, now)?)
        })
        .await
    }
}

fn cancel_dependents(conn: &Connection, id: i64, now: i64) -> rusqlite::Result<usize> {
    conn.execute(
        "UPDATE outbox_ops SET state=?1, completed_at=?2, failure_code='dependency_failed', \
           last_error='the operation this waited on did not succeed' \
         WHERE depends_on_op_id=?3 AND state IN (?4,?5)",
        params![STATE_CANCELLED, now, id, STATE_PENDING, STATE_INFLIGHT],
    )
}

fn counts_for(conn: &Connection, accounts: &[String]) -> anyhow::Result<OutboxCounts> {
    let mut where_sql = String::from("1=1");
    let mut binds: Vec<Box<dyn rusqlite::ToSql>> = vec![];
    if !accounts.is_empty() {
        let holes = vec!["?"; accounts.len()].join(",");
        where_sql.push_str(&format!(" AND account_id IN ({holes})"));
        for a in accounts {
            binds.push(Box::new(a.clone()));
        }
    }
    let mut counts = OutboxCounts::default();
    let mut s = conn.prepare(&format!(
        "SELECT state, count(*) FROM outbox_ops WHERE {where_sql} GROUP BY state"
    ))?;
    let rows = s.query_map(
        rusqlite::params_from_iter(binds.iter().map(|b| b.as_ref())),
        |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
    )?;
    for row in rows {
        let (state, n) = row?;
        match state.as_str() {
            STATE_PENDING => counts.pending = n,
            STATE_INFLIGHT => counts.inflight = n,
            STATE_UNCERTAIN => counts.uncertain = n,
            STATE_DONE => counts.done = n,
            STATE_FAILED => counts.failed = n,
            STATE_CANCELLED => counts.cancelled = n,
            _ => {}
        }
    }
    Ok(counts)
}

/// Human label for a queued op, derived from its kind and payload.
pub fn op_label(kind: &str, payload: &str) -> &'static str {
    if kind == "modify_labels" {
        let v: serde_json::Value = serde_json::from_str(payload).unwrap_or_default();
        let has = |field: &str, val: &str| {
            v.get(field)
                .and_then(|a| a.as_array())
                .map(|a| {
                    a.iter().any(|x| {
                        x.as_str()
                            .map(|s| s == val || s == format!("name:{val}"))
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        };
        if has("add", "TRASH") {
            return "Moving to trash";
        }
        if has("add", "SPAM") {
            return "Marking as spam";
        }
        if has("add", "INBOX") {
            return "Moving to inbox";
        }
        if has("remove", "UNREAD") {
            return "Marking as read";
        }
        if has("add", "UNREAD") {
            return "Marking as unread";
        }
        if has("add", "STARRED") {
            return "Starring";
        }
        if has("remove", "STARRED") {
            return "Unstarring";
        }
        if has("remove", "INBOX") {
            return "Archiving";
        }
        return "Updating labels";
    }
    match kind {
        "trash" => "Moving to trash",
        "untrash" => "Moving to inbox",
        "spam" => "Marking as spam",
        "unspam" => "Moving to inbox",
        "delete" => "Deleting permanently",
        "send" => "Sending",
        "create_label" => "Creating a label",
        "draft_upsert" => "Saving draft",
        "draft_delete" => "Deleting draft",
        "draft_sync" => "Saving draft",
        _ => "Applying change",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;

    #[tokio::test]
    async fn p6_t04_cancel_pending() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        db.outbox_enqueue("a", "modify_labels", "{}", Some("g1"), 0)
            .await
            .unwrap();
        let ops = db.outbox_cancel_group("g1").await.unwrap();
        assert_eq!(ops.len(), 1);
        assert_eq!(db.outbox_pending_count("a").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn p6_1_claim_is_atomic_and_loses_to_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let id = db
            .outbox_enqueue("a", "modify_labels", "{\"ids\":[\"m1\"]}", None, 0)
            .await
            .unwrap();
        let claimed = db.outbox_claim("a").await.unwrap().expect("claimable");
        assert_eq!(claimed.id, id);
        assert_eq!(claimed.state, STATE_INFLIGHT);
        assert!(
            claimed.started_at.is_some(),
            "started_at before the network"
        );
        // A second claimer finds nothing: the row is not pending any more.
        assert!(db.outbox_claim("a").await.unwrap().is_none());
        // And the cancel loses: the provider may already have the operation.
        assert!(!db.outbox_cancel(id, "undo", "too late").await.unwrap());
    }

    #[tokio::test]
    async fn p6_2_double_send_creates_one_operation() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let op = NewOp::new("a", "send", "{}")
            .with_key(Some("send:d1:3".into()))
            .with_draft("d1", 3);
        let first = db.outbox_enqueue_op(&op).await.unwrap();
        let second = db.outbox_enqueue_op(&op).await.unwrap();
        assert!(first.created);
        assert!(!second.created, "the second Send reuses the operation");
        assert_eq!(first.id, second.id);
    }

    #[tokio::test]
    async fn p6_6_dependency_cycle_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let a = db
            .outbox_enqueue("a", "modify_labels", "{}", None, 0)
            .await
            .unwrap();
        let op = NewOp::new("a", "modify_labels", "{}").depends_on(Some(a));
        let b = db.outbox_enqueue_op(&op).await.unwrap().id;
        let cycle = db
            .write(move |c| {
                c.execute(
                    "UPDATE outbox_ops SET depends_on_op_id=?1 WHERE id=?2",
                    params![b, a],
                )?;
                Ok(())
            })
            .await;
        let refused = match &cycle {
            Err(e) => e.to_string().contains("dependency cycle"),
            Ok(()) => false,
        };
        assert!(refused, "a dependency cycle must be refused: {cycle:?}");
    }
}
