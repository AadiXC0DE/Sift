//! Query compiler front end (P7.1).
//!
//! The old parser flattened a query into lossy strings (`terms`, `from`,
//! `label`, …), which is why `in:` was never applied, `is:read` parsed to a
//! value the SQL could not use, labels were compared to provider ids, quoted
//! phrases lost their boundaries and an invalid date silently disappeared
//! (widening the query to the whole mailbox). This module replaces it with a
//! small AST that the SQL stage can actually compile:
//!
//! * `And`, `Or`, `Not`, `Term`, `Phrase` and typed predicates.
//! * `from`/`to`/`cc`/`subject`/`label`/`in`/`has:attachment`/`is:read` |
//!   `unread` | `starred`/`before`/`after`, exact quoted phrases and unary
//!   minus.
//! * Unknown operators survive as **literal text** (never dropped) and are
//!   reported as a hint so the UI can offer "Search Gmail for full syntax".
//! * An invalid date or an unclosed quote produces a recoverable hint. An
//!   invalid date compiles to [`Node::Never`]: it can never broaden a query.
//!
//! Dates are **local calendar-day boundaries**. `after:2026-03-01` includes
//! 00:00 local on 2026-03-01; `before:2026-03-01` excludes it. The help copy
//! exposed by [`HELP_TEXT`] states that.

use serde::{Deserialize, Serialize};

/// Bumped whenever the AST or its compilation changes meaning. Persisted with
/// saved searches so a stored query can be re-validated after an upgrade.
pub const AST_VERSION: i64 = 1;

/// Maximum nesting depth of the compiled AST.
pub const MAX_DEPTH: usize = 20;
/// Maximum number of whitespace-separated tokens accepted from one query.
pub const MAX_TOKENS: usize = 100;

/// The operator list shown next to the "Search Gmail for full syntax" link.
pub const HELP_TEXT: &str = "Dates are calendar days in your Mac's time zone: after:2026-03-01 starts at 00:00 that day, before:2026-03-01 stops just before it. Use quotes for an exact phrase and - to exclude.";

#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    And(Vec<Node>),
    Or(Vec<Node>),
    Not(Box<Node>),
    /// Free text: prefix/word match, plus a literal substring fallback.
    Term(String),
    /// An exact quoted phrase.
    Phrase(String),
    /// Matches nothing. Produced only by a predicate Sift refuses to guess at
    /// (an invalid date), so a broken query can never widen.
    Never,
    Pred(Pred),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Pred {
    From(String),
    To(String),
    Cc(String),
    Subject(String),
    Label(String),
    In(Mailbox),
    HasAttachment,
    IsUnread(bool),
    IsStarred(bool),
    /// Exclusive upper bound: `internal_date < ts`.
    Before(i64),
    /// Inclusive lower bound: `internal_date >= ts`.
    After(i64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mailbox {
    Inbox,
    Sent,
    Drafts,
    Archive,
    Trash,
    Spam,
    Anywhere,
}

impl Mailbox {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "inbox" => Some(Self::Inbox),
            "sent" => Some(Self::Sent),
            "drafts" | "draft" => Some(Self::Drafts),
            "archive" => Some(Self::Archive),
            "trash" => Some(Self::Trash),
            "spam" | "junk" => Some(Self::Spam),
            "anywhere" => Some(Self::Anywhere),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Sent => "sent",
            Self::Drafts => "drafts",
            Self::Archive => "archive",
            Self::Trash => "trash",
            Self::Spam => "spam",
            Self::Anywhere => "anywhere",
        }
    }
}

/// A recoverable problem the user can fix. `kind` is stable; `message` is the
/// visible copy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueryHint {
    pub kind: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}

impl QueryHint {
    fn new(kind: &str, message: impl Into<String>, token: Option<String>) -> Self {
        Self {
            kind: kind.into(),
            message: message.into(),
            token,
        }
    }
}

