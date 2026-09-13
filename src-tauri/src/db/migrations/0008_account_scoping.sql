-- 0008_account_scoping (P4.2)
--
-- Provider message ids are only unique *within* an account, but `messages`
-- keyed rows by the bare provider id and every child table joined on that one
-- column. Two accounts can hold the same Gmail hex id, so reads, labels,
-- bodies, attachments and search could cross accounts. This migration makes
-- the account half of the identity explicit everywhere.
--
-- Rebuild rules (appendix A): start from the exact existing CREATE TABLE,
-- retain every current non-column (including 0004-0007 additions), change only
-- key/foreign-key columns, copy rows with explicit column lists (never
-- SELECT *), and recreate indexes/triggers. Bodies, bytes and filenames are
-- carried across byte-for-byte; nothing is re-downloaded and nothing is
-- deleted except rows that are already orphaned, which are preserved in the
-- recovery tables instead of being silently discarded.
--
-- The runner executes this inside one BEGIN IMMEDIATE transaction with
-- foreign_keys temporarily OFF (standard SQLite table-rebuild procedure) and
-- runs PRAGMA foreign_key_check before committing.

-- ---------------------------------------------------------------------------
-- 1. Recovery tables: orphaned rows are preserved, never silently discarded.
-- ---------------------------------------------------------------------------
CREATE TABLE IF NOT EXISTS migration_recovery_report (
  at        INTEGER NOT NULL,
  kind      TEXT NOT NULL,
  row_count INTEGER NOT NULL,
  detail    TEXT
);
CREATE TABLE IF NOT EXISTS migration_recovery_rows (
  at       INTEGER NOT NULL,
  kind     TEXT NOT NULL,
  row_json TEXT NOT NULL
);

-- ---------------------------------------------------------------------------
-- 2. Reconcile account-level orphans (a delete that ran on a connection
--    without foreign-key enforcement left these behind). Without this step
--    PRAGMA foreign_key_check could never pass.
-- ---------------------------------------------------------------------------
INSERT INTO migration_recovery_rows (at, kind, row_json)
SELECT strftime('%s','now') * 1000, 'labels',
       json_object('account_id', account_id, 'id', id, 'name', name, 'kind', kind,
                   'color_bg', color_bg, 'color_fg', color_fg, 'visible', visible,
                   'unread_count', unread_count, 'total_count', total_count,
                   'sort_order', sort_order)
FROM labels WHERE account_id NOT IN (SELECT id FROM accounts);
DELETE FROM labels WHERE account_id NOT IN (SELECT id FROM accounts);

INSERT INTO migration_recovery_rows (at, kind, row_json)
SELECT strftime('%s','now') * 1000, 'threads',
       json_object('account_id', account_id, 'id', id, 'subject', subject, 'snippet', snippet,
                   'last_message_at', last_message_at, 'first_message_at', first_message_at,
                   'message_count', message_count, 'unread_count', unread_count,
                   'is_starred', is_starred, 'has_attachments', has_attachments,
                   'participants', participants, 'label_ids', label_ids, 'in_inbox', in_inbox,
                   'in_trash', in_trash, 'in_spam', in_spam, 'is_draft_only', is_draft_only,
                   'snoozed_until', snoozed_until)
FROM threads WHERE account_id NOT IN (SELECT id FROM accounts);
DELETE FROM threads WHERE account_id NOT IN (SELECT id FROM accounts);

INSERT INTO migration_recovery_rows (at, kind, row_json)
SELECT strftime('%s','now') * 1000, 'imap_uids',
       json_object('account_id', account_id, 'role', role, 'uid', uid, 'message_id', message_id)
FROM imap_uids
WHERE (account_id, role) NOT IN (SELECT account_id, role FROM imap_folders);
DELETE FROM imap_uids
WHERE (account_id, role) NOT IN (SELECT account_id, role FROM imap_folders);

INSERT INTO migration_recovery_rows (at, kind, row_json)
SELECT strftime('%s','now') * 1000, 'imap_folders',
       json_object('account_id', account_id, 'role', role, 'name', name,
                   'uidvalidity', uidvalidity, 'uidnext', uidnext,
                   'highestmodseq', highestmodseq, 'exists_count', exists_count,
                   'last_full_scan', last_full_scan)
