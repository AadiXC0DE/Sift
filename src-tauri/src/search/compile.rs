//! AST -> SQL (P7.1).
//!
//! Every predicate is compiled against the **same message row** `m`, so
//! `from:A has:attachment` can never be satisfied by one message of a thread
//! matching `from:A` and a different message carrying the attachment. The
//! caller wraps this fragment in a grouped candidate query that reduces the
//! matching messages to unique `(account_id, thread_id)` pairs ordered by the
//! newest matching message, so the LIMIT is applied after every filter
//! (account, mailbox, read state, labels, dates) instead of to a cut-down hit
//! list.

use super::query::{names_mailbox, Mailbox, Node, Pred};
use anyhow::Result;

/// One bound SQL parameter. Values are never interpolated into the SQL text.
#[derive(Debug, Clone, PartialEq)]
pub enum Param {
    Text(String),
    Int(i64),
}

#[derive(Debug, Clone, Default)]
pub struct Compiled {
    pub sql: String,
    pub params: Vec<Param>,
}

/// Column filter used for free text and phrases. `from_text` holds
/// `name email`, so a sender search covers both.
const ALL_COLUMNS: &str = "{subject from_text to_text body}";

struct Builder {
    sql: String,
    params: Vec<Param>,
}

impl Builder {
    fn new() -> Self {
        Self {
            sql: String::with_capacity(256),
            params: Vec::new(),
        }
    }

    fn raw(&mut self, sql: &str) {
        self.sql.push_str(sql);
    }

    fn text(&mut self, value: impl Into<String>) {
        self.sql.push('?');
        self.params.push(Param::Text(value.into()));
    }

    fn int(&mut self, value: i64) {
        self.sql.push('?');
        self.params.push(Param::Int(value));
    }
}

/// Escape a value for an FTS5 string literal: the only escape inside a quoted
/// FTS string is a doubled double-quote. NUL bytes are dropped (they cannot
/// appear in a token and would truncate the C string in SQLite).
pub fn fts_quote(value: &str) -> String {
    let cleaned: String = value.chars().filter(|c| *c != '\0').collect();
    format!("\"{}\"", cleaned.replace('"', "\"\""))
}

/// FTS5 tokenizes punctuation away, so a token or phrase without a single
/// alphanumeric character can never match an index entry. Those fall back to a
/// literal substring comparison rather than producing a query FTS5 rejects.
fn fts_matchable(value: &str) -> bool {
    value.chars().any(char::is_alphanumeric)
}

/// A token that is a single FTS word may be prefix-matched, so a half-typed
/// word still finds the message. Anything else — `C++`, `100%`, `a@b.com` —
/// must be matched as an exact phrase: prefixing `C++` would become `c*` and
/// match every address ending in `.com`.
fn word_like(value: &str) -> bool {
    !value.is_empty() && value.chars().all(char::is_alphanumeric)
}

/// `EXISTS` fragment for one FTS expression, correlated to the message row.
fn fts_exists(b: &mut Builder, expression: String) {
    b.raw(
        "EXISTS (SELECT 1 FROM messages_fts f WHERE f.account_id=m.account_id AND f.message_id=m.id \
         AND f.rowid IN (SELECT rowid FROM messages_fts WHERE messages_fts MATCH ",
    );
    b.text(expression);
    b.raw("))");
}

/// Literal substring comparison over the message columns that can be reached
/// without the full-text index.
fn substring_any(b: &mut Builder, value: &str, include_to: bool) {
    b.raw("(instr(lower(m.subject), lower(");
    b.text(value.to_string());
    b.raw("))>0 OR instr(lower(COALESCE(m.from_name,'') || ' ' || COALESCE(m.from_email,'')), lower(");
    b.text(value.to_string());
    b.raw("))>0");
    if include_to {
        b.raw(" OR instr(lower(m.to_json), lower(");
        b.text(value.to_string());
        b.raw("))>0");
    }
    b.raw(")");
}

fn compile_term(b: &mut Builder, token: &str) {
    b.raw("(");
    let mut wrote = false;
    if fts_matchable(token) {
        // Prefix match so a partially typed word still finds the message. The
        // `*` belongs inside the group, directly after the quoted string.
        let expression = if word_like(token) {
            format!("{ALL_COLUMNS} : ({}*)", fts_quote(token))
        } else {
            format!("{ALL_COLUMNS} : ({})", fts_quote(token))
        };
        fts_exists(b, expression);
        wrote = true;
    }
    if wrote {
        b.raw(" OR ");
    }
    substring_any(b, token, true);
    b.raw(")");
}

