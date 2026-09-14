//! Versioned opaque cursors (P7.3).
//!
//! A cursor carries the **complete** sort tuple and a fingerprint of the scope
//! and query that produced it, so a page boundary can never skip or repeat a
//! row and a cursor cannot be replayed against a different list or search.
//! The payload is base64url JSON: opaque to the UI, versioned so a future
//! change can reject old cursors explicitly instead of misreading them.
//!
//! Each view compares the *same* tuple in WHERE as it orders by:
//!
//! | view    | tuple                                                        |
//! |---------|--------------------------------------------------------------|
//! | default | `(last_message_at DESC, account_id DESC, id DESC)`            |
//! | snoozed | `(snoozed_until ASC, account_id ASC, id ASC)`                 |
//! | search  | `(matched_message_date DESC, account_id DESC, thread_id DESC)` |

use crate::errors::SiftError;
use base64::Engine;
use serde::{Deserialize, Serialize};

/// Cursors written by an older build are rejected rather than guessed at.
pub const CURSOR_VERSION: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortKind {
    Default,
    Snoozed,
    Search,
}

impl SortKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Snoozed => "snoozed",
            Self::Search => "search",
        }
    }
}

/// The decoded keyset. All three tuple shapes are `(i64, text, text)`; the
/// field names differ per view only in the SQL that compares them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cursor {
    pub sort: SortKind,
    pub scope: String,
    pub query: String,
    pub key: (i64, String, String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Wire {
    v: u8,
    #[serde(rename = "s")]
    sort: String,
    /// Scope fingerprint: the list the cursor was issued for.
    #[serde(rename = "c")]
    scope: String,
    /// Query fingerprint: empty for a mailbox view.
    #[serde(rename = "q")]
    query: String,
    #[serde(rename = "t")]
    tuple: Vec<serde_json::Value>,
}

fn sort_from_str(value: &str) -> Option<SortKind> {
    match value {
        "default" => Some(SortKind::Default),
        "snoozed" => Some(SortKind::Snoozed),
        "search" => Some(SortKind::Search),
        _ => None,
    }
}

pub fn encode(cursor: &Cursor) -> String {
    let wire = Wire {
        v: CURSOR_VERSION,
        sort: cursor.sort.as_str().to_string(),
        scope: cursor.scope.clone(),
        query: cursor.query.clone(),
        tuple: vec![
            serde_json::Value::from(cursor.key.0),
            serde_json::Value::from(cursor.key.1.clone()),
            serde_json::Value::from(cursor.key.2.clone()),
        ],
    };
    let json = serde_json::to_vec(&wire).unwrap_or_default();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
}

fn bad(message: &str, reason: &str) -> SiftError {
    SiftError::typed(
        "bad_cursor",
        message,
        serde_json::json!({ "reason": reason }),
    )
}

/// Decode a cursor and refuse one that does not belong to this exact list,
/// query and sort order.
pub fn expect(
    raw: &str,
    sort: SortKind,
    scope: &str,
    query: &str,
) -> Result<Cursor, SiftError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(raw.trim())
        .map_err(|_| {
            bad(
                "This page link is not one Sift issued. Start from the first page.",
                "malformed",
            )
        })?;
    let wire: Wire = serde_json::from_slice(&bytes).map_err(|_| {
        bad(
            "This page link is not one Sift issued. Start from the first page.",
            "malformed",
        )
    })?;
    if wire.v != CURSOR_VERSION {
        return Err(bad(
            "This page link was made by a different version of Sift. Start from the first page.",
            "version",
        ));
    }
    let found = sort_from_str(&wire.sort).ok_or_else(|| {
        bad(
            "This page link is not one Sift issued. Start from the first page.",
            "malformed",
        )
    })?;
    if found != sort {
        return Err(bad(
            "This page link belongs to a different list. Start from the first page.",
            "sort",
        ));
    }
    if wire.scope != scope {
        return Err(bad(
            "This page link belongs to a different mailbox or account selection. Start from the first page.",
            "scope",
        ));
    }
    if wire.query != query {
        return Err(bad(
            "This page link belongs to a different search. Start from the first page.",
            "query",
        ));
    }
    if wire.tuple.len() != 3 {
        return Err(bad(
            "This page link is not one Sift issued. Start from the first page.",
            "shape",
        ));
    }
    let date = wire.tuple[0].as_i64().ok_or_else(|| {
        bad(
            "This page link is not one Sift issued. Start from the first page.",
            "shape",
        )
    })?;
    let account = wire.tuple[1].as_str().ok_or_else(|| {
        bad(
            "This page link is not one Sift issued. Start from the first page.",
            "shape",
        )
    })?;
    let id = wire.tuple[2].as_str().ok_or_else(|| {
        bad(
            "This page link is not one Sift issued. Start from the first page.",
            "shape",
        )
    })?;
    Ok(Cursor {
        sort,
        scope: wire.scope,
        query: wire.query,
        key: (date, account.to_string(), id.to_string()),
    })
}

/// Scope fingerprint. The account *set* is the scope, so ordering does not
/// matter; a different set (or a different view) is a different list.
pub fn scope_key(view: &str, account_ids: &[String]) -> String {
    let mut accounts: Vec<&str> = account_ids.iter().map(String::as_str).collect();
    accounts.sort_unstable();
    accounts.dedup();
    format!("{view}|{}", accounts.join(","))
}
