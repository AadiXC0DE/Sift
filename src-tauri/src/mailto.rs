//! `mailto:` deep links (P9.3).
//!
//! A `mailto:` URL is untrusted input arriving from anywhere on the system, so
//! the parser is deliberately narrow: the scheme, a comma-separated address
//! list, and the five header fields RFC 6068 defines for a composed message
//! (`to`, `cc`, `bcc`, `subject`, `body`). Anything else — an unknown header, a
//! CR or LF anywhere, an `attach` parameter — is rejected outright rather than
//! sanitised, because the only safe interpretation of a crafted link is "no".
//!
//! Two invariants hold no matter what arrives:
//!
//! * **nothing is ever sent.** Parsing produces the fields of a draft; the
//!   user still has to press Send.
//! * **nothing reaches a header it did not name.** The values are data — they
//!   go into the draft body and the address lists the composer already builds
//!   MIME from, never into a header line assembled out of raw text.
//!
//! Percent-decoding is applied once, per component, with `+` left alone (in
//! `mailto:` a `+` is a literal plus, unlike in a form-encoded query string).

use crate::dto::PendingMailto;
use crate::errors::SiftError;

/// The longest URL Sift will look at.
pub const MAX_URL_LEN: usize = 8192;
/// RFC 5322 caps a header line at 998 bytes; a subject match longer than this
/// cannot have come from a well-behaved client.
pub const MAX_SUBJECT_LEN: usize = 998;
/// The body is user content in a text area; anything larger is a data transfer
/// wearing a mailto URL's clothes.
pub const MAX_BODY_LEN: usize = 32 * 1024;
/// Cap on the number of addresses in any one list.
pub const MAX_RECIPIENTS: usize = 100;

const ALLOWED_FIELDS: [&str; 5] = ["to", "cc", "bcc", "subject", "body"];

fn invalid(message: &str) -> SiftError {
    SiftError::app("mailto_invalid", message.to_string(), false)
}

/// Percent-decode one component.
///
/// `+` is *not* a space here: RFC 6068 defines the query component as
/// percent-encoded, and decoding `+` as a space would silently corrupt every
/// address containing one (`first+tag@example.com`).
pub fn percent_decode(input: &str) -> Result<String, SiftError> {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hex = bytes
                    .get(i + 1..i + 3)
                    .and_then(|h| std::str::from_utf8(h).ok())
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .ok_or_else(|| invalid("That link contains a broken escape sequence."))?;
                out.push(hex);
                i += 3;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| invalid("That link is not valid UTF-8."))
}

/// Reject anything that could break out of a field it was meant to fill.
fn assert_single_line(value: &str, what: &str) -> Result<(), SiftError> {
    if value.contains('\r') || value.contains('\n') || value.contains('\0') {
        return Err(invalid(&format!(
            "Sift refused that link: it tries to put a line break in the {what}."
        )));
    }
    if value.chars().any(|c| c.is_control() && c != '\t') {
        return Err(invalid(&format!(
            "Sift refused that link: it contains control characters in the {what}."
        )));
    }
    Ok(())
}

/// One address, as far as Sift is willing to accept one from a URL.
///
/// Deliberately conservative: a single `@`, no whitespace, no angle brackets,
/// no comment syntax. A `mailto:` link is not a place to accept a display-name
/// form, and accepting one would mean parsing an address grammar whose corner
/// cases are exactly where injection lives.
fn parse_address(raw: &str) -> Result<String, SiftError> {
    let value = raw.trim();
    if value.is_empty() {
        return Err(invalid("That link names an empty address."));
    }
    assert_single_line(value, "address")?;
    if value.len() > 320 {
        return Err(invalid("That link names an address that is too long."));
    }
    let mut parts = value.split('@');
    let (local, domain) = match (parts.next(), parts.next(), parts.next()) {
        (Some(l), Some(d), None) => (l, d),
        _ => return Err(invalid("That link contains something that is not an address.")),
    };
    if local.is_empty()
        || domain.is_empty()
        || !domain.contains('.')
        || domain.starts_with('.')
        || domain.ends_with('.')
        || domain.starts_with('-')
        || value.contains(|c: char| c.is_whitespace() || c == '<' || c == '>' || c == '"' || c == '(' || c == ')')
    {
        return Err(invalid("That link contains something that is not an address."));
    }
    Ok(value.to_string())
}