fn compile_phrase(b: &mut Builder, phrase: &str) {
    b.raw("(");
    let mut wrote = false;
    if fts_matchable(phrase) {
        fts_exists(b, format!("{ALL_COLUMNS} : ({})", fts_quote(phrase)));
        wrote = true;
    }
    if wrote {
        b.raw(" OR ");
    }
    substring_any(b, phrase, true);
    b.raw(")");
}

fn compile_pred(b: &mut Builder, pred: &Pred) {
    match pred {
        Pred::From(value) => {
            b.raw("instr(lower(COALESCE(m.from_name,'') || ' ' || COALESCE(m.from_email,'')), lower(");
            b.text(value.clone());
            b.raw("))>0");
        }
        Pred::To(value) => {
            b.raw("instr(lower(m.to_json), lower(");
            b.text(value.clone());
            b.raw("))>0");
        }
        Pred::Cc(value) => {
            b.raw("instr(lower(m.cc_json), lower(");
            b.text(value.clone());
            b.raw("))>0");
        }
        Pred::Subject(value) => {
            b.raw("instr(lower(m.subject), lower(");
            b.text(value.clone());
            b.raw("))>0");
        }
        Pred::Label(value) => {
            // Resolved against the account's own label table, so a user-visible
            // name (including a quoted hierarchical one) matches even when the
            // provider id differs. The id also works, for callers that already
            // hold one.
            b.raw(
                "EXISTS (SELECT 1 FROM message_labels ml JOIN labels l \
                 ON l.account_id=ml.account_id AND l.id=ml.label_id \
                 WHERE ml.account_id=m.account_id AND ml.message_id=m.id \
                 AND (l.name COLLATE NOCASE = ",
            );
            b.text(value.clone());
            b.raw(" OR l.id COLLATE NOCASE = ");
            b.text(value.clone());
            b.raw("))");
        }
        Pred::In(mailbox) => compile_mailbox(b, *mailbox),
        Pred::HasAttachment => b.raw("m.has_attachments=1"),
        Pred::IsUnread(unread) => {
            b.raw(if *unread {
                "m.is_unread=1"
            } else {
                "m.is_unread=0"
            });
        }
        Pred::IsStarred(on) => {
            b.raw(if *on {
                "m.is_starred=1"
            } else {
                "m.is_starred=0"
            });
        }
        Pred::Before(ts) => {
            b.raw("m.internal_date < ");
            b.int(*ts);
        }
        Pred::After(ts) => {
            b.raw("m.internal_date >= ");
            b.int(*ts);
        }
    }
}

fn label_exists(b: &mut Builder, labels: &[&str]) {
    b.raw(
        "EXISTS (SELECT 1 FROM message_labels ml WHERE ml.account_id=m.account_id \
         AND ml.message_id=m.id AND ml.label_id IN (",
    );
    for (index, label) in labels.iter().enumerate() {
        if index > 0 {
            b.raw(",");
        }
        b.text((*label).to_string());
    }
    b.raw("))");
}

fn compile_mailbox(b: &mut Builder, mailbox: Mailbox) {
    match mailbox {
        Mailbox::Anywhere => b.raw("1=1"),
        Mailbox::Inbox => label_exists(b, &["INBOX"]),
        Mailbox::Sent => label_exists(b, &["SENT"]),
        Mailbox::Drafts => label_exists(b, &["DRAFT"]),
        Mailbox::Trash => label_exists(b, &["TRASH"]),
        Mailbox::Spam => label_exists(b, &["SPAM"]),
        Mailbox::Archive => {
            b.raw("NOT ");
            label_exists(b, &["INBOX"]);
            b.raw(" AND NOT ");
            label_exists(b, &["TRASH", "SPAM"]);
            b.raw(" AND NOT ");
            label_exists(b, &["DRAFT"]);
        }
    }
}