/// True when the hint makes the whole query unmatchable rather than merely
/// imprecise. A saved search refuses to store such a query.
pub fn is_fatal_hint(kind: &str) -> bool {
    matches!(kind, "invalid_date" | "too_deep")
}

#[derive(Debug, Clone, PartialEq)]
pub struct Parsed {
    pub raw: String,
    /// `None` for an empty/blank query.
    pub ast: Option<Node>,
    pub hints: Vec<QueryHint>,
    pub ast_version: i64,
}

impl Parsed {
    pub fn is_fatal(&self) -> bool {
        self.hints.iter().any(|h| is_fatal_hint(&h.kind))
    }

    /// Canonical identity of the query for cursor validation. Two parses of
    /// the same text are equal; a different text is a different query even
    /// when it compiles to the same rows.
    pub fn fingerprint(&self) -> String {
        let normalised = self.raw.split_whitespace().collect::<Vec<_>>().join(" ");
        format!("v{}:{}", self.ast_version, normalised)
    }
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum TokKind {
    Word,
    LParen,
    RParen,
    Or,
}

#[derive(Debug, Clone)]
struct Token {
    text: String,
    quoted: bool,
    negate: bool,
    kind: TokKind,
}

fn tokenize(q: &str) -> (Vec<Token>, Vec<QueryHint>) {
    let chars: Vec<char> = q.chars().collect();
    let mut out: Vec<Token> = Vec::new();
    let mut hints: Vec<QueryHint> = Vec::new();
    let mut i = 0usize;
    let mut negate = false;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '(' {
            out.push(Token {
                text: "(".into(),
                quoted: false,
                negate: false,
                kind: TokKind::LParen,
            });
            i += 1;
            negate = false;
            continue;
        }
        if c == ')' {
            out.push(Token {
                text: ")".into(),
                quoted: false,
                negate: false,
                kind: TokKind::RParen,
            });
            i += 1;
            negate = false;
            continue;
        }
        if c == '-' && !negate && chars.get(i + 1).is_some_and(|n| !n.is_whitespace()) {
            negate = true;
            i += 1;
            continue;
        }
        let mut text = String::new();
        let mut quoted = false;
        while i < chars.len() {
            let c = chars[i];
            if c.is_whitespace() || c == ')' || c == '(' {
                break;
            }
            if c == '"' {
                quoted = true;
                i += 1;
                let mut closed = false;
                while i < chars.len() {
                    if chars[i] == '"' {
                        closed = true;
                        i += 1;
                        break;
                    }
                    text.push(chars[i]);
                    i += 1;
                }
                if !closed {
                    hints.push(QueryHint::new(
                        "unclosed_quote",
                        "There is an unclosed quote; Sift searched everything after it as one phrase.",
                        None,
                    ));
                }
                continue;
            }
            text.push(c);
            i += 1;
        }
        if text.is_empty() {
            negate = false;
            continue;
        }
        let kind = if !quoted && text.eq_ignore_ascii_case("or") {
            TokKind::Or
        } else {
            TokKind::Word
        };
        out.push(Token {
            text,
            quoted,
            negate,
            kind,
        });
        negate = false;
    }
    if out.len() > MAX_TOKENS {
        hints.push(QueryHint::new(
            "too_many_tokens",
            format!("Sift used the first {MAX_TOKENS} terms of this query."),
            None,
        ));
        out.truncate(MAX_TOKENS);
    }
    (out, hints)
}

// ---------------------------------------------------------------------------
// Date parsing (local calendar-day boundaries)
// ---------------------------------------------------------------------------

