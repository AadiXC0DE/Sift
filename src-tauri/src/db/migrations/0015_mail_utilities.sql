-- 0015_mail_utilities (P8.2, P8.3, P8.4)
--
-- Appendix A names this migration `0012_mail_utilities`. The repository had
-- already shipped `0012_search_indexes` (Phase 7) by the time Phase 8 landed,
-- and a shipped migration is never edited, so the utilities land in the next
-- free slot. The inventory entry is: reminders, rules, rule applications and
-- VIPs; the notification delivery log is added here with them because it is
-- the same feature (exactly one native notification per ingested message).
--
-- Additive only: nothing existing is rewritten, and every table is empty on
-- upgrade, so a new feature starts off with no records and therefore no UI.

-- ---------------------------------------------------------------------------
-- P8.2 Reminders.
--
-- A reminder is local scheduling that leaves the message where it is: it must
-- never archive, mark unread or move a thread. It is therefore a *separate*
-- table from `snoozes`, which does change Inbox membership.
--
--   remind_at      the user's chosen local deadline, stored as UTC ms
--   delivered_at   written durably BEFORE the OS notification is raised, so a
--                  crash between the two cannot produce a second notification
--   completed_at   set when the user dismisses it; the row stays so a compact
--                  "done" state survives a restart
--
-- The composite foreign key to `threads` is the point of the table: deleting
-- the target thread (last message gone, permanent delete, thread merge) cancels
-- the reminder by itself, and removing the account cascades through
-- `threads`/`accounts` without a manual cleanup list that could drift.
CREATE TABLE reminders (
  account_id   TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  thread_id    TEXT NOT NULL,
  remind_at    INTEGER NOT NULL,
  delivered_at INTEGER,
  completed_at INTEGER,
  created_at   INTEGER NOT NULL,
  PRIMARY KEY (account_id, thread_id),
  FOREIGN KEY (account_id, thread_id) REFERENCES threads(account_id, id) ON DELETE CASCADE
);

-- The scheduler asks for the nearest open deadline on every tick.
CREATE INDEX reminders_due_idx ON reminders(remind_at) WHERE completed_at IS NULL;

-- ---------------------------------------------------------------------------
-- P8.3 Local rules and sender blocking.
--
-- Rules are off by default (`enabled` defaults to 0) and are *local*: Sift
-- evaluates them when it syncs. Nothing here can run a script, match a regex,
-- auto-reply, forward, delete permanently or fetch a URL; the condition and
-- action vocabulary is closed and validated in `crate::rules`.
--
--   conditions_json  [{"field":"sender|recipient|subject|hasAttachment",
--                      "op":"contains|is|domain|isTrue","value":"..."}]
--   actions_json     [{"kind":"addLabel|archive|markRead|star|junk",
--                      "labelId":null}]
--   match_mode       "all" | "any"
--   revision         bumped on every edit, so a rule application is bound to
--                    the exact version that produced it
--   last_error       why a rule disabled itself (e.g. its label was deleted)
CREATE TABLE mail_rules (
  account_id      TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  id              TEXT NOT NULL,
  name            TEXT NOT NULL,
  enabled         INTEGER NOT NULL DEFAULT 0,
  match_mode      TEXT NOT NULL DEFAULT 'all',
  conditions_json TEXT NOT NULL DEFAULT '[]',
  actions_json    TEXT NOT NULL DEFAULT '[]',
  sort_order      INTEGER NOT NULL DEFAULT 0,
  revision        INTEGER NOT NULL DEFAULT 1,
  last_error      TEXT,
  created_at      INTEGER NOT NULL,
  updated_at      INTEGER NOT NULL,
  PRIMARY KEY (account_id, id)
);

CREATE INDEX mail_rules_order_idx ON mail_rules(account_id, enabled, sort_order, id);

-- One row per (rule revision, message). The primary key IS the idempotency
-- contract: the same revision of a rule can never be applied twice to one
-- message, however many times sync or the app restarts.
CREATE TABLE rule_applications (
  rule_id    TEXT NOT NULL,
  revision   INTEGER NOT NULL,
  account_id TEXT NOT NULL,
  message_id TEXT NOT NULL,
  applied_at INTEGER NOT NULL,
  PRIMARY KEY (rule_id, revision, account_id, message_id)
);

CREATE INDEX rule_applications_msg_idx ON rule_applications(account_id, message_id);

-- Newly ingested messages waiting for rule evaluation. The row is written in
-- the same transaction as the message itself (so evaluation always sees a
-- committed row) and drained later, outside the first-page path. Only mail
-- that is genuinely new *and* arrives after the account's first sync is
-- queued, so an initial sync cannot surprise the user by acting on history.
CREATE TABLE rule_queue (
  account_id TEXT NOT NULL,
  message_id TEXT NOT NULL,
  queued_at  INTEGER NOT NULL,
  PRIMARY KEY (account_id, message_id)
);

CREATE INDEX rule_queue_order_idx ON rule_queue(account_id, queued_at, message_id);

-- ---------------------------------------------------------------------------
-- P8.4 VIP senders and the notification delivery log.
--
-- VIPs are chosen from the recent correspondents Sift already knows about
-- (`contacts`), so no address-book permission is needed.
CREATE TABLE vip_senders (
  account_id TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  email      TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (account_id, email)
);

-- Exactly one native notification per ingested message. A repeated partial
-- sync (or a restart replaying the same changed set) finds the row already
-- there and stays quiet, which is what makes "no permission prompt loop" and
-- "one message, one notification" true across restarts.
CREATE TABLE notify_log (
  account_id  TEXT NOT NULL,
  message_id  TEXT NOT NULL,
  thread_id   TEXT NOT NULL,
  notified_at INTEGER NOT NULL,
  PRIMARY KEY (account_id, message_id)
);

CREATE INDEX notify_log_sweep_idx ON notify_log(notified_at);
