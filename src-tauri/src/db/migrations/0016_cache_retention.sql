-- 0016_cache_retention (P9.3 raw source cache, P10.4 retention policy)
--
-- Additive only. Nothing is deleted here: this migration only makes the
-- retention rules expressible (a pin flag, an access-time index and a place to
-- record raw source that the user asked to see).

-- ---------------------------------------------------------------------------
-- P9.3 Cached raw MIME.
--
-- `message_raw_source` used to fetch from the provider on every call, so View
-- Source needed a network round trip and could not work offline. The bytes are
-- now cached on disk (never in SQLite: a message can be megabytes) and the row
-- records what is on disk plus the digest, so `Save as .eml` can prove it wrote
-- the exact bytes that were cached.
--
--   sha256            hex digest of `path`, verified before an export
--   last_accessed_at  drives LRU eviction together with the attachment cache
CREATE TABLE message_raw (
  account_id       TEXT NOT NULL,
  message_id       TEXT NOT NULL,
  path             TEXT NOT NULL,
  size             INTEGER NOT NULL,
  sha256           TEXT NOT NULL,
  cached_at        INTEGER NOT NULL,
  last_accessed_at INTEGER NOT NULL,
  PRIMARY KEY (account_id, message_id),
  FOREIGN KEY (account_id, message_id) REFERENCES messages(account_id, id) ON DELETE CASCADE
);

CREATE INDEX message_raw_lru_idx ON message_raw(last_accessed_at);

-- ---------------------------------------------------------------------------
-- P10.4 Attachment cache retention.
--
-- `pinned_at` is the user's "keep this offline" flag. A pinned file may be
-- evicted by nothing except the user unpinning it or clearing the cache
-- explicitly; the default 512 MiB cap is a target for the *unpinned* cache.
ALTER TABLE attachments ADD COLUMN pinned_at INTEGER;

-- The LRU walk reads ready files in access order. Partial, because a row with
-- no file on disk has nothing to evict and should not be scanned.
CREATE INDEX attachments_lru_idx
  ON attachments(last_accessed_at) WHERE local_path IS NOT NULL;

CREATE INDEX attachments_pinned_idx
  ON attachments(account_id, pinned_at) WHERE pinned_at IS NOT NULL;
