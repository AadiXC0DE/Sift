-- 0007_attachment_cache (P2.2)
--
-- `(message_id, part_id)` is the natural key for an attachment. Before this
-- migration `attachments_put` early-returned for any matching pair, so a
-- metadata-only row (parsed from BODYSTRUCTURE) could never be enriched with
-- the bytes and locators that a later full-body parse produced. Duplicate rows
-- were also possible.
--
-- The table is rebuilt rather than ALTERed so the dedupe and the new columns
-- land in one transaction. Per duplicate group we keep the first stable row id
-- (lowest rowid) and merge the group's richest metadata: the first non-empty
-- filename/mime/content-id/transport locator, the largest declared size, the
-- first stored payload, and the first cache file. Rows that already carry
-- bytes or a file start `unverified`: readable, verified lazily on first use.
-- A zero-length decoded payload is a valid attachment and stays `unverified`
-- with its compressed frame intact.

CREATE TABLE attachments_new (
  id               TEXT PRIMARY KEY,
  message_id       TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  gmail_att_id     TEXT,
  part_id          TEXT NOT NULL,
  filename         TEXT,
  mime             TEXT NOT NULL,
  size             INTEGER NOT NULL DEFAULT 0,
  content_id       TEXT,
  is_inline        INTEGER NOT NULL DEFAULT 0,
  data_z           BLOB,
  local_path       TEXT,
  cache_state      TEXT NOT NULL DEFAULT 'missing',
  decoded_size     INTEGER,
  last_accessed_at INTEGER,
  cache_version    INTEGER NOT NULL DEFAULT 1
);

INSERT INTO attachments_new (
  id, message_id, gmail_att_id, part_id, filename, mime, size, content_id,
  is_inline, data_z, local_path, cache_state, decoded_size, last_accessed_at,
  cache_version
)
SELECT
  g.keep_id,
  g.message_id,
  g.gmail_att_id,
  g.part_id,
  g.filename,
  g.mime,
  g.size,
  g.content_id,
  g.is_inline,
  g.data_z,
  g.local_path,
  CASE
    WHEN g.data_z IS NOT NULL OR IFNULL(g.local_path, '') <> '' THEN 'unverified'
    ELSE 'missing'
  END,
  NULL,
  NULL,
  1
FROM (
  SELECT
    (SELECT a0.id FROM attachments a0
      WHERE a0.message_id = a.message_id AND a0.part_id = a.part_id
      ORDER BY a0.rowid LIMIT 1)                                         AS keep_id,
    a.message_id                                                          AS message_id,
    a.part_id                                                             AS part_id,
    (SELECT a1.gmail_att_id FROM attachments a1
      WHERE a1.message_id = a.message_id AND a1.part_id = a.part_id
        AND a1.gmail_att_id IS NOT NULL AND a1.gmail_att_id <> ''
      ORDER BY a1.rowid LIMIT 1)                                         AS gmail_att_id,
    (SELECT a2.filename FROM attachments a2
      WHERE a2.message_id = a.message_id AND a2.part_id = a.part_id
        AND a2.filename IS NOT NULL AND a2.filename <> ''
      ORDER BY a2.rowid LIMIT 1)                                         AS filename,
    COALESCE(MAX(NULLIF(a.mime, '')), 'application/octet-stream')         AS mime,
    MAX(a.size)                                                           AS size,
    (SELECT a3.content_id FROM attachments a3
      WHERE a3.message_id = a.message_id AND a3.part_id = a.part_id
        AND a3.content_id IS NOT NULL AND a3.content_id <> ''
      ORDER BY a3.rowid LIMIT 1)                                         AS content_id,
    MAX(a.is_inline)                                                      AS is_inline,
    (SELECT a4.data_z FROM attachments a4
      WHERE a4.message_id = a.message_id AND a4.part_id = a.part_id
        AND a4.data_z IS NOT NULL
      ORDER BY a4.rowid LIMIT 1)                                         AS data_z,
    (SELECT a5.local_path FROM attachments a5
      WHERE a5.message_id = a.message_id AND a5.part_id = a.part_id
        AND a5.local_path IS NOT NULL AND a5.local_path <> ''
      ORDER BY a5.rowid LIMIT 1)                                         AS local_path
  FROM attachments a
  GROUP BY a.message_id, a.part_id
) AS g;

DROP INDEX IF EXISTS attachments_msg_idx;
DROP TABLE attachments;
ALTER TABLE attachments_new RENAME TO attachments;

CREATE INDEX attachments_msg_idx ON attachments(message_id);
CREATE UNIQUE INDEX attachments_msg_part_uidx ON attachments(message_id, part_id);
