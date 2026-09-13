use super::Db;
use crate::dto::Contact;
use anyhow::Result;
use rusqlite::params;

/// The trigram tokenizer cannot match anything shorter than three characters,
/// so shorter terms take the LIKE path instead of an expression FTS5 would
/// simply fail to match.
const TRIGRAM_MIN_CHARS: usize = 3;

fn row_to_contact(r: &rusqlite::Row) -> rusqlite::Result<Contact> {
    Ok(Contact {
        email: r.get(0)?,
        name: r.get(1)?,
        last_used_at: r.get(2)?,
        use_count: r.get(3)?,
    })
}

/// A quoted FTS5 phrase. Quoting is what makes user text safe: `%`, `_`, `*`,
/// `:` and `-` are LIKE/expression syntax, not literals, and passing them to
/// MATCH either matches nothing or errors. Only `"` needs escaping (doubled).
fn fts_phrase(q: &str) -> String {
    format!("\"{}\"", q.replace('"', "\"\""))
}

/// Escape the LIKE wildcards in user text so a `%` in a name searches for a
/// `%` instead of matching everything.
fn like_pattern(q: &str) -> String {
    let lower = q.to_lowercase();
    let escaped = lower
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{escaped}%")
}

impl Db {
    pub async fn contacts_upsert(
        &self,
        account_id: &str,
        email: &str,
        name: Option<&str>,
    ) -> Result<()> {
        let (a, e, n) = (
            account_id.to_string(),
            email.to_lowercase(),
            name.map(|s| s.to_string()),
        );
        let now = super::now_ms();
        self.write(move |c| {
      c.execute("INSERT INTO contacts (account_id,email,name,last_used_at,use_count) VALUES (?,?,?, ?,1) ON CONFLICT(account_id,email) DO UPDATE SET name=COALESCE(excluded.name, contacts.name), last_used_at=excluded.last_used_at, use_count=contacts.use_count+1",
        params![a,e,n,now])?;
      Ok(())
    }).await
    }

    /// Recent correspondents for the sending account.
    ///
    /// `contacts_fts` is contentless: the FTS row only supplies the rowid, and
    /// the displayed fields come from `contacts`, joined with the account
    /// filter. Suggestion is always scoped to one account, so a match in
    /// another account can never leak into the list (P5.5).
    pub async fn contacts_suggest(
        &self,
        account_id: &str,
        q: &str,
        limit: i64,
    ) -> Result<Vec<Contact>> {
        let a = account_id.to_string();
        let query = q.trim().to_string();
        let limit = limit.clamp(1, 50);
        self.read(move |c| {
            if query.is_empty() {
                let mut s = c.prepare(
                    "SELECT email,name,last_used_at,use_count FROM contacts WHERE account_id=? \
                     ORDER BY use_count DESC, last_used_at DESC LIMIT ?",
                )?;
                return Ok(s
                    .query_map(params![a, limit], row_to_contact)?
                    .collect::<Result<Vec<_>, _>>()?);
            }
            let mut out: Vec<Contact> = Vec::new();
            if query.chars().count() >= TRIGRAM_MIN_CHARS {
                let mut s = c.prepare(
                    "SELECT c.email, c.name, c.last_used_at, c.use_count \
                     FROM contacts_fts f JOIN contacts c ON c.rowid = f.rowid \
                     WHERE f.contacts_fts MATCH ? AND c.account_id = ? \
                     ORDER BY c.use_count DESC, c.last_used_at DESC LIMIT ?",
                )?;
                out = s
                    .query_map(params![fts_phrase(&query), a, limit], row_to_contact)?
                    .collect::<Result<Vec<_>, _>>()?;
            }
            if out.is_empty() {
                // Bounded fallback: short terms, and any term the trigram index
                // cannot serve. Still account-filtered, still limited.
                let like = like_pattern(&query);
                let mut s = c.prepare(
                    "SELECT email,name,last_used_at,use_count FROM contacts \
                     WHERE account_id=? \
                     AND (lower(email) LIKE ? ESCAPE '\\' OR lower(COALESCE(name,'')) LIKE ? ESCAPE '\\') \
                     ORDER BY use_count DESC, last_used_at DESC LIMIT ?",
                )?;
                out = s
                    .query_map(params![a, like, like, limit], row_to_contact)?
                    .collect::<Result<Vec<_>, _>>()?;
            }
            Ok(out)
        })
        .await
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn p7_t07_suggest_orders() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        db.contacts_upsert("a", "ada@x.com", Some("Ada Lovelace"))
            .await
            .unwrap();
        db.contacts_upsert("a", "ada@x.com", Some("Ada Lovelace"))
            .await
            .unwrap();
        db.contacts_upsert("a", "ben@y.org", Some("Ben"))
            .await
            .unwrap();
        let r = db.contacts_suggest("a", "ada", 10).await.unwrap();
        assert!(!r.is_empty());
        assert_eq!(r[0].email, "ada@x.com");
    }

