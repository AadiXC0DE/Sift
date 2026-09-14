//! Saved searches — local Smart Mailboxes (P7.4).
//!
//! A saved search stores a **validated query** and an account scope. It never
//! copies, moves or deletes mail, and it never persists a mailbox-sized result
//! set: opening one runs at most one indexed query, and the count is computed
//! lazily when the mailbox is visible.

use super::Db;
use crate::dto::{SavedSearch, SavedSearchCount, SavedSearchInput};
use crate::search::query::{self, AST_VERSION};
use anyhow::{Context, Result};
use rusqlite::params;
use uuid::Uuid;

/// The AST version the stored queries were validated against.
const AST_VERSION_KEY: &str = "saved_search_ast_version";

fn parse_scope(raw: &str) -> Vec<String> {
    serde_json::from_str(raw).unwrap_or_default()
}

/// Validate a query against the P7 compiler. A query that can never match
/// (an invalid date, an empty group, over-nested parentheses) is refused
/// rather than saved as a trap that silently returns nothing.
pub fn validate_query(text: &str) -> Result<(), String> {
    let parsed = query::parse(text);
    if parsed.ast.is_none() {
        return Err("A saved search needs a query.".into());
    }
    if let Some(hint) = parsed.hints.iter().find(|h| query::is_fatal_hint(&h.kind)) {
        return Err(hint.message.clone());
    }
    if contains_never(parsed.ast.as_ref().unwrap()) {
        return Err("This query cannot match anything; check its dates and parentheses.".into());
    }
    Ok(())
}

fn contains_never(node: &query::Node) -> bool {
    match node {
        query::Node::Never => true,
        query::Node::And(items) | query::Node::Or(items) => items.iter().any(contains_never),
        query::Node::Not(inner) => contains_never(inner),
        query::Node::Term(_) | query::Node::Phrase(_) | query::Node::Pred(_) => false,
    }
}

impl Db {
    /// Create or update one saved search. Validating here means every stored
    /// query compiles with the same compiler the ad hoc search uses, so the
    /// saved result and the typed result are always identical.
    pub async fn saved_search_upsert(&self, input: SavedSearchInput) -> Result<SavedSearch> {
        let name = input.name.trim().to_string();
        if name.is_empty() {
            anyhow::bail!("a saved search needs a name");
        }
        validate_query(&input.query).map_err(|e| anyhow::anyhow!(e))?;
        let mut scope: Vec<String> = input.account_scope.clone();
        scope.sort_unstable();
        scope.dedup();
        let scope_json = serde_json::to_string(&scope)?;
        let id = input
            .id
            .clone()
            .unwrap_or_else(|| Uuid::now_v7().to_string());
        let created_at = super::now_ms();
        let sort_order = input.sort_order.unwrap_or(0);
        let (id_w, name_w, query_w) = (id.clone(), name.clone(), input.query.clone());
        self.write(move |c| {
            let next_order: i64 = if sort_order > 0 {
                sort_order
            } else {
                c.query_row(
                    "SELECT COALESCE(MAX(sort_order),0)+1 FROM saved_searches",
                    [],
                    |r| r.get(0),
                )?
            };
            c.execute(
                "INSERT INTO saved_searches (id,name,query,account_scope_json,sort_order,created_at)
                 VALUES (?,?,?,?,?,?)
                 ON CONFLICT(id) DO UPDATE SET name=excluded.name, query=excluded.query,
                   account_scope_json=excluded.account_scope_json, sort_order=excluded.sort_order",
                params![id_w, name_w, query_w, scope_json, next_order, created_at],
            )?;
            Ok(())
        })
        .await?;
        self.setting_set_raw(AST_VERSION_KEY, &AST_VERSION.to_string())
            .await?;
        self.saved_search_get(&id)
            .await?
            .context("saved search disappeared after upsert")
    }

    /// Delete a saved search. It removes the saved query only: no message,
    /// thread, label or operation is touched.
    pub async fn saved_search_delete(&self, id: &str) -> Result<()> {
        let id = id.to_string();
        self.write(move |c| {
            c.execute("DELETE FROM saved_searches WHERE id=?", params![id])?;
            Ok(())
        })
        .await
    }

    pub async fn saved_search_get(&self, id: &str) -> Result<Option<SavedSearch>> {
        let rows = self.saved_search_list().await?;
        Ok(rows.into_iter().find(|s| s.id == id))
    }

    pub async fn saved_search_list(&self) -> Result<Vec<SavedSearch>> {
        let stored_ast_version = self
            .setting_get_raw(AST_VERSION_KEY)
            .await?
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(AST_VERSION);
        let accounts: Vec<String> = self
            .read(|c| {
                let mut statement = c.prepare("SELECT id FROM accounts")?;
                let rows: Vec<String> = statement
                    .query_map([], |r| r.get(0))?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        let rows = self
            .read(|c| {
                let mut statement = c.prepare(
                    "SELECT id,name,query,account_scope_json,sort_order,created_at
                     FROM saved_searches ORDER BY sort_order, created_at, id",
                )?;
                #[allow(clippy::type_complexity)]
                let rows: Vec<(String, String, String, String, i64, i64)> = statement
                    .query_map([], |r| {
                        Ok((
                            r.get(0)?,
                            r.get(1)?,
                            r.get(2)?,
                            r.get(3)?,
                            r.get(4)?,
                            r.get(5)?,
                        ))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            })
            .await?;
        Ok(rows
            .into_iter()
            .map(|(id, name, query, scope, sort_order, created_at)| {
                let account_scope = parse_scope(&scope);
                let missing_accounts = account_scope
                    .iter()
                    .filter(|account| !accounts.contains(account))
                    .cloned()
                    .collect();
                SavedSearch {
                    id,
                    name,
                    query,
                    account_scope,
                    missing_accounts,
                    sort_order,
                    created_at,
                    ast_version: stored_ast_version,
                    match_count: None,
                    counted_at: None,
                }
            })
            .collect())
    }

    /// Count the conversations a saved search matches, on demand. The count is
    /// computed when a mailbox is visible, never on a tick for every saved
    /// search, and it never materialises the matching rows.
    pub async fn saved_search_count(&self, id: &str) -> Result<SavedSearchCount> {
        let saved = self
            .saved_search_get(id)
            .await?
            .context("unknown saved search")?;
        let parsed = query::parse(&saved.query);
        let count = crate::search::local::search_count(self, &saved.account_scope, &parsed).await?;
        Ok(SavedSearchCount {
            id: saved.id,
            count,
            computed_at: super::now_ms(),
        })
    }
}
