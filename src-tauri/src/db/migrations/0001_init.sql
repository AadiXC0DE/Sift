CREATE TABLE schema_version (version INTEGER NOT NULL);
INSERT INTO schema_version VALUES (1);

CREATE TABLE accounts (
  id            TEXT PRIMARY KEY,
  provider      TEXT NOT NULL DEFAULT 'gmail',
  email         TEXT NOT NULL UNIQUE,
  display_name  TEXT,
  avatar_url    TEXT,
  color         TEXT NOT NULL DEFAULT 'blue',
  history_id    TEXT,
  sync_state    TEXT NOT NULL DEFAULT 'new',
  last_sync_at  INTEGER,
  created_at    INTEGER NOT NULL,
  sort_order    INTEGER NOT NULL DEFAULT 0,
  signature_html TEXT
);

CREATE TABLE labels (
  account_id    TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  id            TEXT NOT NULL,
  name          TEXT NOT NULL,
  kind          TEXT NOT NULL,
  color_bg      TEXT, color_fg TEXT,
  visible       INTEGER NOT NULL DEFAULT 1,
  unread_count  INTEGER NOT NULL DEFAULT 0,
  total_count   INTEGER NOT NULL DEFAULT 0,
  sort_order    INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (account_id, id)
);

CREATE TABLE threads (
  account_id        TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  id                TEXT NOT NULL,
  subject           TEXT NOT NULL DEFAULT '',
  snippet           TEXT NOT NULL DEFAULT '',
  last_message_at   INTEGER NOT NULL,
  first_message_at  INTEGER NOT NULL,
  message_count     INTEGER NOT NULL DEFAULT 0,
  unread_count      INTEGER NOT NULL DEFAULT 0,
  is_starred        INTEGER NOT NULL DEFAULT 0,
  has_attachments   INTEGER NOT NULL DEFAULT 0,
  participants      TEXT NOT NULL DEFAULT '[]',
  label_ids         TEXT NOT NULL DEFAULT '[]',
  in_inbox          INTEGER NOT NULL DEFAULT 0,
  in_trash          INTEGER NOT NULL DEFAULT 0,
  in_spam           INTEGER NOT NULL DEFAULT 0,
  is_draft_only     INTEGER NOT NULL DEFAULT 0,
  snoozed_until     INTEGER,
  PRIMARY KEY (account_id, id)
);
CREATE INDEX threads_inbox_idx   ON threads(account_id, in_inbox, last_message_at DESC);
CREATE INDEX threads_last_idx    ON threads(account_id, last_message_at DESC);
CREATE INDEX threads_snooze_idx  ON threads(snoozed_until) WHERE snoozed_until IS NOT NULL;
CREATE INDEX threads_starred_idx ON threads(account_id, is_starred, last_message_at DESC);

CREATE TABLE messages (
  id                TEXT PRIMARY KEY,
  account_id        TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  thread_id         TEXT NOT NULL,
  history_id        TEXT,
  internal_date     INTEGER NOT NULL,
  from_name         TEXT, from_email TEXT,
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
  fetched_at        INTEGER
);
CREATE INDEX messages_thread_idx ON messages(account_id, thread_id, internal_date);
CREATE INDEX messages_body_state_idx ON messages(account_id, body_state, internal_date DESC);

CREATE TABLE message_labels (
  message_id TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  label_id   TEXT NOT NULL,
  PRIMARY KEY (message_id, label_id)
);
CREATE INDEX message_labels_label_idx ON message_labels(label_id, message_id);

CREATE TABLE bodies (
  message_id        TEXT PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
  html_z            BLOB,
  text_z            BLOB,
  remote_image_count INTEGER NOT NULL DEFAULT 0,
  tracker_count     INTEGER NOT NULL DEFAULT 0,
  dark_safe         INTEGER NOT NULL DEFAULT 1,
  quoted_from       INTEGER
);

CREATE TABLE attachments (
  id            TEXT PRIMARY KEY,
  message_id    TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  gmail_att_id  TEXT,
  part_id       TEXT NOT NULL,
  filename      TEXT,
  mime          TEXT NOT NULL,
  size          INTEGER NOT NULL DEFAULT 0,
  content_id    TEXT,
  is_inline     INTEGER NOT NULL DEFAULT 0,
  data_z        BLOB,
  local_path    TEXT
);
CREATE INDEX attachments_msg_idx ON attachments(message_id);

CREATE TABLE contacts (
  account_id    TEXT NOT NULL,
  email         TEXT NOT NULL,
  name          TEXT,
  last_used_at  INTEGER NOT NULL,
  use_count     INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (account_id, email)
);
CREATE VIRTUAL TABLE contacts_fts USING fts5(email, name, tokenize='trigram', content='');

CREATE TABLE outbox_ops (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id    TEXT NOT NULL,
  kind          TEXT NOT NULL,
  payload       TEXT NOT NULL,
  undo_group    TEXT,
  state         TEXT NOT NULL DEFAULT 'pending',
  attempts      INTEGER NOT NULL DEFAULT 0,
  not_before    INTEGER NOT NULL DEFAULT 0,
  last_error    TEXT,
  created_at    INTEGER NOT NULL
);
CREATE INDEX outbox_pending_idx ON outbox_ops(account_id, state, not_before, id);

CREATE TABLE drafts (
  local_id      TEXT PRIMARY KEY,
  account_id    TEXT NOT NULL,
  remote_draft_id TEXT,
  remote_message_id TEXT,
  thread_id     TEXT,
  in_reply_to_message_id TEXT,
  mode          TEXT NOT NULL,
  to_json TEXT NOT NULL DEFAULT '[]', cc_json TEXT NOT NULL DEFAULT '[]', bcc_json TEXT NOT NULL DEFAULT '[]',
  subject       TEXT NOT NULL DEFAULT '',
  body_html     TEXT NOT NULL DEFAULT '',
  attachments_json TEXT NOT NULL DEFAULT '[]',
  updated_at    INTEGER NOT NULL,
  dirty         INTEGER NOT NULL DEFAULT 1
);

CREATE TABLE snoozes (
  account_id TEXT NOT NULL, thread_id TEXT NOT NULL, wake_at INTEGER NOT NULL,
  PRIMARY KEY (account_id, thread_id)
);

CREATE TABLE sender_prefs (
  email TEXT PRIMARY KEY, allow_remote_images INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);

CREATE TABLE sync_log (
  id INTEGER PRIMARY KEY AUTOINCREMENT, account_id TEXT, at INTEGER NOT NULL,
  kind TEXT NOT NULL, detail TEXT
);

CREATE VIRTUAL TABLE messages_fts USING fts5(
  message_id UNINDEXED, account_id UNINDEXED,
  subject, from_text, to_text, body,
  tokenize='unicode61 remove_diacritics 2', prefix='2 3'
);
