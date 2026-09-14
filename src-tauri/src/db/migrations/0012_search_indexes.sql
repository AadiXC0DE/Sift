-- 0012_search_indexes (P7.3)
--
-- Indexes the recorded query plans justify.
--
-- Search applies `is:read`, `has:attachment` and date bounds to `messages`
-- before the LIMIT, inside one grouped candidate query. Each filter gets an
-- account-prefixed index; the planner picks the one matching its chosen
-- predicate. The recorded plan for
-- `quarterly from:ada is:read before:2030-01-01 has:attachment` is
--
--   SEARCH m USING INDEX messages_attachment_date_idx (account_id=? AND has_attachments=? AND internal_date<?)
--
-- so the attachment index is the one proven in use; the other two are the
-- same shape for the same stage and cover the queries that filter on read
-- state or on a date alone.
CREATE INDEX messages_account_date_idx ON messages(account_id, internal_date DESC);
CREATE INDEX messages_unread_date_idx ON messages(account_id, is_unread, internal_date DESC);
CREATE INDEX messages_attachment_date_idx ON messages(account_id, has_attachments, internal_date DESC);

-- The snoozed list is a partial index on exactly the tuple it orders and keys
-- by: `(snoozed_until ASC, account_id ASC, id ASC)`. Only snoozed threads are
-- in it, and every row of the view is one of them.
CREATE INDEX threads_snooze_order_idx ON threads(snoozed_until ASC, account_id ASC, id ASC)
  WHERE snoozed_until IS NOT NULL;

-- The default list does NOT get a new index: the existing
-- `threads_inbox_idx`/`threads_last_idx` already provide the account prefix,
-- and the recorded plan uses them:
--
--   SEARCH t USING INDEX threads_inbox_idx (account_id=? AND in_inbox=? AND last_message_at<?)
--
-- No OFFSET is used anywhere, so no index is added to support paging into the
-- middle of a mailbox.
