//! Raw MIME bytes → Gmail `Message` tree → [`ParsedMessage`][super::super::gmail::mime::ParsedMessage].
//!
//! IMAP serves raw RFC 5322; the REST path parses Gmail's JSON `payload`
//! tree. Both funnel into `mime::parse_full`, so bodies, attachments, quotes,
//! and address handling are byte-identical across transports (P11-T08).
//! mail-parser does the charset/transfer/RFC2047 decoding; this module only
//! reshapes its part graph into the tree `parse_full` expects, tracking IMAP
//! section numbers (`"1"`, `"1.2"`) as part ids.
use crate::provider::gmail::{
    mime,
    types::{Body, Header, Message, MessagePart},
};

/// Convert raw message bytes into a parsed message via the shared REST path.
/// Returns None when the bytes are not a message at all.
pub fn parse_raw(raw: &[u8]) -> Option<mime::ParsedMessage> {
    let tree = to_gmail_tree(raw)?;
    Some(mime::parse_full(&tree))
}

/// mail-parser message → Gmail `Message` payload tree. Section numbers follow
/// BODYSTRUCTURE conventions so part ids double as IMAP section paths.
pub fn to_gmail_tree(raw: &[u8]) -> Option<Message> {
    let msg = mail_parser::MessageParser::new().parse(raw)?;
    let mut headers = vec![];
    for h in msg.headers_raw() {
        // Only real field names (printable ASCII, no colon): mail-parser is
        // lenient and will happily "parse" binary garbage into junk fields.
        if !is_field_name(h.0) {
            continue;
        }
        headers.push(Header {
            name: h.0.to_string(),
            // Trim OWS/CRLF remnants: the Gmail API never includes them in
            // header values, and parse_full compares/round-trips them.
            value: h.1.trim().to_string(),
        });
    }
    if headers.is_empty() {
        // No parseable headers: not a message we can represent.
        return None;
    }
    let mut parts = vec![];
    walk(&msg, 0, String::new(), &mut parts);
    Some(Message {
        id: String::new(),
        thread_id: String::new(),
        label_ids: None,
        snippet: None,
        history_id: None,
        internal_date: None,
        size_estimate: Some(raw.len() as i64),
        payload: Some(MessagePart {
            part_id: None,
            mime_type: Some(mime_of_root(&msg)),
            filename: None,
            headers: Some(headers),
            body: None,
            parts: if parts.is_empty() { None } else { Some(parts) },
        }),
        raw: None,
    })
}

fn mime_of_root(msg: &mail_parser::Message) -> String {
    let root = msg.root_part();
    match &root.body {
        mail_parser::PartType::Multipart(_) => "multipart/mixed".into(),
        mail_parser::PartType::Text(_) => "text/plain".into(),
        mail_parser::PartType::Html(_) => "text/html".into(),
        mail_parser::PartType::Binary(_) | mail_parser::PartType::InlineBinary(_) => {
            let raw = header_raw(msg, 0, "content-type").unwrap_or_default();
            let (mime, _) = split_content_type(&raw);
            if mime.is_empty() {
                "application/octet-stream".into()
            } else {
                mime
            }
        }
        mail_parser::PartType::Message(_) => "message/rfc822".into(),
    }
}

fn walk(msg: &mail_parser::Message, id: usize, section: String, out: &mut Vec<MessagePart>) {
    let part = &msg.parts[id];
    match &part.body {
        mail_parser::PartType::Multipart(ids) => {
            for (i, sub) in ids.iter().enumerate() {
                let n = if section.is_empty() {
                    format!("{}", i + 1)
                } else {
                    format!("{}.{}", section, i + 1)
                };
                walk(msg, *sub, n, out);
            }
            // Multipart containers carry no content of their own; the
            // recursion above emitted every leaf with its section number.
            // (Single-part messages fall through to the leaf handling below
            // via the root call when parts.len() == 1 and body is a leaf.)
            if msg.parts.len() == 1 {
                // Degenerate single leaf at root handled by caller.
            }
        }
        _ => {
            out.push(leaf(msg, id, section));
        }
    }
}

/// Single message with no multipart structure: the root part itself is the
/// leaf (to_gmail_tree handles this by checking parts length).
fn leaf(msg: &mail_parser::Message, id: usize, section: String) -> MessagePart {
    use base64::Engine;
    let part = &msg.parts[id];
    let raw_ct = header_raw(msg, id, "content-type").unwrap_or_default();
    let (mime, params) = split_content_type(&raw_ct);
    let disp_raw = header_raw(msg, id, "content-disposition").unwrap_or_default();
    let (disp, disp_params) = split_content_type(&disp_raw);
    let mut filename = disp_params
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("filename"))
        .map(|(_, v)| v.clone())
        .or_else(|| {
            params
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("name"))
                .map(|(_, v)| v.clone())
        });
    if filename.is_none()
        && matches!(
            msg.parts.get(id).map(|p| &p.body),
            Some(mail_parser::PartType::Message(_))
        )
    {
        // Forwarded message: surfaced in the attachment strip, opened on demand.
        filename = Some("forwarded-message.eml".into());
    }
    let cid = header_raw(msg, id, "content-id");
    let (bytes, size) = match &part.body {
        mail_parser::PartType::Text(s) | mail_parser::PartType::Html(s) => {
            (s.as_bytes().to_vec(), s.len() as i64)
        }
        mail_parser::PartType::Binary(b) | mail_parser::PartType::InlineBinary(b) => {
            (b.to_vec(), b.len() as i64)
        }
        mail_parser::PartType::Message(nested) => {
            let b = nested.raw_message.to_vec();
            let n = b.len() as i64;
            (b, n)
        }
        mail_parser::PartType::Multipart(_) => (vec![], 0),
    };
    let mut headers = vec![];
    if let Some(c) = cid {
        headers.push(Header {
            name: "Content-ID".into(),
            value: c,
        });
    }
    if !disp_raw.is_empty() {
        headers.push(Header {
            name: "Content-Disposition".into(),
            value: format!(
                "{disp}{}",
                disp_params
                    .iter()
                    .map(|(k, v)| format!("; {k}=\"{v}\""))
                    .collect::<String>()
            ),
        });
    }
    MessagePart {
        part_id: if section.is_empty() {
            None
        } else {
            Some(section)
        },
        mime_type: Some(if mime.is_empty() {
            "application/octet-stream".into()
        } else {
            mime
        }),
        filename,
        headers: if headers.is_empty() {
            None
        } else {
            Some(headers)
        },
        body: Some(Body {
            attachment_id: None,
            size: Some(size),
            data: Some(base64::engine::general_purpose::URL_SAFE.encode(&bytes)),
        }),
        parts: None,
    }
}

