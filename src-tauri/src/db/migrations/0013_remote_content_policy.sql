-- 0013_remote_content_policy (P9.1)
--
-- The remote-content policy becomes an explicit choice: block | ask | allow.
-- A newly configured installation defaults to `ask` (see Settings::default);
-- this migration only has work to do for a database that already exists.
--
-- 0004 and 0005 rewrote a legacy `ask` to `always` without recording whether
-- the user had chosen that, so an existing `always` cannot be distinguished
-- from a deliberate one. Sift therefore:
--
--   * preserves a deliberate `never` exactly, as `block`;
--   * keeps the behaviour an ambiguous record already has (`allow`) and marks
--     the one-time compact privacy choice as pending, so the upgrade changes
--     nothing silently and the user answers the question once.
--
-- A database with no settings row at all has no recorded choice: it keeps the
-- new default of `ask`.
UPDATE settings
SET value = json_set(
      value,
      '$.remoteContentMode',
      CASE
        WHEN json_extract(value, '$.remoteImages') = 'never' THEN 'block'
        ELSE 'allow'
      END,
      '$.remoteContentChoicePending',
      json(
        CASE
          WHEN json_extract(value, '$.remoteImages') = 'never' THEN 'false'
          ELSE 'true'
        END
      )
    )
WHERE key = 'settings'
  AND json_valid(value)
  AND json_extract(value, '$.remoteContentMode') IS NULL;
