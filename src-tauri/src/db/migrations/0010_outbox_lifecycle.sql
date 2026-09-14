-- Phase 6 — durable outbox lifecycle, operation identity and snooze state.
--
-- Additive only (appendix A inventory: 0010 must preserve existing queued
-- work). A row queued by an older build keeps its id, its payload and its
-- kind; it simply has no operation key, no previous state and no dependency,
-- which is exactly how a "legacy" op behaves. Nothing is replayed blindly:
-- startup recovery (db::recover_outbox) decides per kind and per state.
--
--   operation_key        stable identity of one logical operation.
--                        send:<draft id>:<queued revision> is unique, so a
--                        double Send produces one operation, not two.
--   draft_id / revision  the immutable revision a send froze
--   rfc_message_id       the stable Message-ID used to reconcile an
--                        uncertain send against the provider's Sent folder
--   started_at           persisted before the network call (claim time)
--   completed_at         set when the op reaches done/failed/cancelled
--   previous_state_json  per-message prior values for an exact Undo
--   failure_code         machine-readable reason, kept next to the message
--   result_json          provider ids/receipt evidence for a done op
--   depends_on_op_id     send-and-archive and create-label ordering
--   summary_*            lightweight fields the Outbox panel reads instead of
--                        grouping by (potentially megabytes of) payload
ALTER TABLE outbox_ops ADD COLUMN operation_key TEXT;
ALTER TABLE outbox_ops ADD COLUMN draft_id TEXT;
ALTER TABLE outbox_ops ADD COLUMN draft_revision INTEGER;
ALTER TABLE outbox_ops ADD COLUMN rfc_message_id TEXT;
ALTER TABLE outbox_ops ADD COLUMN started_at INTEGER;
ALTER TABLE outbox_ops ADD COLUMN completed_at INTEGER;
ALTER TABLE outbox_ops ADD COLUMN previous_state_json TEXT;
ALTER TABLE outbox_ops ADD COLUMN failure_code TEXT;
ALTER TABLE outbox_ops ADD COLUMN result_json TEXT;
ALTER TABLE outbox_ops ADD COLUMN depends_on_op_id INTEGER REFERENCES outbox_ops(id);
ALTER TABLE outbox_ops ADD COLUMN summary_action TEXT;
ALTER TABLE outbox_ops ADD COLUMN summary_recipient TEXT;
ALTER TABLE outbox_ops ADD COLUMN summary_subject TEXT;
-- Bounded reconciliation of an uncertain send: 5s, 30s, 120s, then the user
-- decides. Never an automatic resubmission.
ALTER TABLE outbox_ops ADD COLUMN reconcile_at INTEGER;
ALTER TABLE outbox_ops ADD COLUMN reconcile_attempts INTEGER NOT NULL DEFAULT 0;

-- One operation per logical unit of work. Partial so the many legacy/plain
-- rows that legitimately have no key are unaffected.
CREATE UNIQUE INDEX outbox_operation_key
  ON outbox_ops(operation_key) WHERE operation_key IS NOT NULL;

-- Blast-radius of a send: everything queued behind it (archive-after-send,
-- create-label-then-apply-label).
CREATE INDEX outbox_dependency_idx
  ON outbox_ops(depends_on_op_id) WHERE depends_on_op_id IS NOT NULL;

-- Pruning walks done/cancelled rows by completion time.
CREATE INDEX outbox_sweep_idx
  ON outbox_ops(state, completed_at) WHERE completed_at IS NOT NULL;

-- Explicit cycle rejection (appendix A). An INSERT can only create a cycle
-- with itself; a cycle through other rows needs an UPDATE.
CREATE TRIGGER outbox_dependency_self_cycle
BEFORE INSERT ON outbox_ops
WHEN NEW.depends_on_op_id IS NOT NULL AND NEW.depends_on_op_id = NEW.id
BEGIN
  SELECT RAISE(ABORT, 'outbox dependency cycle');
END;

CREATE TRIGGER outbox_dependency_cycle
BEFORE UPDATE OF depends_on_op_id ON outbox_ops
WHEN NEW.depends_on_op_id IS NOT NULL
BEGIN
  SELECT RAISE(ABORT, 'outbox dependency cycle')
  WHERE EXISTS (
    WITH RECURSIVE dependents(id) AS (
      SELECT OLD.id
      UNION
      SELECT o.id FROM outbox_ops o JOIN dependents d ON o.depends_on_op_id = d.id
    )
    SELECT 1 FROM dependents WHERE id = NEW.depends_on_op_id
  );
END;

-- The scope Google actually granted (P6.4). The consent screen can return
-- less than Sift requested, and permanent deletion must be gated on what was
-- really granted rather than on what was asked for.
ALTER TABLE accounts ADD COLUMN granted_scope TEXT;

-- Snooze is LOCAL scheduling with an optional matching remote label (P6.5).
-- The row records what the thread looked like when it was snoozed, so
-- Unsnooze restores the exact prior Inbox policy, and which label (if any)
-- carries the server-side copy.
ALTER TABLE snoozes ADD COLUMN label_id TEXT;
ALTER TABLE snoozes ADD COLUMN previous_json TEXT;
ALTER TABLE snoozes ADD COLUMN wake_unread INTEGER NOT NULL DEFAULT 0;
ALTER TABLE snoozes ADD COLUMN gesture_id TEXT;
ALTER TABLE snoozes ADD COLUMN created_at INTEGER;
ALTER TABLE snoozes ADD COLUMN state TEXT NOT NULL DEFAULT 'sleeping';

-- Account removal must take snoozes with it (P4.2 cleanup); the table had no
-- account foreign key, so deletions were manual and could leave a timer that
-- wakes a thread that no longer exists.
CREATE INDEX snoozes_due_idx ON snoozes(wake_at) WHERE state = 'sleeping';