FROM imap_folders WHERE account_id NOT IN (SELECT id FROM accounts);
DELETE FROM imap_folders WHERE account_id NOT IN (SELECT id FROM accounts);

INSERT INTO migration_recovery_rows (at, kind, row_json)
SELECT strftime('%s','now') * 1000, 'messages',
       json_object('id', id, 'account_id', account_id, 'thread_id', thread_id,
                   'history_id', history_id, 'internal_date', internal_date,
                   'from_name', from_name, 'from_email', from_email, 'to_json', to_json,
                   'cc_json', cc_json, 'bcc_json', bcc_json, 'reply_to', reply_to,
                   'subject', subject, 'snippet', snippet, 'rfc_message_id', rfc_message_id,
                   'in_reply_to', in_reply_to, 'references_json', references_json,
                   'list_unsubscribe', list_unsubscribe,
                   'list_unsubscribe_post', list_unsubscribe_post,
                   'size_estimate', size_estimate, 'has_attachments', has_attachments,
                   'is_unread', is_unread, 'is_starred', is_starred, 'is_draft', is_draft,
                   'is_sent_by_me', is_sent_by_me, 'label_ids', label_ids,
                   'body_state', body_state, 'fetched_at', fetched_at)
FROM messages WHERE account_id NOT IN (SELECT id FROM accounts);
DELETE FROM messages WHERE account_id NOT IN (SELECT id FROM accounts);

-- ---------------------------------------------------------------------------
-- 3. Reconcile message-child orphans (children whose parent no longer exists).
-- ---------------------------------------------------------------------------
INSERT INTO migration_recovery_rows (at, kind, row_json)
SELECT strftime('%s','now') * 1000, 'message_labels',
       json_object('message_id', message_id, 'label_id', label_id)
FROM message_labels WHERE message_id NOT IN (SELECT id FROM messages);
DELETE FROM message_labels WHERE message_id NOT IN (SELECT id FROM messages);

INSERT INTO migration_recovery_rows (at, kind, row_json)
SELECT strftime('%s','now') * 1000, 'bodies',
       json_object('message_id', message_id, 'html_z_hex', CASE WHEN html_z IS NULL THEN NULL ELSE hex(html_z) END,
                   'text_z_hex', CASE WHEN text_z IS NULL THEN NULL ELSE hex(text_z) END,
                   'remote_image_count', remote_image_count, 'tracker_count', tracker_count,
                   'dark_safe', dark_safe, 'quoted_from', quoted_from)
FROM bodies WHERE message_id NOT IN (SELECT id FROM messages);
DELETE FROM bodies WHERE message_id NOT IN (SELECT id FROM messages);

INSERT INTO migration_recovery_rows (at, kind, row_json)
SELECT strftime('%s','now') * 1000, 'attachments',
       json_object('id', id, 'message_id', message_id, 'gmail_att_id', gmail_att_id,
                   'part_id', part_id, 'filename', filename, 'mime', mime, 'size', size,
                   'content_id', content_id, 'is_inline', is_inline,
                   'data_z_hex', CASE WHEN data_z IS NULL THEN NULL ELSE hex(data_z) END,
                   'local_path', local_path, 'cache_state', cache_state,
                   'decoded_size', decoded_size, 'last_accessed_at', last_accessed_at,
                   'cache_version', cache_version)
FROM attachments WHERE message_id NOT IN (SELECT id FROM messages);
DELETE FROM attachments WHERE message_id NOT IN (SELECT id FROM messages);

