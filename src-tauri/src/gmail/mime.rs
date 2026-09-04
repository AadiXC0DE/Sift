use crate::gmail::types::{Message, MessagePart};
use base64::{engine::general_purpose::URL_SAFE, Engine};

#[derive(Debug, Clone, Default)]
pub struct ParsedPart {
    pub part_id: String,
    pub filename: Option<String>,
    pub mime: String,
    pub size: i64,
    pub content_id: Option<String>,
    pub is_inline: bool,
    pub data: Vec<u8>,
    pub attachment_id: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ParsedMessage {
    pub headers: std::collections::HashMap<String, String>,
    pub html: Option<String>,
    pub text: Option<String>,
    pub attachments: Vec<ParsedPart>,
    pub inline: Vec<ParsedPart>,
    pub quoted_from: Option<usize>,
    pub from_name: Option<String>,
    pub from_email: Option<String>,
    pub to: Vec<(Option<String>, String)>,
    pub cc: Vec<(Option<String>, String)>,
    pub subject: String,
    pub rfc_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Vec<String>,
    pub list_unsub: Option<String>,
    pub list_unsub_post: bool,
}

fn b64url(s: &str) -> Vec<u8> {
    URL_SAFE
        .decode(s.replace('-', "+").replace('_', "/"))
        .or_else(|_| URL_SAFE.decode(s))
        .unwrap_or_default()
}

#[allow(dead_code)] // reserved header accessor for future payload paths
fn header(p: &MessagePart, name: &str) -> Option<String> {
    p.headers
        .as_ref()?
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.clone())
}

fn walk<'a>(p: &'a MessagePart, out: &mut Vec<&'a MessagePart>) {
    out.push(p);
    if let Some(subs) = &p.parts {
        for s in subs {
            walk(s, out);
        }
    }
}

fn decode_rfc2047(s: &str) -> String {
    // Minimal RFC2047 B/Q decoder (avoids mail-parser API churn)
    let out = s.to_string();
    // =?charset?B?data?=
    let mut res = String::new();
    let mut rest = s;
    let mut found = false;
    while let Some(a) = rest.find("=?") {
        if let Some(b) = rest[a..].find("?=") {
            let token = &rest[a..a + b + 2];
            let inner = &token[2..token.len() - 2];
            let parts: Vec<&str> = inner.split('?').collect();
            if parts.len() == 3 {
                let (enc, data) = (parts[1].to_uppercase(), parts[2]);
                let decoded = if enc == "B" {
                    use base64::Engine;
                    base64::engine::general_purpose::STANDARD
                        .decode(data)
                        .map(|v| String::from_utf8_lossy(&v).into_owned())
                        .unwrap_or_else(|_| data.to_string())
                } else {
                    // Q-encoding: _ => space, =XX hex
                    let q = data.replace('_', " ");
                    // simple percent-like decode
                    let s2 = q.clone();
                    let mut out2 = String::new();
                    let mut chars = s2.chars().peekable();
                    while let Some(ch) = chars.next() {
                        if ch == '=' {
                            let h1 = chars.next().unwrap_or('0');
                            let h2 = chars.next().unwrap_or('0');
                            if let Ok(b) = u8::from_str_radix(&format!("{h1}{h2}"), 16) {
                                out2.push(b as char);
                            } else {
                                out2.push(ch);
                                out2.push(h1);
                                out2.push(h2);
                            }
                        } else {
                            out2.push(ch);
                        }
                    }
                    out2
                };
                res.push_str(&rest[..a]);
                res.push_str(&decoded);
                rest = &rest[a + b + 2..];
                found = true;
                continue;
            }
        }
        break;
    }
    if found {
        res.push_str(rest);
        res
    } else {
        out
    }
}

pub fn parse_addrs(raw: &str) -> Vec<(Option<String>, String)> {
    // Handle "Name" <a@b>, c@d
    let mut out: Vec<(Option<String>, String)> = vec![];
    for part in split_addrs(raw) {
        let part = part.trim().to_string();
        if part.is_empty() {
            continue;
        }
        if let Some((name, email)) = parse_one(&part) {
            out.push((name, email));
        }
    }
    out
}

fn split_addrs(s: &str) -> Vec<String> {
    let mut parts: Vec<String> = vec![];
    let mut cur = String::new();
    let mut in_q = false;
    let mut depth = 0;
    for ch in s.chars() {
        match ch {
            '"' => {
                in_q = !in_q;
                cur.push(ch);
            }
            '<' if !in_q => {
                depth += 1;
                cur.push(ch);
            }
            '>' if !in_q => {
                depth -= 1;
                cur.push(ch);
            }
            ',' if !in_q && depth == 0 => {
                parts.push(cur.clone());
                cur.clear();
            }
            _ => cur.push(ch),
        }
    }
    if !cur.trim().is_empty() {
        parts.push(cur);
    }
    parts
}

fn parse_one(s: &str) -> Option<(Option<String>, String)> {
    if let Some(a) = s.find('<') {
        if let Some(b) = s.find('>') {
            let email = s[a + 1..b].trim().to_string();
            let name = s[..a].trim().trim_matches('"').trim().to_string();
            let name = if name.is_empty() {
                None
            } else {
                Some(decode_rfc2047(&name))
            };
            return Some((name, email));
        }
    }
    let t = s.trim().trim_matches(|c| c == '"' || c == '\'').to_string();
    if t.contains('@') {
        return Some((None, decode_rfc2047(&t)));
    }
    None
}