/// Local midnight for a calendar date, in unix milliseconds.
///
/// A spring-forward DST gap at midnight has no `00:00` instant; the first
/// valid instant of that day is used instead so the boundary still exists.
pub fn local_day_start_ms(y: i32, m: u32, d: u32) -> Option<i64> {
    use chrono::{Duration, Local, LocalResult, NaiveDate, TimeZone};
    let date = NaiveDate::from_ymd_opt(y, m, d)?;
    let naive = date.and_hms_opt(0, 0, 0)?;
    match Local.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Some(dt.timestamp_millis()),
        LocalResult::Ambiguous(first, _) => Some(first.timestamp_millis()),
        LocalResult::None => {
            let mut probe = naive;
            for _ in 0..48 {
                probe += Duration::hours(1);
                if let LocalResult::Single(dt) = Local.from_local_datetime(&probe) {
                    return Some(dt.timestamp_millis());
                }
            }
            None
        }
    }
}

fn parse_day(s: &str) -> Option<i64> {
    let normalised = s.trim().replace('/', "-");
    let parts: Vec<&str> = normalised.split('-').collect();
    if parts.len() != 3 || parts[0].len() != 4 {
        return None;
    }
    let y: i32 = parts[0].parse().ok()?;
    let m: u32 = parts[1].parse().ok()?;
    let d: u32 = parts[2].parse().ok()?;
    local_day_start_ms(y, m, d)
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser {
    tokens: Vec<Token>,
    pos: usize,
    hints: Vec<QueryHint>,
    depth_exceeded: bool,
}

impl Parser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.pos)
    }

    fn parse_or(&mut self, depth: usize) -> Node {
        let mut left = self.parse_and(depth);
        while matches!(self.peek().map(|t| &t.kind), Some(TokKind::Or)) {
            self.pos += 1;
            let right = self.parse_and(depth);
            left = match left {
                Node::Or(mut items) => {
                    items.push(right);
                    Node::Or(items)
                }
                other => Node::Or(vec![other, right]),
            };
        }
        left
    }

    fn parse_and(&mut self, depth: usize) -> Node {
        let mut items: Vec<Node> = Vec::new();
        while let Some(tok) = self.peek() {
            match tok.kind {
                TokKind::RParen | TokKind::Or => break,
                _ => {}
            }
            items.push(self.parse_unary(depth));
        }
        match items.len() {
            0 => Node::Never,
            1 => items.pop().unwrap_or(Node::Never),
            _ => Node::And(items),
        }
    }

    fn parse_unary(&mut self, depth: usize) -> Node {
        // The negation flag rides on the token it applies to; the token itself
        // is still the operand, so it must not be consumed here.
        let negate = self.peek().is_some_and(|t| t.negate);
        let node = self.parse_primary(depth);
        if negate {
            Node::Not(Box::new(node))
        } else {
            node
        }
    }

    fn parse_primary(&mut self, depth: usize) -> Node {
        let Some(tok) = self.peek().cloned() else {
            return Node::Never;
        };
        if matches!(tok.kind, TokKind::LParen) {
            self.pos += 1;
            if depth >= MAX_DEPTH {
                // Consume the group so the rest of the query is still parsed,
                // then refuse to guess what it meant.
                self.depth_exceeded = true;
                let mut balance = 1i32;
                while let Some(t) = self.peek() {
                    match t.kind {
                        TokKind::LParen => balance += 1,
                        TokKind::RParen => balance -= 1,
                        _ => {}
                    }
                    self.pos += 1;
                    if balance == 0 {
                        break;
                    }
                }
                return Node::Never;
            }
            let inner = self.parse_or(depth + 1);
            if matches!(self.peek().map(|t| &t.kind), Some(TokKind::RParen)) {
                self.pos += 1;
            }
            return inner;
        }
        if matches!(tok.kind, TokKind::RParen) {
            self.pos += 1;
            return self.parse_unary(depth);
        }
        self.pos += 1;
        leaf(&tok, &mut self.hints)
    }
}

/// True when a `key:value` token looks like an operator rather than a URL or a
/// time-of-day. Only operator-shaped tokens get an "unknown operator" hint.
fn looks_like_operator(key: &str, value: &str) -> bool {
    !key.is_empty()
        && key.len() <= 20
        && key.chars().all(|c| c.is_ascii_alphabetic() || c == '_')
        && !value.is_empty()
        && !value.starts_with('/')
}

