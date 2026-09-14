-- 0014_unsubscribe_auth (P9.4)
--
-- One-click unsubscribe (RFC 8058) needs the *exact* `List-Unsubscribe-Post`
-- field value and evidence that the headers really came from the receiving
-- provider. The old schema kept only a derived boolean, so Sift could not tell
-- `List-Unsubscribe=One-Click` from anything else, and had no authentication
-- evidence at all.
--
-- `auth_results` stores the `Authentication-Results` header as the provider
-- returned it, and `auth_results_trusted` records whether the provider itself
-- produced that header (the Gmail API) rather than the message merely
-- containing one. Sift never infers authenticity from a sender-supplied
-- header: without trusted evidence a one-click POST is refused and the user
-- gets an open-link or `mailto:` flow instead.
ALTER TABLE messages ADD COLUMN list_unsubscribe_post_value TEXT;
ALTER TABLE messages ADD COLUMN auth_results TEXT;
ALTER TABLE messages ADD COLUMN auth_results_trusted INTEGER NOT NULL DEFAULT 0;