pub fn parse_full(msg: &Message) -> ParsedMessage {
    let mut pm = ParsedMessage::default();
    let empty = MessagePart {
        part_id: None,
        mime_type: None,
        filename: None,
        headers: None,
        body: None,
        parts: None,
    };
    let root = msg.payload.as_ref().unwrap_or(&empty);
    let mut all = vec![];
    walk(root, &mut all);
    // headers from root
    for h in root.headers.clone().unwrap_or_default() {
        pm.headers.insert(h.name.to_lowercase(), h.value.clone());
    }
    let get = |n: &str| {
        pm.headers
            .get(&n.to_lowercase())
            .cloned()
            .unwrap_or_default()
    };
    pm.subject = decode_rfc2047(&get("subject"));
    pm.in_reply_to = pm.headers.get("in-reply-to").cloned();
    pm.rfc_id = pm.headers.get("message-id").cloned();
    if let Some(r) = pm.headers.get("references") {
        pm.references = r.split_whitespace().map(|s| s.to_string()).collect();
    }
    pm.list_unsub = pm.headers.get("list-unsubscribe").cloned();
    pm.list_unsub_post = pm
        .headers
        .get("list-unsubscribe-post")
        .map(|s| s.contains("List-Unsubscribe=One-Click"))
        .unwrap_or(false);
    let from_raw = get("from");
    let f = parse_addrs(&from_raw);
    pm.from_name = f.first().and_then(|(n, _)| n.clone());
    pm.from_email = f.first().map(|(_, e)| e.clone());
    pm.to = parse_addrs(&get("to"));
    pm.cc = parse_addrs(&get("cc"));

    // bodies: prefer html under alternative
    let mut html: Option<(String, usize)> = None;
    let mut text: Option<(String, usize)> = None;
    for p in &all {
        let mime = p.mime_type.as_deref().unwrap_or("").to_lowercase();
        let disp_inline = p.filename.as_deref().map(|f| f.is_empty()).unwrap_or(true);
        if (mime == "text/html" || mime == "text/plain") && disp_inline {
            if let Some(b) = &p.body {
                if let Some(d) = &b.data {
                    let bytes = b64url(d);
                    let s = String::from_utf8_lossy(&bytes).into_owned();
                    let depth = 0;
                    if mime == "text/html" && html.is_none() {
                        html = Some((s.clone(), depth));
                    }
                    if mime == "text/plain" && text.is_none() {
                        text = Some((s, depth));
                    }
                }
            }
        }
    }
    // attachments: filename non-empty OR (has attachmentId and not text part)
    for p in &all {
        let mime = p
            .mime_type
            .as_deref()
            .unwrap_or("application/octet-stream")
            .to_string();
        let filename = p.filename.clone().filter(|f| !f.is_empty());
        let cid = p
            .headers
            .as_ref()
            .and_then(|hs| {
                hs.iter()
                    .find(|h| h.name.eq_ignore_ascii_case("Content-ID"))
            })
            .map(|h| {
                h.value
                    .trim()
                    .trim_matches(|c| c == '<' || c == '>')
                    .to_string()
            });
        let disp: String = p
            .headers
            .as_ref()
            .and_then(|hs| {
                hs.iter()
                    .find(|h| h.name.eq_ignore_ascii_case("Content-Disposition"))
            })
            .map(|h| h.value.to_lowercase())
            .unwrap_or_default();
        let is_att = filename.is_some()
            || disp.contains("attachment")
            || (p
                .body
                .as_ref()
                .and_then(|b| b.attachment_id.clone())
                .is_some()
                && !mime.starts_with("text/"));
        let is_inline = cid.is_some() || disp.contains("inline");
        if is_att || (is_inline && filename.is_some()) {
            let data = p
                .body
                .as_ref()
                .and_then(|b| b.data.as_deref())
                .map(b64url)
                .unwrap_or_default();
            let pp = ParsedPart {
                part_id: p.part_id.clone().unwrap_or_default(),
                filename,
                mime,
                size: p
                    .body
                    .as_ref()
                    .and_then(|b| b.size)
                    .unwrap_or(data.len() as i64),
                content_id: cid,
                is_inline,
                data,
                attachment_id: p.body.as_ref().and_then(|b| b.attachment_id.clone()),
            };
            if is_inline {
                pm.inline.push(pp);
            } else {
                pm.attachments.push(pp);
            }
        }
    }
    if let Some((h, _)) = html {
        pm.html = Some(h);
    }
    if let Some((t, _)) = text {
        pm.text = Some(t.clone());
        pm.quoted_from = find_quote(&t);
    } else if let Some(h) = &pm.html {
        // extract text-ish for quote detection
        let stripped = strip_tags(h);
        pm.quoted_from = find_quote(&stripped);
    }
    pm
}

fn find_quote(t: &str) -> Option<usize> {
    for (i, line) in t.lines().enumerate() {
        let l = line.trim();
        if l.starts_with("On ") && l.ends_with("wrote:") {
            return Some(i);
        }
        if l == "-----Original Message-----" {
            return Some(i);
        }
        if l.starts_with("From: ")
            && t.lines()
                .nth(i + 1)
                .map(|n| n.starts_with("Sent: ") || n.starts_with("Date: "))
                .unwrap_or(false)
        {
            return Some(i);
        }
    }
    None
}

fn strip_tags(h: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in h.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn p3_t06_rfc2047_and_commas() {
        let addrs = parse_addrs("\"Doe, John\" <john@x.com>, =?UTF-8?B?QWTDpQ==?= <ada@x.com>");
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs[0].1, "john@x.com");
        assert_eq!(addrs[0].0.as_deref(), Some("Doe, John"));
    }
}