fn push_addresses(list: &mut Vec<String>, raw: &str, what: &str) -> Result<(), SiftError> {
    for part in raw.split(',') {
        if part.trim().is_empty() {
            continue;
        }
        if list.len() >= MAX_RECIPIENTS {
            return Err(invalid(&format!("That link names more {what} than Sift will use.")));
        }
        let address = parse_address(part)?;
        if !list.iter().any(|a| a.eq_ignore_ascii_case(&address)) {
            list.push(address);
        }
    }
    Ok(())
}

/// Parse a `mailto:` URL into the fields of a new draft.
pub fn parse(url: &str) -> Result<PendingMailto, SiftError> {
    if url.len() > MAX_URL_LEN {
        return Err(invalid("That link is too long for Sift to open."));
    }
    let rest = url
        .strip_prefix("mailto:")
        .or_else(|| url.strip_prefix("MAILTO:"))
        .or_else(|| {
            // A case-insensitive scheme match that does not rely on ASCII case
            // folding of the whole URL (the rest is case-sensitive data).
            let lower = url.get(..7)?.to_ascii_lowercase();
            (lower == "mailto:").then(|| &url[7..])
        })
        .ok_or_else(|| invalid("Sift only opens mailto: links."))?;

    let (path, query) = match rest.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (rest, None),
    };

    let mut out = PendingMailto::default();
    push_addresses(&mut out.to, &percent_decode(path)?, "recipients")?;

    if let Some(query) = query {
        for pair in query.split('&') {
            if pair.is_empty() {
                continue;
            }
            let (key, value) = match pair.split_once('=') {
                Some((k, v)) => (k, v),
                None => (pair, ""),
            };
            let key = percent_decode(key)?.to_ascii_lowercase();
            if !ALLOWED_FIELDS.contains(&key.as_str()) {
                return Err(invalid(&format!(
                    "Sift refused that link: \"{key}\" is not something a mailto link may set."
                )));
            }
            let value = percent_decode(value)?;
            match key.as_str() {
                "to" => push_addresses(&mut out.to, &value, "recipients")?,
                "cc" => {
                    assert_single_line(&value, "cc list")?;
                    push_addresses(&mut out.cc, &value, "cc recipients")?
                }
                "bcc" => {
                    assert_single_line(&value, "bcc list")?;
                    push_addresses(&mut out.bcc, &value, "bcc recipients")?
                }
                "subject" => {
                    assert_single_line(&value, "subject")?;
                    if value.len() > MAX_SUBJECT_LEN {
                        return Err(invalid("That link's subject is too long."));
                    }
                    // A second subject combines rather than overwrites: the
                    // link said two things and dropping one would be a lie.
                    if out.subject.is_empty() {
                        out.subject = value;
                    } else {
                        out.subject.push_str(&value);
                    }
                }
                "body" => {
                    if value.len() > MAX_BODY_LEN {
                        return Err(invalid("That link's message body is too large."));
                    }
                    // Newlines are legitimate in a body; they are the one field
                    // where a line break means what it says.
                    if value.contains('\0') {
                        return Err(invalid("Sift refused that link's message body."));
                    }
                    out.body.push_str(&value);
                }
                _ => unreachable!("the allowlist above is exhaustive"),
            }
        }
    }
    if out.body.len() > MAX_BODY_LEN {
        return Err(invalid("That link's message body is too large."));
    }
    Ok(out)
}

