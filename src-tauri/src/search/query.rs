#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    pub terms: String,
    pub from: Option<String>,
    pub to: Option<String>,
    pub subject: Option<String>,
    pub has_attachment: bool,
    pub is_unread: Option<bool>,
    pub is_starred: Option<bool>,
    pub labels: Vec<String>,
    pub before: Option<i64>,
    pub after: Option<i64>,
    pub in_: Option<String>,
}

fn parse_date(s: &str) -> Option<i64> {
    // YYYY-MM-DD -> unix ms (UTC midnight)
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 3 {
        return None;
    }
    let (y, m, d): (i32, u32, u32) = (
        parts[0].parse().ok()?,
        parts[1].parse().ok()?,
        parts[2].parse().ok()?,
    );
    let date = chrono::NaiveDate::from_ymd_opt(y, m, d)?;
    Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis())
}

pub fn parse(q: &str) -> Query {
    let mut out = Query::default();
    let mut terms: Vec<String> = vec![];
    let it = q.split_whitespace().peekable();
    // handle quoted phrases
    let mut tokens: Vec<String> = vec![];
    let mut buf = String::new();
    let mut in_q = false;
    for ch in q.chars() {
        match ch {
            '"' => {
                in_q = !in_q;
                buf.push(ch);
                if !in_q {
                    tokens.push(buf.clone());
                    buf.clear();
                }
            }
            ' ' if !in_q => {
                if !buf.is_empty() {
                    tokens.push(buf.clone());
                    buf.clear();
                }
            }
            _ => buf.push(ch),
        }
    }
    if !buf.is_empty() {
        tokens.push(buf);
    }
    let _ = it;
    for tok in tokens {
        let t = tok.trim_matches('"').to_string();
        if let Some((k, v)) = tok.split_once(':') {
            let kl = k.to_lowercase();
            let v = v.trim_matches('"').to_string();
            match kl.as_str() {
                "from" => {
                    out.from = Some(v);
                    continue;
                }
                "to" => {
                    out.to = Some(v);
                    continue;
                }
                "subject" => {
                    out.subject = Some(v);
                    continue;
                }
                "label" => {
                    out.labels.push(v);
                    continue;
                }
                "in" => {
                    out.in_ = Some(v);
                    continue;
                }
                "has" if v == "attachment" => {
                    out.has_attachment = true;
                    continue;
                }
                "is" if v == "unread" => {
                    out.is_unread = Some(true);
                    continue;
                }
                "is" if v == "read" => {
                    out.is_unread = Some(false);
                    continue;
                }
                "is" if v == "starred" => {
                    out.is_starred = Some(true);
                    continue;
                }
                "before" => {
                    out.before = parse_date(&v);
                    continue;
                }
                "after" => {
                    out.after = parse_date(&v);
                    continue;
                }
                _ => {}
            }
        }
        // bare is:unread style without split? already handled; unknown ops become terms
        if tok.starts_with("is:")
            || tok.starts_with("has:")
            || tok.starts_with("in:")
            || tok.starts_with("label:")
            || tok.starts_with("before:")
            || tok.starts_with("after:")
        {
            // known prefixes with empty parse fall through as terms
            if !tok.contains(':') {
                terms.push(t);
                continue;
            }
            // if we reach here the operator was unknown
            if !(tok.starts_with("from:") || tok.starts_with("to:") || tok.starts_with("subject:"))
            {
                // re-check: unknown -> term
                let known = [
                    "from:", "to:", "subject:", "has:", "is:", "label:", "before:", "after:", "in:",
                ];
                if !known.iter().any(|k| tok.to_lowercase().starts_with(k)) {
                    terms.push(t);
                    continue;
                }
                // known but unparsed (e.g. has:foo) -> term
                if tok.starts_with("has:") && tok != "has:attachment" {
                    terms.push(t);
                    continue;
                }
            }
        }
        terms.push(t);
    }
    out.terms = terms.join(" ");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn p8_t01_parse_full() {
        let q = parse("from:ada has:attachment before:2026-01-01 \"q3 numbers\" is:unread");
        assert_eq!(q.from.as_deref(), Some("ada"));
        assert!(q.has_attachment);
        assert_eq!(q.is_unread, Some(true));
        assert!(q.terms.contains("q3 numbers"));
        assert!(q.before.is_some());
        let q2 = parse("foo:bar hello");
        assert!(q2.terms.contains("foo:bar") || q2.terms.contains("hello"));
    }
}