-- ---------------------------------------------------------------------------
-- 4. Rebuild `messages` with PRIMARY KEY (account_id, id), preserving every
--    non-key column added through 0007.
-- ---------------------------------------------------------------------------
CREATE TABLE messages_new (
  id                TEXT NOT NULL,
  account_id        TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  thread_id         TEXT NOT NULL,
  history_id        TEXT,
  internal_date     INTEGER NOT NULL,
  from_name         TEXT,
  from_email        TEXT,
  to_json           TEXT NOT NULL DEFAULT '[]',
  cc_json           TEXT NOT NULL DEFAULT '[]',
  bcc_json          TEXT NOT NULL DEFAULT '[]',
  reply_to          TEXT,
  subject           TEXT NOT NULL DEFAULT '',
  snippet           TEXT NOT NULL DEFAULT '',
  rfc_message_id    TEXT,
  in_reply_to       TEXT,
  references_json   TEXT NOT NULL DEFAULT '[]',
  list_unsubscribe  TEXT,
  list_unsubscribe_post INTEGER NOT NULL DEFAULT 0,
  size_estimate     INTEGER,
  has_attachments   INTEGER NOT NULL DEFAULT 0,
  is_unread         INTEGER NOT NULL DEFAULT 0,
  is_starred        INTEGER NOT NULL DEFAULT 0,
  is_draft          INTEGER NOT NULL DEFAULT 0,
  is_sent_by_me     INTEGER NOT NULL DEFAULT 0,
  label_ids         TEXT NOT NULL DEFAULT '[]',
  body_state        TEXT NOT NULL DEFAULT 'none',
  fetched_at        INTEGER,
  PRIMARY KEY (account_id, id)
);

INSERT INTO messages_new (
  id, account_id, thread_id, history_id, internal_date, from_name, from_email,
  to_json, cc_json, bcc_json, reply_to, subject, snippet, rfc_message_id,
  in_reply_to, references_json, list_unsubscribe, list_unsubscribe_post,
  size_estimate, has_attachments, is_unread, is_starred, is_draft, is_sent_by_me,
  label_ids, body_state, fetched_at
)
SELECT
  id, account_id, thread_id, history_id, internal_date, from_name, from_email,
  to_json, cc_json, bcc_json, reply_to, subject, snippet, rfc_message_id,
  in_reply_to, references_json, list_unsubscribe, list_unsubscribe_post,
  size_estimate, has_attachments, is_unread, is_starred, is_draft, is_sent_by_me,
  label_ids, body_state, fetched_at
FROM messages;

DROP TABLE messages;
ALTER TABLE messages_new RENAME TO messages;
CREATE INDEX messages_thread_idx ON messages(account_id, thread_id, internal_date);
CREATE INDEX messages_body_state_idx ON messages(account_id, body_state, internal_date DESC);

-- ---------------------------------------------------------------------------
-- 5. Rebuild `message_labels` keyed by (account_id, message_id, label_id).
-- ---------------------------------------------------------------------------
CREATE TABLE message_labels_new (
  account_id TEXT NOT NULL,
  message_id TEXT NOT NULL,
  label_id   TEXT NOT NULL,
  PRIMARY KEY (account_id, message_id, label_id),
  FOREIGN KEY (account_id, message_id) REFERENCES messages(account_id, id) ON DELETE CASCADE
);
INSERT INTO message_labels_new (account_id, message_id, label_id)
SELECT m.account_id, ml.message_id, ml.label_id
FROM message_labels ml JOIN messages m ON m.id = ml.message_id;
DROP INDEX IF EXISTS message_labels_label_idx;
DROP TABLE message_labels;
ALTER TABLE message_labels_new RENAME TO message_labels;
CREATE INDEX message_labels_label_idx ON message_labels(account_id, label_id, message_id);

-- ---------------------------------------------------------------------------
-- 6. Rebuild `bodies` keyed by (account_id, message_id). Payloads are copied
--    as-is: cached mail stays offline-readable (0006's wholesale body delete
--    is not repeated).
-- ---------------------------------------------------------------------------
CREATE TABLE bodies_new (
  account_id        TEXT NOT NULL,
  message_id        TEXT NOT NULL,
  html_z            BLOB,
  text_z            BLOB,
  remote_image_count INTEGER NOT NULL DEFAULT 0,
  tracker_count     INTEGER NOT NULL DEFAULT 0,
  dark_safe         INTEGER NOT NULL DEFAULT 1,
  quoted_from       INTEGER,
  PRIMARY KEY (account_id, message_id),
  FOREIGN KEY (account_id, message_id) REFERENCES messages(account_id, id) ON DELETE CASCADE
);
INSERT INTO bodies_new (account_id, message_id, html_z, text_z, remote_image_count, tracker_count, dark_safe, quoted_from)
SELECT m.account_id, b.message_id, b.html_z, b.text_z, b.remote_image_count, b.tracker_count, b.dark_safe, b.quoted_from
FROM bodies b JOIN messages m ON m.id = b.message_id;
DROP TABLE bodies;
ALTER TABLE bodies_new RENAME TO bodies;