fn leaf(tok: &Token, hints: &mut Vec<QueryHint>) -> Node {
    let text = tok.text.trim();
    if text.is_empty() {
        return Node::Never;
    }
    if let Some((key, value)) = text.split_once(':') {
        let kl = key.trim().to_ascii_lowercase();
        let value = value.trim();
        let literal = |hints: &mut Vec<QueryHint>| {
            if looks_like_operator(key.trim(), value) {
                hints.push(QueryHint::new(
                    "unknown_operator",
                    format!("Sift searched “{text}” as text. Search Gmail for full syntax."),
                    Some(text.to_string()),
                ));
            }
            Node::Term(text.to_string())
        };
        if value.is_empty() {
            return literal(hints);
        }
        match kl.as_str() {
            "from" => return Node::Pred(Pred::From(value.to_string())),
            "to" => return Node::Pred(Pred::To(value.to_string())),
            "cc" => return Node::Pred(Pred::Cc(value.to_string())),
            // A quoted subject is an exact phrase; an unquoted one is a
            // case-insensitive substring. Both keep the whole value, which the
            // old parser lost at the first space.
            "subject" => return Node::Pred(Pred::Subject(value.to_string())),
            "label" => return Node::Pred(Pred::Label(value.to_string())),
            "in" => {
                return match Mailbox::parse(value) {
                    Some(mb) => Node::Pred(Pred::In(mb)),
                    None => literal(hints),
                }
            }
            "has" => {
                return if value.eq_ignore_ascii_case("attachment") {
                    Node::Pred(Pred::HasAttachment)
                } else {
                    literal(hints)
                }
            }
            "is" => {
                return match value.to_ascii_lowercase().as_str() {
                    "read" => Node::Pred(Pred::IsUnread(false)),
                    "unread" => Node::Pred(Pred::IsUnread(true)),
                    "starred" => Node::Pred(Pred::IsStarred(true)),
                    _ => literal(hints),
                }
            }
            "before" | "after" => {
                return match parse_day(value) {
                    Some(ts) => Node::Pred(if kl == "before" {
                        Pred::Before(ts)
                    } else {
                        Pred::After(ts)
                    }),
                    None => {
                        // Never drop a date filter: dropping it would silently
                        // return the whole mailbox.
                        hints.push(QueryHint::new(
                            "invalid_date",
                            format!(
                                "“{value}” is not a date Sift understands for {kl}: use YYYY-MM-DD. With dates cleared, this query matches nothing."
                            ),
                            Some(text.to_string()),
                        ));
                        Node::Never
                    }
                }
            }
            _ => return literal(hints),
        }
    }
    if tok.quoted {
        Node::Phrase(text.to_string())
    } else {
        Node::Term(text.to_string())
    }
}

/// Parse a query string. Never fails: unparsable input becomes a hint plus a
/// node that cannot broaden the search.
pub fn parse(q: &str) -> Parsed {
    let (tokens, mut hints) = tokenize(q);
    if tokens.is_empty() {
        return Parsed {
            raw: q.to_string(),
            ast: None,
            hints,
            ast_version: AST_VERSION,
        };
    }
    let mut parser = Parser {
        tokens,
        pos: 0,
        hints: Vec::new(),
        depth_exceeded: false,
    };
    let ast = parser.parse_or(0);
    if parser.pos < parser.tokens.len() {
        // A stray `)` at the top level: keep the parsed part, note the rest.
        hints.push(QueryHint::new(
            "unbalanced_parenthesis",
            "Sift ignored an extra “)”.",
            None,
        ));
    }
    if parser.depth_exceeded {
        hints.push(QueryHint::new(
            "too_deep",
            format!("This query nests deeper than {MAX_DEPTH} levels; Sift searched nothing for the over-nested part."),
            None,
        ));
    }
    hints.extend(parser.hints);
    Parsed {
        raw: q.to_string(),
        ast: Some(ast),
        hints,
        ast_version: AST_VERSION,
    }
}

