-- Fidelity is the default, but preserve existing cached mail and an explicit
-- "never" privacy choice. Bodies are refreshed naturally when providers sync;
-- never destroy the only readable copy during an app upgrade.
UPDATE settings
SET value = json_set(value, '$.remoteImages', 'always')
WHERE key = 'settings'
  AND json_valid(value)
  AND COALESCE(json_extract(value, '$.remoteImages'), 'ask') != 'never';
