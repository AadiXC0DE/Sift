-- 0006_rerender_bodies
--
-- The rendering pipeline changed: plain text keeps whitespace and quote
-- markers, HTML head/title text is scrubbed, body presentation is restored,
-- and Gmail bodies honor their declared charset. Bodies sanitized by older
-- builds are stored output and cannot be fixed in place, so drop them and let
-- the foreground fetch and body backfill rebuild each message with the
-- current renderer.
DELETE FROM bodies;
UPDATE messages SET body_state = 'none', fetched_at = NULL;