fn compile_node(b: &mut Builder, node: &Node) {
    match node {
        Node::And(items) => {
            if items.is_empty() {
                b.raw("1=1");
                return;
            }
            b.raw("(");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    b.raw(" AND ");
                }
                compile_node(b, item);
            }
            b.raw(")");
        }
        Node::Or(items) => {
            if items.is_empty() {
                b.raw("0");
                return;
            }
            b.raw("(");
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    b.raw(" OR ");
                }
                compile_node(b, item);
            }
            b.raw(")");
        }
        Node::Not(inner) => {
            b.raw("NOT (");
            compile_node(b, inner);
            b.raw(")");
        }
        Node::Term(token) => compile_term(b, token),
        Node::Phrase(phrase) => compile_phrase(b, phrase),
        Node::Never => b.raw("0"),
        Node::Pred(pred) => compile_pred(b, pred),
    }
}

/// Compile an AST into a SQL boolean expression over the alias `m`.
pub fn compile(node: &Node) -> Compiled {
    let mut builder = Builder::new();
    compile_node(&mut builder, node);
    Compiled {
        sql: builder.sql,
        params: builder.params,
    }
}

/// The grouped candidate CTE: unique `(account_id, thread_id)` pairs with the
/// date of their newest matching message, after every filter.
fn matched_cte(
    b: &mut Builder,
    node: Option<&Node>,
    account_ids: &[String],
    exclude_trash_and_junk: bool,
) {
    b.raw(
        "WITH matched AS (SELECT m.account_id AS account_id, m.thread_id AS thread_id, \
         MAX(m.internal_date) AS matched_date FROM messages m WHERE m.account_id IN (",
    );
    for (index, account) in account_ids.iter().enumerate() {
        if index > 0 {
            b.raw(",");
        }
        b.text(account.clone());
    }
    b.raw(")");
    if let Some(node) = node {
        b.raw(" AND ");
        compile_node(b, node);
    }
    if exclude_trash_and_junk {
        b.raw(" AND NOT ");
        label_exists(b, &["TRASH", "SPAM"]);
    }
    b.raw(" GROUP BY m.account_id, m.thread_id)");
}

/// `SELECT COUNT(*)` over the same candidate set, for a lazy saved-search
/// count. It never returns the rows themselves.
pub fn count_query(
    node: Option<&Node>,
    account_ids: &[String],
    exclude_trash_and_junk: bool,
) -> Result<Compiled> {
    if account_ids.is_empty() {
        anyhow::bail!("a count needs at least one account");
    }
    let mut b = Builder::new();
    matched_cte(&mut b, node, account_ids, exclude_trash_and_junk);
    b.raw(" SELECT COUNT(*) FROM matched");
    Ok(Compiled {
        sql: b.sql,
        params: b.params,
    })
}

/// The bounded, grouped candidate query for one search page.
///
/// * every filter (accounts, mailbox, read state, labels, dates) is applied to
///   `messages` **before** LIMIT;
/// * duplicates collapse to unique `(account_id, thread_id)` pairs in SQL,
///   ordered by the newest matching message;
/// * the keyset predicate uses the same tuple, in the same direction, as the
///   ORDER BY, so a page boundary can never skip or repeat a thread;
/// * there is no OFFSET.
pub fn candidate_query(
    node: Option<&Node>,
    account_ids: &[String],
    exclude_trash_and_junk: bool,
    cursor: Option<&(i64, String, String)>,
    limit: i64,
) -> Result<Compiled> {
    if account_ids.is_empty() {
        anyhow::bail!("search needs at least one account");
    }
    let mut b = Builder::new();
    matched_cte(&mut b, node, account_ids, exclude_trash_and_junk);
    b.raw(" SELECT account_id, thread_id, matched_date FROM matched");
    if let Some((date, account, thread)) = cursor {
        b.raw(" WHERE (matched_date, account_id, thread_id) < (");
        b.int(*date);
        b.raw(",");
        b.text(account.clone());
        b.raw(",");
        b.text(thread.clone());
        b.raw(")");
    }
    b.raw(" ORDER BY matched_date DESC, account_id DESC, thread_id DESC LIMIT ");
    b.int(limit);
    Ok(Compiled {
        sql: b.sql,
        params: b.params,
    })
}

