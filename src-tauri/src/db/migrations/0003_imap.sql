ALTER TABLE accounts ADD COLUMN auth_kind TEXT NOT NULL DEFAULT 'oauth'; -- oauth|app_password

CREATE TABLE imap_folders (
  account_id     TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  role           TEXT NOT NULL,             -- all|trash|junk|drafts|sent|inbox
  name           TEXT NOT NULL,             -- server folder name (localized, e.g. "[Gmail]/Alle Nachrichten")
  uidvalidity    INTEGER NOT NULL,
  uidnext        INTEGER NOT NULL,
  highestmodseq  INTEGER,                   -- NULL when CONDSTORE is unavailable
  exists_count   INTEGER NOT NULL DEFAULT 0,
  last_full_scan INTEGER,                   -- unix ms of the last UID-set reconciliation
  PRIMARY KEY (account_id, role)
);

CREATE TABLE imap_uids (
  account_id  TEXT NOT NULL,
  role        TEXT NOT NULL,
  uid         INTEGER NOT NULL,
  message_id  TEXT NOT NULL,                -- hex(X-GM-MSGID) == messages.id
  PRIMARY KEY (account_id, role, uid),
  FOREIGN KEY (account_id, role) REFERENCES imap_folders(account_id, role) ON DELETE CASCADE
);
CREATE INDEX imap_uids_by_message ON imap_uids(account_id, message_id);
