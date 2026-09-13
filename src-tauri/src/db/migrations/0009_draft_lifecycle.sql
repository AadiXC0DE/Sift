-- Phase 5 — draft lifecycle, threading context and contacts FTS maintenance.
--
-- Additive only (appendix A migration inventory: 0009 must preserve existing
-- unsent drafts and remote ids). Every existing draft keeps its local id, its
-- remote draft/message ids and its content; it starts at revision 0 in state
-- 'editing', which is exactly the state of a draft that was never queued.
--
--   revision          local content revision, monotonic per draft
--   saved_revision    the revision whose content the remote copy holds
--   remote_revision   the last remote-side revision this row reconciled with
--   state             editing | queued | sent | failed
--   from_email        the sending identity the user selected
--   rfc_message_id    stable Message-ID of this draft's send lineage
--   parent_rfc_message_id / references_json  threading context for MIME/REST
--   not_before        queued send deadline (undo / send later)
--   scheduled_*       user-visible scheduling metadata
ALTER TABLE drafts ADD COLUMN revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE drafts ADD COLUMN saved_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE drafts ADD COLUMN remote_revision INTEGER NOT NULL DEFAULT 0;
ALTER TABLE drafts ADD COLUMN state TEXT NOT NULL DEFAULT 'editing';
ALTER TABLE drafts ADD COLUMN from_email TEXT;
ALTER TABLE drafts ADD COLUMN rfc_message_id TEXT;
ALTER TABLE drafts ADD COLUMN parent_rfc_message_id TEXT;
ALTER TABLE drafts ADD COLUMN references_json TEXT NOT NULL DEFAULT '[]';
ALTER TABLE drafts ADD COLUMN not_before INTEGER;
ALTER TABLE drafts ADD COLUMN scheduled_at INTEGER;
ALTER TABLE drafts ADD COLUMN scheduled_timezone TEXT;
ALTER TABLE drafts ADD COLUMN scheduled_local_time TEXT;

-- Draft list paging is keyset on (updated_at, local_id); remote reconciliation
-- looks a draft up by its remote id.
CREATE INDEX IF NOT EXISTS drafts_by_account_updated
  ON drafts(account_id, updated_at DESC, local_id DESC);
CREATE INDEX IF NOT EXISTS drafts_by_remote
  ON drafts(account_id, remote_draft_id);

-- P5.5: `contacts_fts` is contentless, so it is only as correct as its
-- triggers. 0002 covered insert and delete; a rename must be reindexed as
-- delete+insert, because an AFTER UPDATE trigger that only inserted would
-- leave the old trigram row behind and suggestions would return the stale
-- name forever.
CREATE TRIGGER IF NOT EXISTS contacts_au AFTER UPDATE ON contacts BEGIN
  INSERT INTO contacts_fts(contacts_fts, rowid, email, name)
    VALUES('delete', old.rowid, old.email, old.name);
  INSERT INTO contacts_fts(rowid, email, name)
    VALUES (new.rowid, new.email, new.name);
END;