-- ---------------------------------------------------------------------------
-- 7. Rebuild `attachments`: stable row ids kept, PRIMARY KEY (account_id, id),
--    natural uniqueness widened from (message_id, part_id).
-- ---------------------------------------------------------------------------
CREATE TABLE attachments_new (
  id            TEXT NOT NULL,
  account_id    TEXT NOT NULL,
  message_id    TEXT NOT NULL,
  gmail_att_id  TEXT,
  part_id       TEXT NOT NULL,
  filename      TEXT,
  mime          TEXT NOT NULL,
  size          INTEGER NOT NULL DEFAULT 0,
  content_id    TEXT,
  is_inline     INTEGER NOT NULL DEFAULT 0,
  data_z        BLOB,
  local_path    TEXT,
  cache_state   TEXT NOT NULL DEFAULT 'missing',
  decoded_size  INTEGER,
  last_accessed_at INTEGER,
  cache_version INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (account_id, id),
  FOREIGN KEY (account_id, message_id) REFERENCES messages(account_id, id) ON DELETE CASCADE
);
INSERT INTO attachments_new (
  id, account_id, message_id, gmail_att_id, part_id, filename, mime, size, content_id,
  is_inline, data_z, local_path, cache_state, decoded_size, last_accessed_at, cache_version
)
SELECT
  a.id, m.account_id, a.message_id, a.gmail_att_id, a.part_id, a.filename, a.mime, a.size,
  a.content_id, a.is_inline, a.data_z, a.local_path, a.cache_state, a.decoded_size,
  a.last_accessed_at, a.cache_version
FROM attachments a JOIN messages m ON m.id = a.message_id;
DROP INDEX IF EXISTS attachments_msg_idx;
DROP INDEX IF EXISTS attachments_msg_part_uidx;
DROP TABLE attachments;
ALTER TABLE attachments_new RENAME TO attachments;
CREATE INDEX attachments_msg_idx ON attachments(account_id, message_id);
CREATE UNIQUE INDEX attachments_msg_part_uidx ON attachments(account_id, message_id, part_id);

-- ---------------------------------------------------------------------------
-- 7b. Upgrade `sender_prefs` to account/sender scope. Legacy rows were
--     global; to preserve the user's explicit choice, each one is copied to
--     every existing account (the preference now lives per account).
-- ---------------------------------------------------------------------------
CREATE TABLE sender_prefs_new (
  account_id          TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  email               TEXT NOT NULL,
  allow_remote_images INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (account_id, email)
);
INSERT INTO sender_prefs_new (account_id, email, allow_remote_images)
SELECT a.id, p.email, p.allow_remote_images
FROM sender_prefs p CROSS JOIN accounts a;
DROP TABLE sender_prefs;
ALTER TABLE sender_prefs_new RENAME TO sender_prefs;

-- ---------------------------------------------------------------------------
-- 8. Rebuild FTS with account keys, after the row copy and before commit.
--    Body text is decompressed from the (preserved) compressed payload so the
--    index keeps matching cached mail; `zstd_text` is registered on the
--    migration connection.
-- ---------------------------------------------------------------------------
DROP TABLE messages_fts;
CREATE VIRTUAL TABLE messages_fts USING fts5(
  message_id UNINDEXED, account_id UNINDEXED,
  subject, from_text, to_text, body,
  tokenize='unicode61 remove_diacritics 2', prefix='2 3'
);
INSERT INTO messages_fts (message_id, account_id, subject, from_text, to_text, body)
SELECT
  m.id,
  m.account_id,
  m.subject,
  COALESCE(m.from_name, '') || ' ' || COALESCE(m.from_email, ''),
  m.to_json,
  COALESCE(zstd_text((SELECT b.text_z FROM bodies b
                       WHERE b.account_id = m.account_id AND b.message_id = m.id)), '')
FROM messages m;

-- ---------------------------------------------------------------------------
-- 9. Record what was rehomed.
-- ---------------------------------------------------------------------------
INSERT INTO migration_recovery_report (at, kind, row_count, detail)
SELECT strftime('%s','now') * 1000, kind, count(*),
       'orphaned rows preserved in migration_recovery_rows during 0008_account_scoping'
FROM migration_recovery_rows
GROUP BY kind;
