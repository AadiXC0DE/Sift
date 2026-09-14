//! Local search execution (P7.1/P7.3).
//!
//! One compiled candidate query groups qualifying messages into unique
//! `(account_id, thread_id)` pairs, applies the keyset cursor and the LIMIT;
//! one hydration statement then turns at most `limit` pairs into rows. There
//! is no per-hit `message_thread`/`thread_row_for` round trip (which used to
//! cost one query per message hit) and no in-memory mailbox-sized result set.

use super::compile;
use super::cursor::{self, Cursor, SortKind};
use super::query::Parsed;
use crate::db::Db;
use crate::db::threads::thread_row_from;
use crate::dto::ThreadRow;
use anyhow::{Context, Result};
use rusqlite::types::Value;

/// Hard cap on one page of search results.
pub const MAX_SEARCH_LIMIT: i64 = 100;

#[derive(Debug, Default)]
pub struct SearchPage {
    pub rows: Vec<ThreadRow>,
    pub next_cursor: Option<String>,
}

/// Run one page of a local search.
pub async fn search_page(
    db: &Db,
    account_ids: &[String],
    parsed: &Parsed,
    cursor: Option<&Cursor>,
    scope: &str,
    limit: i64,
) -> Result<SearchPage> {
    if account_ids.is_empty() {
        return Ok(SearchPage::default());
    }
    let limit = limit.clamp(1, MAX_SEARCH_LIMIT);
    let node = parsed.ast.as_ref();
    let compiled = compile::candidate_query(
        node,
        account_ids,
        compile::default_mailbox_scope(node),
        cursor.map(|c| &c.key),
        // One row past the page: its existence is what makes nextCursor real.
        limit + 1,
    )?;
    let query_fingerprint = parsed.fingerprint();
    let scope = scope.to_string();
    db.read(move |conn| {
        let mut statement = conn
            .prepare(&compiled.sql)
            .context("prepare search candidate query")?;
        let params: Vec<Value> = compiled
            .params
            .iter()
            .map(|p| match p {
                compile::Param::Text(value) => Value::Text(value.clone()),
                compile::Param::Int(value) => Value::Integer(*value),
            })
            .collect();
        let mut pairs: Vec<(String, String, i64)> = statement
            .query_map(rusqlite::params_from_iter(params.iter()), |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        let has_more = pairs.len() as i64 > limit;
        if has_more {
            pairs.pop();
        }
        let next_cursor = if has_more {
            pairs.last().map(|(account, thread, date)| {
                cursor::encode(&Cursor {
                    sort: SortKind::Search,
                    scope: scope.clone(),
                    query: query_fingerprint.clone(),
                    key: (*date, account.clone(), thread.clone()),
                })
            })
        } else {
            None
        };
        let rows = hydrate(conn, &pairs)?;
        Ok(SearchPage { rows, next_cursor })
    })
    .await
}

/// Count the conversations one query matches. Used by saved-search counts so
/// a visible mailbox never materialises its rows just to show a number.
pub async fn search_count(db: &Db, account_ids: &[String], parsed: &Parsed) -> Result<i64> {
    if account_ids.is_empty() {
        return Ok(0);
    }
    let node = parsed.ast.as_ref();
    let compiled = compile::count_query(node, account_ids, compile::default_mailbox_scope(node))?;
    db.read(move |conn| {
        let mut statement = conn
            .prepare(&compiled.sql)
            .context("prepare search count query")?;
        let params: Vec<Value> = compiled
            .params
            .iter()
            .map(|p| match p {
                compile::Param::Text(value) => Value::Text(value.clone()),
                compile::Param::Int(value) => Value::Integer(*value),
            })
            .collect();
        Ok(statement.query_row(rusqlite::params_from_iter(params.iter()), |row| row.get(0))?)
    })
    .await
}

/// Fetch up to 100 thread rows in one statement, in the order the candidate
/// query produced them.
///
/// `(account_id, id) IN (VALUES …)` uses the composite primary key rather than
/// a text `OR` chain, and the requested order is restored explicitly because
/// SQLite is free to return the rows in any order.
pub(crate) fn hydrate(
    conn: &rusqlite::Connection,
    pairs: &[(String, String, i64)],
) -> Result<Vec<ThreadRow>> {
    if pairs.is_empty() {
        return Ok(Vec::new());
    }
    let mut sql = String::from(
        "SELECT t.*, (SELECT r.remind_at FROM reminders r WHERE r.account_id=t.account_id \
         AND r.thread_id=t.id AND r.completed_at IS NULL) AS reminder_at \
         FROM threads t WHERE (account_id, id) IN (VALUES ",
    );
    for index in 0..pairs.len() {
        if index > 0 {
            sql.push(',');
        }
        sql.push_str("(?,?)");
    }
    sql.push(')');
    let mut statement = conn.prepare(&sql).context("prepare thread hydration")?;
    let mut params: Vec<Value> = Vec::with_capacity(pairs.len() * 2);
    for (account, thread, _) in pairs {
        params.push(Value::Text(account.clone()));
        params.push(Value::Text(thread.clone()));
    }
    let mut found: std::collections::HashMap<(String, String), ThreadRow> =
        std::collections::HashMap::with_capacity(pairs.len());
    let rows = statement
        .query_map(rusqlite::params_from_iter(params.iter()), thread_row_from)?
        .collect::<Result<Vec<_>, _>>()?;
    for row in rows {
        found.insert((row.account_id.clone(), row.id.clone()), row);
    }
    Ok(pairs
        .iter()
        .filter_map(|(account, thread, _)| found.remove(&(account.clone(), thread.clone())))
        .collect())
}