/// Visit every predicate in the tree.
pub fn walk<'a>(node: &'a Node, visit: &mut dyn FnMut(&'a Pred)) {
    match node {
        Node::And(items) | Node::Or(items) => {
            for item in items {
                walk(item, visit);
            }
        }
        Node::Not(inner) => walk(inner, visit),
        Node::Pred(p) => visit(p),
        Node::Term(_) | Node::Phrase(_) | Node::Never => {}
    }
}

/// True when the query *asserts* a mailbox, which replaces the default
/// "everything except Trash and Junk" scope.
///
/// An exclusion does not: `-in:inbox` narrows within the default scope rather
/// than opting out of it, so Trash and Junk stay excluded.
pub fn names_mailbox(node: &Node) -> bool {
    fn walk(node: &Node, positive: bool) -> bool {
        match node {
            Node::And(items) | Node::Or(items) => items.iter().any(|item| walk(item, positive)),
            Node::Not(inner) => walk(inner, !positive),
            Node::Pred(Pred::In(_)) => positive,
            _ => false,
        }
    }
    walk(node, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(q: &str) -> Parsed {
        parse(q)
    }

    #[test]
    fn p7_t01_parses_every_advertised_operator() {
        let p = parsed("from:ada@example.com to:bob@example.com cc:carol subject:report label:\"Client Work\" in:sent has:attachment is:unread -from:spam@x.com before:2026-03-02 after:2026-03-01 \"quarterly numbers\"");
        assert!(!p.is_fatal());
        assert!(p.hints.is_empty(), "{:?}", p.hints);
        let mut preds: Vec<Pred> = Vec::new();
        if let Some(ast) = &p.ast {
            walk(ast, &mut |x| preds.push(x.clone()));
        }
        assert!(preds.contains(&Pred::From("ada@example.com".into())));
        assert!(preds.contains(&Pred::To("bob@example.com".into())));
        assert!(preds.contains(&Pred::Cc("carol".into())));
        assert!(preds.contains(&Pred::Subject("report".into())));
        assert!(preds.contains(&Pred::Label("Client Work".into())));
        assert!(preds.contains(&Pred::In(Mailbox::Sent)));
        assert!(preds.contains(&Pred::HasAttachment));
        assert!(preds.contains(&Pred::IsUnread(true)));
        assert!(preds.iter().any(|x| matches!(x, Pred::Before(_))));
        assert!(preds.iter().any(|x| matches!(x, Pred::After(_))));
        let Node::And(items) = p.ast.clone().unwrap() else {
            panic!("expected conjunction")
        };
        assert!(items.iter().any(|n| matches!(n, Node::Phrase(t) if t == "quarterly numbers")));
        assert!(items
            .iter()
            .any(|n| matches!(n, Node::Not(inner) if matches!(**inner, Node::Pred(Pred::From(_))))));
        // before excludes the day's start, after includes it.
        let before = preds
            .iter()
            .find_map(|x| match x {
                Pred::Before(ts) => Some(*ts),
                _ => None,
            })
            .unwrap();
        let after = preds
            .iter()
            .find_map(|x| match x {
                Pred::After(ts) => Some(*ts),
                _ => None,
            })
            .unwrap();
        let mar1 = local_day_start_ms(2026, 3, 1).unwrap();
        let mar2 = local_day_start_ms(2026, 3, 2).unwrap();
        assert_eq!(after, mar1);
        assert_eq!(before, mar2);
    }

    #[test]
    fn p7_t02_is_read_and_starred_carry_their_value() {
        let mut seen = vec![];
        if let Some(ast) = parsed("is:read").ast {
            walk(&ast, &mut |p| seen.push(p.clone()));
        }
        assert_eq!(seen, vec![Pred::IsUnread(false)]);
        let mut seen = vec![];
        if let Some(ast) = parsed("is:starred").ast {
            walk(&ast, &mut |p| seen.push(p.clone()));
        }
        assert_eq!(seen, vec![Pred::IsStarred(true)]);
    }

    #[test]
    fn p7_t03_invalid_date_never_broadens() {
        let p = parsed("from:ada before:not-a-date");
        assert!(p.is_fatal());
        assert_eq!(p.hints[0].kind, "invalid_date");
        let ast = p.ast.unwrap();
        let mut has_never = false;
        fn scan(n: &Node, f: &mut bool) {
            match n {
                Node::Never => *f = true,
                Node::And(v) | Node::Or(v) => v.iter().for_each(|x| scan(x, f)),
                Node::Not(x) => scan(x, f),
                _ => {}
            }
        }
        scan(&ast, &mut has_never);
        assert!(has_never, "an unparsable date must not vanish: {ast:?}");
    }

    #[test]
    fn p7_t04_unclosed_quote_is_recoverable_not_silent() {
        let p = parsed("subject:\"q3 numbers");
        assert_eq!(p.hints.len(), 1);
        assert_eq!(p.hints[0].kind, "unclosed_quote");
        assert!(!p.is_fatal());
        let mut seen = vec![];
        if let Some(ast) = p.ast {
            walk(&ast, &mut |x| seen.push(x.clone()));
        }
        assert_eq!(seen, vec![Pred::Subject("q3 numbers".into())]);
    }

    #[test]
    fn p7_t05_unknown_operator_survives_as_literal_text() {
        let p = parsed("foo:bar hello");
        assert!(p.hints.iter().any(|h| h.kind == "unknown_operator"));
        let Node::And(items) = p.ast.unwrap() else {
            panic!("expected two nodes")
        };
        assert_eq!(items[0], Node::Term("foo:bar".into()));
        assert_eq!(items[1], Node::Term("hello".into()));
        // A URL is not reported as an unknown operator.
        let url = parsed("https://example.com/x");
        assert!(url.hints.is_empty(), "{:?}", url.hints);
    }

    #[test]
    fn p7_t06_or_and_parentheses() {
        let p = parsed("(from:a OR from:b) subject:c");
        assert!(p.hints.is_empty(), "{:?}", p.hints);
        let Node::And(items) = p.ast.unwrap() else {
            panic!("expected conjunction")
        };
        assert_eq!(items.len(), 2);
        let Node::Or(branches) = &items[0] else {
            panic!("expected disjunction, got {:?}", items[0])
        };
        assert_eq!(branches.len(), 2);
        assert_eq!(items[1], Node::Pred(Pred::Subject("c".into())));
    }

    #[test]
    fn p7_t07_depth_and_token_bounds() {
        let deep = "(".repeat(MAX_DEPTH + 5) + "a" + &")".repeat(MAX_DEPTH + 5);
        let p = parsed(&deep);
        assert!(p.hints.iter().any(|h| h.kind == "too_deep"));
        assert!(p.is_fatal());

        let wide = (0..MAX_TOKENS + 20)
            .map(|i| format!("w{i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let p = parsed(&wide);
        assert!(p.hints.iter().any(|h| h.kind == "too_many_tokens"));
        assert!(!p.is_fatal());
    }

    #[test]
    fn p7_t08_tabs_and_whitespace_are_equivalent() {
        let a = parsed("from:ada\thas:attachment");
        let b = parsed("from:ada has:attachment");
        assert_eq!(a.ast, b.ast);
    }

    #[test]
    fn p7_t09_punctuation_and_cjk_terms() {
        for q in ["C++", "--", "日本語", "café", "a.b@c.com"] {
            let p = parsed(q);
            assert!(p.ast.is_some(), "{q}");
            assert!(p.hints.is_empty(), "{q}: {:?}", p.hints);
        }
        assert_eq!(parsed("\"日 本\"").ast, Some(Node::Phrase("日 本".into())));
    }

    #[test]
    fn p7_t10_empty_query_has_no_ast() {
        assert!(parsed("   ").ast.is_none());
    }
}