/// Whether a URL is one Sift should treat as a compose request.
pub fn is_mailto(url: &str) -> bool {
    url.get(..7)
        .map(|s| s.eq_ignore_ascii_case("mailto:"))
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p9_3_encoded_subject_and_body_are_decoded() {
        let parsed = parse(
            "mailto:ada@example.com?subject=Q3%20figures&cc=bob%40example.com&body=Hello%0AWorld",
        )
        .unwrap();
        assert_eq!(parsed.to, vec!["ada@example.com"]);
        assert_eq!(parsed.cc, vec!["bob@example.com"]);
        assert_eq!(parsed.subject, "Q3 figures");
        assert_eq!(parsed.body, "Hello\nWorld");
    }

    #[test]
    fn p9_3_multiple_recipients_in_path_and_query() {
        let parsed =
            parse("mailto:ada@example.com,bob@example.com?to=cid@example.com").unwrap();
        assert_eq!(
            parsed.to,
            vec!["ada@example.com", "bob@example.com", "cid@example.com"]
        );
        // A duplicate is one recipient, not two.
        let parsed = parse("mailto:ADA@example.com?to=ada@example.com").unwrap();
        assert_eq!(parsed.to, vec!["ADA@example.com"]);
        // A bare address is valid.
        assert_eq!(parse("mailto:ada@example.com").unwrap().to.len(), 1);
        assert_eq!(parse("MAILTO:ada@example.com").unwrap().to.len(), 1);
    }

    #[test]
    fn p9_3_plus_is_literal_not_a_space() {
        let parsed = parse("mailto:first+tag@example.com").unwrap();
        assert_eq!(parsed.to, vec!["first+tag@example.com"]);
        let parsed = parse("mailto:x@example.com?subject=a+b").unwrap();
        assert_eq!(parsed.subject, "a+b");
        assert_eq!(percent_decode("%20").unwrap(), " ");
    }

    #[test]
    fn p9_3_header_injection_and_unknown_fields_are_rejected() {
        // A newline, encoded or literal, never reaches a field.
        for url in [
            "mailto:a@example.com?subject=Hi%0ABcc:evil@example.com",
            "mailto:a@example.com?subject=Hi%0D%0ABcc:evil@example.com",
            "mailto:a@example.com?subject=Hi\nBcc:x@y.com",
            "mailto:a@example.com?bcc=a@example.com%0AX-Evil:1",
        ] {
            let err = parse(url).unwrap_err();
            assert_eq!(err.code(), "mailto_invalid", "{url}");
        }
        // Anything that is not one of the five fields is refused outright.
        for url in [
            "mailto:a@example.com?x-evil=1",
            "mailto:a@example.com?attach=/etc/passwd",
            "mailto:a@example.com?in-reply-to=%3C1@x%3E",
            "mailto:a@example.com?to=a@example.com&foo=bar",
        ] {
            assert_eq!(parse(url).unwrap_err().code(), "mailto_invalid", "{url}");
        }
        // Malformed escapes, non-UTF8 escapes and non-addresses are refused.
        assert_eq!(parse("mailto:a@example.com?subject=%zz").unwrap_err().code(), "mailto_invalid");
        assert_eq!(parse("mailto:%FF%FE").unwrap_err().code(), "mailto_invalid");
        assert_eq!(parse("mailto:not-an-address").unwrap_err().code(), "mailto_invalid");
        assert_eq!(parse("mailto:a@example.com?to=bad@@example.com").unwrap_err().code(), "mailto_invalid");
        assert_eq!(parse("https://example.com").unwrap_err().code(), "mailto_invalid");
    }

    #[test]
    fn p9_3_oversized_links_are_refused() {
        let long_subject = "a".repeat(MAX_SUBJECT_LEN + 1);
        assert_eq!(
            parse(&format!("mailto:a@example.com?subject={long_subject}"))
                .unwrap_err()
                .code(),
            "mailto_invalid"
        );
        let long_body = "b".repeat(MAX_BODY_LEN + 1);
        assert_eq!(
            parse(&format!("mailto:a@example.com?body={long_body}"))
                .unwrap_err()
                .code(),
            "mailto_invalid"
        );
        let many: Vec<String> = (0..(MAX_RECIPIENTS + 5))
            .map(|i| format!("u{i}@example.com"))
            .collect();
        assert_eq!(
            parse(&format!("mailto:{}", many.join(",")))
                .unwrap_err()
                .code(),
            "mailto_invalid"
        );
        let huge = format!("mailto:a@example.com?body={}", "c".repeat(MAX_URL_LEN));
        assert_eq!(parse(&huge).unwrap_err().code(), "mailto_invalid");
    }

    #[test]
    fn p9_3_parsing_never_sends_anything() {
        // The only output of a parse is draft fields; there is no send here and
        // no queue: the user still has to compose and press Send.
        let parsed = parse("mailto:ada@example.com?subject=Hi&body=%0A%0A").unwrap();
        assert_eq!(parsed.subject, "Hi");
        assert!(!is_mailto("https://example.com"));
        assert!(is_mailto("MAILTO:x@y.com"));
    }
}