    /// P5.5: a display name containing a comma is found by any part of it, and
    /// a LIKE wildcard in the query is a literal, not a match-everything.
    #[tokio::test]
    async fn p55_comma_names_match_and_wildcards_are_literal() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        db.contacts_upsert("a", "jane@example.com", Some("Doe, Jane"))
            .await
            .unwrap();
        db.contacts_upsert("a", "percent@example.com", Some("100% Real"))
            .await
            .unwrap();

        let by_name = db.contacts_suggest("a", "Doe, Jane", 10).await.unwrap();
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].email, "jane@example.com");
        assert_eq!(by_name[0].name.as_deref(), Some("Doe, Jane"));
        // Any fragment of the comma-containing name works, including the comma.
        assert_eq!(
            db.contacts_suggest("a", "Jane", 10).await.unwrap()[0].email,
            "jane@example.com"
        );
        assert_eq!(
            db.contacts_suggest("a", "Doe,", 10).await.unwrap()[0].email,
            "jane@example.com"
        );
        // A `%` in the query is not a wildcard.
        assert!(db.contacts_suggest("a", "%%%%", 10).await.unwrap().is_empty());
        let r = db.contacts_suggest("a", "100%", 10).await.unwrap();
        assert_eq!(r.len(), 1, "the literal percent still matches");
        assert_eq!(r[0].email, "percent@example.com");
    }

    /// P5.5: suggestions never cross accounts, even for an identical name, and
    /// terms shorter than the trigram minimum still match.
    #[tokio::test]
    async fn p55_suggestions_are_account_scoped_and_short_terms_match() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        db.contacts_upsert("a", "shared@x.com", Some("Shared Name"))
            .await
            .unwrap();
        db.contacts_upsert("b", "shared@x.com", Some("Shared Name"))
            .await
            .unwrap();
        db.contacts_upsert("b", "other@y.com", Some("Other Person"))
            .await
            .unwrap();

        let only_a = db.contacts_suggest("a", "Shared", 10).await.unwrap();
        assert_eq!(only_a.len(), 1);
        assert_eq!(only_a[0].email, "shared@x.com");
        assert!(
            !only_a.iter().any(|c| c.email == "other@y.com"),
            "another account's contact must never be suggested"
        );
        // Two characters is below the trigram minimum: the LIKE fallback runs.
        let short = db.contacts_suggest("a", "sh", 10).await.unwrap();
        assert_eq!(short.len(), 1);
        assert_eq!(short[0].email, "shared@x.com");
        assert!(db.contacts_suggest("a", "ot", 10).await.unwrap().is_empty());
    }

    /// P5.5: renaming a contact reindexes it, so the old name stops matching
    /// and the new one starts.
    #[tokio::test]
    async fn p55_rename_reindexes_the_trigram_index() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        db.contacts_upsert("a", "person@x.com", Some("Old Name"))
            .await
            .unwrap();
        assert_eq!(
            db.contacts_suggest("a", "Old Name", 10).await.unwrap().len(),
            1
        );
        db.contacts_upsert("a", "person@x.com", Some("New Name"))
            .await
            .unwrap();
        assert!(
            db.contacts_suggest("a", "Old Name", 10).await.unwrap().is_empty(),
            "the stale trigram row must be gone"
        );
        let renamed = db.contacts_suggest("a", "New Name", 10).await.unwrap();
        assert_eq!(renamed.len(), 1);
        assert_eq!(renamed[0].name.as_deref(), Some("New Name"));
    }
}
