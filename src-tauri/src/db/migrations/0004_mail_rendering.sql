-- Bodies stored before the fidelity renderer were irreversibly altered by the
-- old sanitizer. Refetch them on demand so existing mail benefits from the fix.
DELETE FROM bodies;
UPDATE messages SET body_state = 'none', fetched_at = NULL WHERE body_state = 'fetched';
UPDATE messages_fts SET body = '';
-- Fidelity is the default, but preserve an explicit "never" privacy choice.
UPDATE settings
SET value = json_set(value, '$.remoteImages', 'always')
WHERE key = 'settings'
  AND json_valid(value)
  AND COALESCE(json_extract(value, '$.remoteImages'), 'ask') != 'never';