/// Raw header value from a part's byte offsets. Raw (not pre-decoded)
/// text is what the Gmail tree wants — `parse_full` decodes RFC2047 itself.
fn header_raw(msg: &mail_parser::Message, id: usize, name: &str) -> Option<String> {
    let raw = msg.raw_message();
    let part = msg.parts.get(id)?;
    let block = std::str::from_utf8(raw.get(part.offset_header..part.offset_end)?).ok()?;
    // Unfold continuation lines, then find the field (first occurrence wins,
    // matching how the values are used downstream).
    let mut unfolded = String::new();
    for line in block.lines() {
        if line.starts_with([' ', '\t']) {
            unfolded.push(' ');
            unfolded.push_str(line.trim());
        } else {
            unfolded.push('\n');
            unfolded.push_str(line);
        }
    }
    for field in unfolded.split('\n').skip(1) {
        // skip the (empty) pre-first-line segment
        if field.is_empty() {
            continue;
        }
        if let Some((k, v)) = field.split_once(':') {
            if k.trim().eq_ignore_ascii_case(name) {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// RFC 5322 field name: printable ASCII except colon (also rejects the
/// control bytes mail-parser's lenient mode invents for binary input).
fn is_field_name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| (0x21..=0x7E).contains(&b) && b != b':')
}

/// Split `type/subtype; k=v; ...` into (mime, params). Values unquoted.
fn split_content_type(raw: &str) -> (String, Vec<(String, String)>) {
    let mut parts = raw.split(';');
    let mime = parts.next().unwrap_or("").trim().to_string();
    let mut params = vec![];
    for p in parts {
        if let Some((k, v)) = p.split_once('=') {
            params.push((k.trim().to_string(), v.trim().trim_matches('"').to_string()));
        }
    }
    (mime, params)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE: &[u8] = b"From: Ada Lovelace <ada@acme.com>\r\nTo: Ben <ben@globex.io>\r\nSubject: Hello\r\nDate: Wed, 12 Aug 2026 09:14:03 +0000\r\nMessage-ID: <m1@acme.com>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nHello world, this is plain text with a link https://example.com/x.\r\n";
    const ALT_ATTACH: &[u8] = b"From: Ada <ada@acme.com>\r\nTo: Ben <ben@globex.io>\r\nSubject: Files\r\nMessage-ID: <m2@acme.com>\r\nContent-Type: multipart/mixed; boundary=\"MIXED\"\r\n\r\n--MIXED\r\nContent-Type: multipart/alternative; boundary=\"ALT\"\r\n\r\n--ALT\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nPlain version here.\r\n--ALT\r\nContent-Type: text/html; charset=utf-8\r\n\r\n<html><body><p>HTML version <b>here</b>.</p></body></html>\r\n--ALT--\r\n--MIXED\r\nContent-Type: application/pdf; name=\"doc.pdf\"\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"doc.pdf\"\r\n\r\nJVBERi0xLjQK\r\n--MIXED--\r\n";

    #[test]
    fn p11_convert_simple_text() {
        let pm = parse_raw(SIMPLE).expect("parse");
        assert_eq!(pm.subject, "Hello");
        assert_eq!(pm.from_email.as_deref(), Some("ada@acme.com"));
        assert_eq!(pm.from_name.as_deref(), Some("Ada Lovelace"));
        assert!(pm.text.as_deref().unwrap_or("").contains("Hello world"));
        assert!(pm.html.is_none());
        assert!(pm.attachments.is_empty());
    }

    #[test]
    fn p11_convert_alternative_and_attachment() {
        let pm = parse_raw(ALT_ATTACH).expect("parse");
        assert_eq!(pm.subject, "Files");
        let html = pm.html.expect("html");
        assert!(html.contains("HTML version"));
        assert!(pm.text.unwrap().contains("Plain version"));
        assert_eq!(pm.attachments.len(), 1);
        assert_eq!(pm.attachments[0].filename.as_deref(), Some("doc.pdf"));
        assert_eq!(pm.attachments[0].mime, "application/pdf");
        assert!(!pm.attachments[0].data.is_empty());
    }

    #[test]
    fn p11_convert_garbage_is_none() {
        assert!(parse_raw(b"\x00\x01\x02").is_none());
        // Header-only input parses (empty body), never panics.
        let _ = parse_raw(b"From: x@y.z\r\n\r\n");
    }
}
