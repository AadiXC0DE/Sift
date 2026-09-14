-- 0011_saved_searches (P7.4)
--
-- A saved search is a validated query plus an account scope. It stores no
-- message, thread, label or body: deleting one can never move or delete mail,
-- and a saved mailbox costs at most one indexed query when it is opened.
--
-- `account_scope_json` is the explicit list of accounts the search was saved
-- against. An account that is later removed stays in the list and is reported
-- as missing, so the scope is offered for editing instead of silently widening
-- to every account.
CREATE TABLE saved_searches (
  id                 TEXT PRIMARY KEY,
  name               TEXT NOT NULL,
  query              TEXT NOT NULL,
  account_scope_json TEXT NOT NULL,
  sort_order         INTEGER NOT NULL,
  created_at         INTEGER NOT NULL
);
