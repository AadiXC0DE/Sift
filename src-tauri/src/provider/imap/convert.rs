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
    // A non-multipart message is a single part: make the ROOT payload that
    // part, so `parse_full` sees its filename/disposition/body exactly once
    // (a separate container + leaf would duplicate it as an empty root
    // attachment). Multipart roots keep a container payload plus leaves.
    let payload = match &msg.root_part().body {
        mail_parser::PartType::Multipart(_) => {
            let mut parts = vec![];
            walk(&msg, 0, String::new(), &mut parts);
            MessagePart {
                part_id: None,
                mime_type: Some("multipart/mixed".into()),
                filename: None,
                headers: Some(headers),
                body: None,
                parts: if parts.is_empty() { None } else { Some(parts) },
            }
        }
        _ => {
            let mut root = leaf(&msg, 0, "1".to_string());
            let mut all_headers = headers;
            if let Some(extra) = root.headers.take() {
                for h in extra {
                    if !all_headers
                        .iter()
                        .any(|x| x.name.eq_ignore_ascii_case(&h.name))
                    {
                        all_headers.push(h);
                    }
                }
            }
            root.headers = Some(all_headers);
            root
        }
    };
    Some(Message {
        id: String::new(),
        thread_id: String::new(),
        label_ids: None,
        snippet: None,
        history_id: None,
        internal_date: None,
        size_estimate: Some(raw.len() as i64),
        payload: Some(payload),
        raw: None,
    })
}

/// Declared MIME of one part, from mail-parser's decoded Content-Type. Falls
/// back to the parsed body variant when the header is absent or malformed.
fn mime_of_part(part: &mail_parser::MessagePart<'_>) -> String {
    use mail_parser::MimeHeaders;
    if let Some(ct) = part.content_type() {
        if let Some(st) = ct.subtype() {
            return format!("{}/{}", ct.ctype(), st).to_lowercase();
        }
        return ct.ctype().to_lowercase();
    }
    match &part.body {
        mail_parser::PartType::Text(_) => "text/plain".into(),
        mail_parser::PartType::Html(_) => "text/html".into(),
        mail_parser::PartType::Message(_) => "message/rfc822".into(),
        mail_parser::PartType::Binary(_) | mail_parser::PartType::InlineBinary(_) => {
            "application/octet-stream".into()
        }
        mail_parser::PartType::Multipart(_) => "multipart/mixed".into(),
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
        }
        _ => {
            // IMAP section numbering: a non-multipart ROOT's body is section
            // `1`, matching `proto::walk_parts` (P2.7). Without this a
            // single-part attachment had an empty part id and no locator.
            let section = if section.is_empty() {
                "1".to_string()
            } else {
                section
            };
            out.push(leaf(msg, id, section));
        }
    }
}