/// True when the query should keep the default "everything except Trash and
/// Junk" scope: an explicit `in:` replaces it.
pub fn default_mailbox_scope(node: Option<&Node>) -> bool {
    match node {
        Some(node) => !names_mailbox(node),
        None => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::query::parse;

    fn sql_of(q: &str) -> Compiled {
        let parsed = parse(q);
        compile(parsed.ast.as_ref().unwrap())
    }

    #[test]
    fn p7_t11_punctuation_tokens_skip_fts() {
        let compiled = sql_of("--");
        assert!(!compiled.sql.contains("MATCH"), "{}", compiled.sql);
        assert!(compiled.sql.contains("instr("));
    }

    #[test]
    fn p7_t12_every_predicate_is_bound() {
        let compiled = sql_of("from:a to:b cc:c subject:d label:e in:trash has:attachment is:unread before:2026-01-01");
        // No literal user text can appear in the SQL string.
        for needle in ["'a'", "'b'", "'c'", "'d'", "'e'", "2026-01-01"] {
            assert!(!compiled.sql.contains(needle), "{} in {}", needle, compiled.sql);
        }
        assert_eq!(compiled.sql.matches('?').count(), compiled.params.len());
    }

    #[test]
    fn p7_t13_fts_operators_and_metacharacters_are_quoted() {
        // `AND` is a word here, not the FTS operator: every text leaf is a
        // fully quoted FTS string, so nothing the user types can change the
        // shape of the expression.
        let compiled = sql_of(r#"hello AND world (c)"#);
        let expressions: Vec<&str> = compiled
            .params
            .iter()
            .filter_map(|p| match p {
                Param::Text(value) if value.starts_with('{') => Some(value.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(expressions.len(), 4, "{expressions:?}");
        for expected in [
            r#"{subject from_text to_text body} : ("AND"*)"#,
            r#"{subject from_text to_text body} : ("hello"*)"#,
            r#"{subject from_text to_text body} : ("world"*)"#,
            r#"{subject from_text to_text body} : ("c"*)"#,
        ] {
            assert!(expressions.contains(&expected), "{expected} missing");
        }
        assert!(!compiled.sql.contains("MATCH AND"), "{}", compiled.sql);
    }

    #[test]
    fn p7_t13b_punctuation_terms_are_never_prefix_matched() {
        // `C++` must not become the prefix `c*`: every address ending in
        // `.com` would match. It is an exact phrase plus a literal substring.
        let compiled = sql_of("C++");
        let Param::Text(expression) = compiled
            .params
            .iter()
            .find(|p| matches!(p, Param::Text(v) if v.starts_with('{')))
            .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(expression, r#"{subject from_text to_text body} : ("C++")"#);
        assert!(!expression.contains('*'));
        assert!(compiled.sql.contains("instr("));
    }

    #[test]
    fn p7_t14_candidate_query_puts_one_message_under_all_predicates() {
        let parsed = parse("from:a has:attachment");
        let accounts = vec!["a1".to_string()];
        let compiled = candidate_query(
            parsed.ast.as_ref(),
            &accounts,
            default_mailbox_scope(parsed.ast.as_ref()),
            None,
            51,
        )
        .unwrap();
        assert!(compiled.sql.contains("GROUP BY m.account_id, m.thread_id"));
        assert!(!compiled.sql.contains("HAVING"));
        assert!(compiled.sql.contains("m.has_attachments=1"));
        assert!(compiled.sql.contains("instr(lower(COALESCE(m.from_name"));
        assert!(!compiled.sql.contains("OFFSET"));
        // Both predicates live in the same WHERE over one alias.
        let where_at = compiled.sql.find("WHERE m.account_id IN").unwrap();
        let group_at = compiled.sql.find("GROUP BY").unwrap();
        assert!(compiled.sql[where_at..group_at].contains("has_attachments=1"));
        assert!(compiled.sql[where_at..group_at].contains("instr(lower(COALESCE(m.from_name"));
    }

    #[test]
    fn p7_t15_explicit_mailbox_replaces_the_default() {
        assert!(!default_mailbox_scope(parse("in:trash").ast.as_ref()));
        assert!(default_mailbox_scope(parse("from:a").ast.as_ref()));
        let parsed = parse("in:inbox");
        let compiled = candidate_query(
            parsed.ast.as_ref(),
            &["a1".to_string()],
            default_mailbox_scope(parsed.ast.as_ref()),
            None,
            10,
        )
        .unwrap();
        assert!(!compiled.sql.contains("'TRASH','SPAM'"));
    }
}
