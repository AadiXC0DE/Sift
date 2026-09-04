-- Backfill contacts_fts triggers (runs once). Actual backfill runs in background task.
CREATE TRIGGER IF NOT EXISTS contacts_ai AFTER INSERT ON contacts BEGIN
  INSERT INTO contacts_fts(rowid, email, name) VALUES (new.rowid, new.email, new.name);
END;
CREATE TRIGGER IF NOT EXISTS contacts_ad AFTER DELETE ON contacts BEGIN
  INSERT INTO contacts_fts(contacts_fts, rowid, email, name) VALUES('delete', old.rowid, old.email, old.name);
END;