/// One MIME leaf → Gmail-tree part. Filenames and parameters come from
/// mail-parser's decoded headers, so RFC 2231 continuations, RFC 2047 words
/// and quoted/semicolon filenames are correct without any manual splitting
/// (P2.7).
fn leaf(msg: &mail_parser::Message, id: usize, section: String) -> MessagePart {
    use base64::Engine;
    use mail_parser::MimeHeaders;
    let part = &msg.parts[id];
    let mut filename = part.attachment_name().map(str::to_string);
    if filename.is_none() && matches!(part.body, mail_parser::PartType::Message(_)) {
        // Forwarded message: surfaced in the attachment strip, opened on demand.
        filename = Some("forwarded-message.eml".into());
    }
    let cid = part.content_id().map(str::to_string);
    let (bytes, size) = match &part.body {
        mail_parser::PartType::Text(s) | mail_parser::PartType::Html(s) => {
            (s.as_bytes().to_vec(), s.len() as i64)
        }
        mail_parser::PartType::Binary(b) | mail_parser::PartType::InlineBinary(b) => {
            (b.to_vec(), b.len() as i64)
        }
        mail_parser::PartType::Message(nested) => {
            // The RAW encapsulated message is what section `section` returns.
            let b = nested.raw_message().to_vec();
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
    let text_like = matches!(
        &part.body,
        mail_parser::PartType::Text(_) | mail_parser::PartType::Html(_)
    );
    match part.content_disposition() {
        Some(disp) => headers.push(Header {
            name: "Content-Disposition".into(),
            value: disp.ctype().to_string(),
        }),
        // An unnamed, non-text leaf is still an attachment (P2.7); the shared
        // parser keys on filename/Content-Disposition, so mark it explicitly
        // rather than dropping the bytes.
        None if !text_like => headers.push(Header {
            name: "Content-Disposition".into(),
            value: "attachment".into(),
        }),
        None => {}
    }
    MessagePart {
        part_id: Some(section),
        mime_type: Some(mime_of_part(part)),
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

/// RFC 5322 field name: printable ASCII except colon (also rejects the
/// control bytes mail-parser's lenient mode invents for binary input).
fn is_field_name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| (0x21..=0x7E).contains(&b) && b != b':')
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
        // Nested multipart numbering: the PDF after multipart/alternative is
        // IMAP section 2, never a display index (P2.7).
        assert_eq!(pm.attachments[0].part_id, "2");
    }

    #[test]
    fn p11_convert_garbage_is_none() {
        assert!(parse_raw(b"\x00\x01\x02").is_none());
        // Header-only input parses (empty body), never panics.
        let _ = parse_raw(b"From: x@y.z\r\n\r\n");
    }

    fn leaf_part_ids(raw: &[u8]) -> Vec<String> {
        let tree = to_gmail_tree(raw).expect("tree");
        let mut out = vec![];
        let root = tree.payload.as_ref().expect("payload");
        if let Some(id) = &root.part_id {
            out.push(id.clone());
        }
        if let Some(parts) = &root.parts {
            out.extend(parts.iter().filter_map(|p| p.part_id.clone()));
        }
        out
    }

    #[test]
    fn p27_single_part_attachment_is_addressable_section_one() {
        // Single-part application/pdf with an attachment disposition: the old
        // empty section made it unaddressable (P2.7).
        let raw = b"From: Ada <ada@acme.com>\r\nTo: Ben <ben@globex.io>\r\nSubject: Single\r\nMessage-ID: <s1@acme.com>\r\nContent-Type: application/pdf; name=\"doc.pdf\"\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"doc.pdf\"\r\n\r\nJVBERi0xLjQK\r\n";
        assert_eq!(leaf_part_ids(raw), vec!["1".to_string()]);
        let pm = parse_raw(raw).expect("parse");
        assert_eq!(
            pm.attachments.len(),
            1,
            "appears exactly once: {:?}",
            pm.attachments
                .iter()
                .map(|a| (&a.filename, &a.mime, &a.part_id, a.data.len()))
                .collect::<Vec<_>>()
        );
        assert_eq!(pm.attachments[0].part_id, "1");
        assert_eq!(pm.attachments[0].filename.as_deref(), Some("doc.pdf"));
        assert_eq!(pm.attachments[0].mime, "application/pdf");
        assert_eq!(pm.attachments[0].data, b"%PDF-1.4\n");

        // A single-part text/plain mail still renders as the body.
        let text = b"From: Ada <ada@acme.com>\r\nSubject: Plain\r\nMessage-ID: <s2@acme.com>\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nJust the body.\r\n";
        assert_eq!(leaf_part_ids(text), vec!["1".to_string()]);
        let pm = parse_raw(text).expect("parse");
        assert!(pm.text.unwrap().contains("Just the body"));
        assert!(pm.attachments.is_empty());
    }

    #[test]
    fn p27_root_text_attachment_stays_an_attachment() {
        let raw = b"From: Ada <ada@acme.com>\r\nSubject: Note\r\nMessage-ID: <s3@acme.com>\r\nContent-Type: text/plain; name=\"note.txt\"\r\nContent-Disposition: attachment; filename=\"note.txt\"\r\n\r\nhello note\r\n";
        let pm = parse_raw(raw).expect("parse");
        assert_eq!(
            pm.attachments.len(),
            1,
            "{:?}",
            pm.attachments
                .iter()
                .map(|a| (&a.filename, &a.mime, &a.part_id))
                .collect::<Vec<_>>()
        );
        assert_eq!(pm.attachments[0].part_id, "1");
        assert_eq!(pm.attachments[0].filename.as_deref(), Some("note.txt"));
        assert_eq!(pm.attachments[0].mime, "text/plain");
        // The part's exact bytes, including the trailing line break.
        assert_eq!(pm.attachments[0].data, b"hello note\r\n");
    }

    #[test]
    fn p27_binary_root_with_invalid_utf8_keeps_headers_and_bytes() {
        // Valid headers, then a 8bit binary body that is not valid UTF-8: the
        // old header range bled into the body and lost the headers entirely.
        let mut raw = b"From: Ada <ada@acme.com>\r\nSubject: Bin\r\nMessage-ID: <s4@acme.com>\r\nContent-Type: application/octet-stream; name=\"blob.bin\"\r\nContent-Transfer-Encoding: 8bit\r\nContent-Disposition: attachment; filename=\"blob.bin\"\r\n\r\n".to_vec();
        let payload: Vec<u8> = vec![0x80, 0xff, 0xfe, 0xc3, 0x28, 0x00, 0x1b, b'B', b'I', b'N'];
        raw.extend_from_slice(&payload);
        let pm = parse_raw(&raw).expect("parse");
        assert_eq!(pm.subject, "Bin");
        assert_eq!(
            pm.attachments.len(),
            1,
            "named root binary is an attachment: {:?}",
            pm.attachments
                .iter()
                .map(|a| (&a.filename, &a.mime, &a.part_id))
                .collect::<Vec<_>>()
        );
        assert_eq!(pm.attachments[0].part_id, "1");
        assert_eq!(pm.attachments[0].filename.as_deref(), Some("blob.bin"));
        assert_eq!(pm.attachments[0].mime, "application/octet-stream");
        assert_eq!(pm.attachments[0].data, payload);
    }

    #[test]
    fn p27_unnamed_root_binary_is_still_an_attachment() {
        let mut raw = b"From: Ada <ada@acme.com>\r\nSubject: Raw\r\nMessage-ID: <s5@acme.com>\r\nContent-Type: application/octet-stream\r\nContent-Transfer-Encoding: binary\r\n\r\n".to_vec();
        raw.extend_from_slice(&[0x00, 0x01, 0x80, 0xff]);
        let pm = parse_raw(&raw).expect("parse");
        assert_eq!(pm.attachments.len(), 1);
        assert_eq!(pm.attachments[0].part_id, "1");
        assert_eq!(pm.attachments[0].data, vec![0x00, 0x01, 0x80, 0xff]);
    }

    #[test]
    fn p27_rfc2231_and_quoted_filenames_decode() {
        // RFC 2231 continuation with a charset-encoded first segment.
        let raw = b"From: Ada <ada@acme.com>\r\nSubject: Invoice\r\nMessage-ID: <s6@acme.com>\r\nContent-Type: application/pdf\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment;\r\n filename*0*=utf-8''%E2%82%AC;\r\n filename*1*=%20Rechnung.pdf\r\n\r\nJVBERi0xLjQK\r\n";
        let pm = parse_raw(raw).expect("parse");
        assert_eq!(
            pm.attachments.len(),
            1,
            "{:?}",
            pm.attachments
                .iter()
                .map(|a| (&a.filename, &a.mime, &a.part_id))
                .collect::<Vec<_>>()
        );
        let name = pm.attachments[0].filename.clone().unwrap();
        assert!(name.ends_with("Rechnung.pdf"), "got {name:?}");
        assert!(
            name.contains('€'),
            "RFC2231 charset segment decoded: {name:?}"
        );

        // A filename containing a semicolon and an escaped quote.
        let raw = b"From: Ada <ada@acme.com>\r\nSubject: Odd\r\nMessage-ID: <s7@acme.com>\r\nContent-Type: application/pdf\r\nContent-Transfer-Encoding: base64\r\nContent-Disposition: attachment; filename=\"a;b\\\"c.pdf\"\r\n\r\nJVBERi0xLjQK\r\n";
        let pm = parse_raw(raw).expect("parse");
        assert_eq!(
            pm.attachments.len(),
            1,
            "{:?}",
            pm.attachments
                .iter()
                .map(|a| (&a.filename, &a.mime, &a.part_id))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            pm.attachments[0].filename.as_deref(),
            Some("a;b\"c.pdf"),
            "semicolon and escaped quote are part of the name"
        );
    }

    #[test]
    fn p27_forwarded_eml_is_one_raw_message_attachment() {
        let raw = b"From: Ada <ada@acme.com>\r\nSubject: Fwd\r\nMessage-ID: <s8@acme.com>\r\nContent-Type: multipart/mixed; boundary=\"M\"\r\n\r\n--M\r\nContent-Type: text/plain; charset=utf-8\r\n\r\nouter body\r\n--M\r\nContent-Type: message/rfc822\r\nContent-Transfer-Encoding: 7bit\r\n\r\nFrom: Inner <inner@acme.com>\r\nSubject: Inner\r\nMessage-ID: <i1@acme.com>\r\nTo: Ada <ada@acme.com>\r\n\r\nInner body text\r\n--M--\r\n";
        let pm = parse_raw(raw).expect("parse");
        assert_eq!(pm.attachments.len(), 1, "one forwarded .eml");
        let att = &pm.attachments[0];
        assert_eq!(att.mime, "message/rfc822");
        assert_eq!(att.part_id, "2");
        assert_eq!(att.filename.as_deref(), Some("forwarded-message.eml"));
        let body = String::from_utf8_lossy(&att.data);
        assert!(
            body.contains("Subject: Inner"),
            "raw inner message: {body:?}"
        );
        assert!(body.contains("Inner body text"));
        assert!(!body.contains("outer body"), "not the outer body");
    }
}
