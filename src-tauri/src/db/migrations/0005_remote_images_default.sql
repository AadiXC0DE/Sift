-- Remote images now load by default like any other email client.
-- The per-message "Ask" gate was removed; normalize legacy 'ask' to 'always'.
-- An explicit 'never' privacy choice is preserved.
UPDATE settings
SET value = json_set(value, '$.remoteImages', 'always')
WHERE key = 'settings'
  AND json_valid(value)
  AND json_extract(value, '$.remoteImages') = 'ask';
