//! IMAP → local-row mapping shared by full and partial sync.
//!
//! One FETCH row becomes one [`MsgUpsert`]; BODYSTRUCTURE becomes attachment
//! rows + snippet section selection; transfer encodings + charsets decode to
//! snippet text. Gmail-specific semantics (labels, hex ids) live here so
//! `full.rs`/`partial.rs` stay orchestration-only.
use super::conn::Conn;
use super::proto::{decode_utf7_mailbox, walk_parts, BodyStruct, FetchAttr, GmailMeta};
use crate::db::{attachments::AttPut, messages::MsgUpsert};
use crate::errors::SiftError;
use std::collections::HashMap;

/// One parsed metadata row: sequence number, IMAP data, header block.
#[derive(Clone)]
pub struct MetaItem {
    pub seq: u32,
    pub uid: u32,
    pub flags: Vec<String>,
    pub internaldate: String,
    pub size: u32,
    pub meta: GmailMeta,
    pub structure: Option<BodyStruct>,
    pub headers: HashMap<String, String>,
}

/// The metadata FETCH item list (task 6 step 3). Header fields mirror the
/// REST `metadataHeaders` set so replies/threads/unsubscribe all work.
pub const META_ITEMS: &str = "(UID FLAGS INTERNALDATE RFC822.SIZE MODSEQ X-GM-MSGID X-GM-THRID X-GM-LABELS BODYSTRUCTURE BODY.PEEK[HEADER.FIELDS (FROM TO CC BCC REPLY-TO SUBJECT DATE MESSAGE-ID IN-REPLY-TO REFERENCES LIST-UNSUBSCRIBE LIST-UNSUBSCRIBE-POST)])";

/// UID set string, newest-first, compressed ranges, max `max` entries.
pub fn uid_set(uids: &[u32], max: usize) -> String {
    let mut sorted: Vec<u32> = uids.to_vec();
    sorted.sort_unstable_by(|a, b| b.cmp(a));
    sorted.truncate(max);
    // ascending runs compress best; emit ranges then reverse for newest-first.
    sorted.sort_unstable();
    let mut runs: Vec<String> = vec![];
    let mut i = 0;
    while i < sorted.len() {
        let mut j = i;
        while j + 1 < sorted.len() && sorted[j + 1] == sorted[j] + 1 {
            j += 1;
        }
        if j > i {
            runs.push(format!("{}:{}", sorted[i], sorted[j]));
        } else {
            runs.push(format!("{}", sorted[i]));
        }
        i = j + 1;
    }
    runs.reverse();
    runs.join(",")
}

pub async fn fetch_meta(conn: &mut Conn, uids: &[u32]) -> Result<Vec<MetaItem>, SiftError> {
    if uids.is_empty() {
        return Ok(vec![]);
    }
    let fetched = conn
        .uid_fetch(&uid_set(uids, uids.len()), META_ITEMS)
        .await?;
    let mut out = vec![];
    for (seq, attrs) in fetched {
        let mut item = MetaItem {
            seq,
            uid: 0,
            flags: vec![],
            internaldate: String::new(),
            size: 0,
            meta: GmailMeta::default(),
            structure: None,
            headers: HashMap::new(),
        };
        for a in &attrs {
            match a {
                FetchAttr::Uid(u) => item.uid = *u,
                FetchAttr::Flags(f) => item.flags = f.clone(),
                FetchAttr::InternalDate(d) => item.internaldate = d.clone(),
                FetchAttr::Rfc822Size(s) => item.size = *s,
                FetchAttr::HeaderFields(b) => item.headers = super::proto::parse_header_fields(b),
                FetchAttr::BodyStructure(bs) => item.structure = Some(bs.clone()),
                _ => {}
            }
        }
        item.meta = super::proto::gmail_meta(&attrs);
        if item.uid != 0 {
            out.push(item);
        }
    }
    Ok(out)
}

/// Map Gmail labels + IMAP flags to local label ids and derived flags.
/// System `\`-labels become REST ids (sidebar/queries unchanged); anything
/// else passes through verbatim (UTF-8 preserved).
pub fn map_labels(gm_labels: &[String], flags: &[String]) -> (Vec<String>, bool, bool, bool, bool) {
    let mut ids: Vec<String> = vec![];
    let mut push = |id: &str| {
        if !ids.iter().any(|x| x == id) {
            ids.push(id.to_string());
        }
    };
    for l in gm_labels {
        if let Some(rest) = l.strip_prefix('\\') {
            match rest.to_ascii_uppercase().as_str() {
                "INBOX" => push("INBOX"),
                "SENT" => push("SENT"),
                "DRAFT" => push("DRAFT"),
                "STARRED" => push("STARRED"),
                "IMPORTANT" => push("IMPORTANT"),
                "TRASH" => push("TRASH"),
                "SPAM" => push("SPAM"),
                // \Seen \Flagged \Deleted never arrive here (they're FLAGS),
                // but tolerate them silently instead of creating junk labels.
                "SEEN" | "FLAGGED" | "DELETED" | "RECENT" => {}
                other => push(other),
            }
        } else {
            push(l);
        }
    }
    let flagged = flags.iter().any(|f| f.eq_ignore_ascii_case("\\Flagged"));
    if flagged {
        push("STARRED");
    }
    let unread = !flags.iter().any(|f| f.eq_ignore_ascii_case("\\Seen"));
    if unread {
        push("UNREAD");
    }
    let starred = flagged || ids.iter().any(|x| x == "STARRED");
    let draft = ids.iter().any(|x| x == "DRAFT");
    let sent = ids.iter().any(|x| x == "SENT");
    (ids, unread, starred, draft, sent)
}

pub fn meta_to_upsert(
    account_id: &str,
    item: &MetaItem,
    snippet: String,
    extra_labels: &[String],
    is_draft_only_hint: bool,
) -> Option<MsgUpsert> {
    let msgid = item.meta.msgid?;
    let thrid = item.meta.thrid?;
    let (mut label_ids, unread, starred, draft, sent) = map_labels(&item.meta.labels, &item.flags);
    for e in extra_labels {
        if !label_ids.iter().any(|x| x == e) {
            label_ids.push(e.clone());
        }
    }
    let get = |n: &str| item.headers.get(n).cloned().unwrap_or_default();
    let from_raw = get("from");
    let parsed = crate::provider::gmail::mime::parse_addrs(&from_raw);
    let _ = is_draft_only_hint;
    Some(MsgUpsert {
        id: format!("{msgid:x}"),
        account_id: account_id.into(),
        thread_id: format!("{thrid:x}"),
        history_id: None,
        internal_date: super::proto::parse_internaldate(&item.internaldate).unwrap_or(0),
        from_name: parsed.first().and_then(|(n, _)| n.clone()),
        from_email: parsed.first().map(|(_, e)| e.clone()),
        to_json: serde_json::to_string(&parsed).unwrap_or("[]".into()),
        cc_json: "[]".into(),
        bcc_json: "[]".into(),
        reply_to: {
            let r = get("reply-to");
            if r.is_empty() {
                None
            } else {
                Some(r)
            }
        },
        subject: get("subject"),
        snippet,
        rfc_message_id: {
            let r = get("message-id");
            if r.is_empty() {
                None
            } else {
                Some(r)
            }
        },
        in_reply_to: {
            let r = get("in-reply-to");
            if r.is_empty() {
                None
            } else {
                Some(r)
            }
        },
        references_json: "[]".into(),
        list_unsubscribe: {
            let r = get("list-unsubscribe");
            if r.is_empty() {
                None
            } else {
                Some(r)
            }
        },
        list_unsubscribe_post: get("list-unsubscribe-post").contains("One-Click"),
        size_estimate: Some(item.size as i64),
        has_attachments: false,
        is_unread: unread,
        is_starred: starred,
        is_draft: draft,
        is_sent_by_me: sent,
        label_ids,
    })
}

/// Attachment rows from a BODYSTRUCTURE: filename/disposition parts plus
/// non-text leaf parts. `attachments.id` is `<hex-msgid>:<section>` (task 6).
/// Inline parts keep their Content-ID (brackets stripped at serve time).
pub fn attachment_rows(message_hex: &str, bs: &BodyStruct) -> Vec<AttPut> {
    let mut out = vec![];
    for (num, part) in walk_parts(bs) {
        if num.is_empty() {
            continue;
        }
        if let BodyStruct::Single {
            mime,
            subtype,
            params,
            id,
            encoding: _,
            size,
            disposition,
            ..
        } = part
        {
            let filename = disposition
                .as_ref()
                .and_then(|(_, p)| {
                    p.iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("filename"))
                        .map(|(_, v)| v.clone())
                })
                .or_else(|| {
                    params
                        .iter()
                        .find(|(k, _)| k.eq_ignore_ascii_case("name"))
                        .map(|(_, v)| v.clone())
                });
            let is_attachment = disposition
                .as_ref()
                .is_some_and(|(k, _)| k.eq_ignore_ascii_case("attachment"));
            let full_mime = format!("{mime}/{subtype}").to_lowercase();
            let is_text = full_mime.starts_with("text/");
            if filename.is_none() && (is_text || full_mime == "message/rfc822") && !is_attachment {
                continue;
            }
            let inline = !is_attachment
                && (id.is_some()
                    || disposition
                        .as_ref()
                        .is_some_and(|(k, _)| k.eq_ignore_ascii_case("inline")));
            out.push(AttPut {
                id: format!("{message_hex}:{num}"),
                message_id: message_hex.into(),
                gmail_att_id: None,
                part_id: num,
                filename,
                mime: full_mime,
                size: *size as i64,
                content_id: id
                    .clone()
                    .map(|c| c.trim_matches(|ch| ch == '<' || ch == '>').to_string()),
                is_inline: inline,
                data: None,
            });
        }
    }
    out
}

/// Snippet section: first text/plain part, else first text/html.
pub fn snippet_section(bs: &BodyStruct) -> Option<(String, String, String)> {
    let mut plain: Option<(String, String, String)> = None;
    let mut html: Option<(String, String, String)> = None;
    for (num, part) in walk_parts(bs) {
        if num.is_empty() {
            continue;
        }
        if let BodyStruct::Single {
            mime,
            subtype,
            params,
            encoding,
            ..
        } = part
        {
            if !mime.eq_ignore_ascii_case("text") {
                continue;
            }
            let charset = params
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case("charset"))
                .map(|(_, v)| v.clone())
                .unwrap_or_else(|| "utf-8".into());
            let entry = (num.clone(), encoding.clone(), charset);
            if subtype.eq_ignore_ascii_case("plain") && plain.is_none() {
                plain = Some(entry);
            } else if subtype.eq_ignore_ascii_case("html") && html.is_none() {
                html = Some(entry);
            }
        }
    }
    plain.or(html)
}

/// Decode one partial body: transfer encoding, then charset, then snippet.
/// 200 chars, whitespace-collapsed (task 6 step 4).
/// Raw transfer decoding (base64 / quoted-printable / 7bit), no charset.
pub(crate) fn decode_transfer(bytes: &[u8], encoding: &str) -> Vec<u8> {
    match encoding.to_ascii_lowercase().as_str() {
        "base64" => base64_decode_clean(bytes),
        "quoted-printable" => qp_decode(bytes),
        _ => bytes.to_vec(),
    }
}

pub fn decode_snippet(bytes: &[u8], encoding: &str, charset: &str, is_html: bool) -> String {
    let raw: Vec<u8> = decode_transfer(bytes, encoding);
    let text = decode_charset(&raw, charset);
    let text = if is_html {
        crate::render::text::html_to_text(&text)
    } else {
        text
    };
    snippet_of_text(&text)
}

/// Collapse whitespace, keep 200 chars. Shared with snippet backfill.
pub fn snippet_of_text(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect()
}

fn base64_decode_clean(b: &[u8]) -> Vec<u8> {
    use base64::Engine;
    let clean: Vec<u8> = b
        .iter()
        .copied()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    base64::engine::general_purpose::STANDARD
        .decode(&clean)
        .unwrap_or_default()
}

fn qp_decode(b: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'=' {
            // soft breaks: =\r\n and =\n vanish
            if b.get(i + 1) == Some(&b'\r') && b.get(i + 2) == Some(&b'\n') {
                i += 3;
                continue;
            }
            if b.get(i + 1) == Some(&b'\n') {
                i += 2;
                continue;
            }
            if let (Some(&h), Some(&l)) = (b.get(i + 1), b.get(i + 2)) {
                if let (Some(h), Some(l)) = ((h as char).to_digit(16), (l as char).to_digit(16)) {
                    out.push((h * 16 + l) as u8);
                    i += 3;
                    continue;
                }
            }
            out.push(b'=');
            i += 1;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    out
}

fn decode_charset(raw: &[u8], charset: &str) -> String {
    let label = charset.trim().to_lowercase();
    // encoding_rs handles UTF-8, latin-1, windows-1252, ISO-2022-JP, etc.
    let (s, _) = encoding_rs::Encoding::for_label(label.as_bytes())
        .unwrap_or(encoding_rs::UTF_8)
        .decode_without_bom_handling(raw);
    s.into_owned()
}

/// Folder display name decode helper (LIST names are modified UTF-7).
pub fn decode_folder_name(raw: &str) -> String {
    decode_utf7_mailbox(raw)
}

pub(crate) struct SnipTarget {
    pub(crate) uid: u32,
    pub(crate) message_hex: String,
    pub(crate) section: String,
    pub(crate) enc: String,
    pub(crate) charset: String,
    pub(crate) is_html: bool,
}

pub(crate) fn section_is_html(bs: &BodyStruct, section: &str) -> bool {
    walk_parts(bs)
        .into_iter()
        .find(|(n, _)| n == section)
        .and_then(|(_, p)| match p {
            BodyStruct::Single { mime, subtype, .. } => {
                Some(mime.eq_ignore_ascii_case("text") && subtype.eq_ignore_ascii_case("html"))
            }
            _ => None,
        })
        .unwrap_or(false)
}

/// Snippet pass: group targets by (section, encoding) so messages sharing a
/// section shape need one partial FETCH each.
pub(crate) async fn fetch_snippet_groups(
    pool: &super::conn::ImapPool,
    sink: &dyn super::super::SyncSink,
    targets: &[SnipTarget],
    cancel: &tokio_util::sync::CancellationToken,
) {
    let mut groups: HashMap<(String, String), Vec<&SnipTarget>> = HashMap::new();
    for t in targets {
        groups
            .entry((t.section.clone(), t.enc.clone()))
            .or_default()
            .push(t);
    }
    for ((section, _enc), members) in &groups {
        if cancel.is_cancelled() {
            return;
        }
        let uids: Vec<u32> = members.iter().map(|m| m.uid).collect();
        let fetched = {
            let mut guard = match pool.worker().await {
                Ok(g) => g,
                Err(_) => return,
            };
            let conn = match guard.as_mut() {
                Some(c) => c,
                None => return,
            };
            match conn
                .uid_fetch(
                    &uid_set(&uids, uids.len()),
                    &format!("UID BODY.PEEK[{section}]<0.2048>"),
                )
                .await
            {
                Ok(f) => f,
                Err(_) => return,
            }
        };
        let by_uid: HashMap<u32, &SnipTarget> = members.iter().map(|m| (m.uid, *m)).collect();
        for (_seq, attrs) in &fetched {
            let uid = attrs.iter().find_map(|a| match a {
                FetchAttr::Uid(u) => Some(*u),
                _ => None,
            });
            let bytes = attrs.iter().find_map(|a| match a {
                FetchAttr::BodySection { bytes, .. } => Some(bytes),
                FetchAttr::HeaderFields(b) => Some(b),
                _ => None,
            });
            if let (Some(uid), Some(bytes)) = (uid, bytes) {
                if let Some(t) = by_uid.get(&uid) {
                    let text = decode_snippet(bytes, &t.enc, &t.charset, t.is_html);
                    if !text.is_empty() {
                        let _ = sink.set_snippet(&t.message_hex, &text).await;
                    }
                }
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::imap::proto::{parse_transcript, Response, Untagged};

    fn meta_fixture() -> Vec<MetaItem> {
        // parsed via a scratch Conn-less path: reuse proto directly
        let bytes = std::fs::read(format!(
            "{}/../fixtures/imap/fetch-meta.txt",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let mut out = vec![];
        for r in parse_transcript(&bytes).unwrap() {
            if let Response::Untagged(Untagged::Fetch { seq, attrs }) = r {
                // emulate fetch_meta mapping for header-carrying items
                let mut item = MetaItem {
                    seq,
                    uid: 0,
                    flags: vec![],
                    internaldate: String::new(),
                    size: 0,
                    meta: GmailMeta::default(),
                    structure: None,
                    headers: HashMap::new(),
                };
                for a in &attrs {
                    match a {
                        FetchAttr::Uid(u) => item.uid = *u,
                        FetchAttr::Flags(f) => item.flags = f.clone(),
                        FetchAttr::InternalDate(d) => item.internaldate = d.clone(),
                        FetchAttr::Rfc822Size(s) => item.size = *s,
                        FetchAttr::HeaderFields(b) => {
                            item.headers = super::super::proto::parse_header_fields(b)
                        }
                        FetchAttr::BodyStructure(bs) => item.structure = Some(bs.clone()),
                        _ => {}
                    }
                }
                item.meta = super::super::proto::gmail_meta(&attrs);
                out.push(item);
            }
        }
        out
    }

    #[test]
    fn p11_message_meta_mapping() {
        let items = meta_fixture();
        assert_eq!(items.len(), 2);
        let up = meta_to_upsert("acc", &items[0], "snippet here".into(), &[], false).unwrap();
        assert_eq!(up.id, "1a8a04434ea94b87");
        assert_eq!(up.thread_id, "1a8a04434ea94b87");
        assert_eq!(up.from_email.as_deref(), Some("ada@acme.com"));
        assert_eq!(up.subject, "Launch checklist");
        assert!(!up.is_unread); // FLAGS (\Seen) in the fixture
        assert!(!up.is_starred);
        assert!(up.label_ids.contains(&"INBOX".to_string()));
        let up2 =
            meta_to_upsert("acc", &items[1], String::new(), &["TRASH".into()], false).unwrap();
        assert!(up2.is_starred);
        assert!(up2.label_ids.contains(&"TRASH".to_string()));
        // missing ids -> skipped, never a ghost row
        let mut no_id = items[0].clone();
        no_id.meta.msgid = None;
        assert!(meta_to_upsert("acc", &no_id, String::new(), &[], false).is_none());
    }

    // MetaItem needs Clone for the ghost-row check above.
    #[test]
    fn p11_message_attachments_and_snippet_section() {
        let items = meta_fixture();
        let bs = items[1].structure.clone().expect("structure");
        let rows = attachment_rows("abc123", &bs);
        // pdf + inline png; the html part itself is not an attachment
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "abc123:2");
        assert_eq!(rows[0].filename.as_deref(), Some("contract.pdf"));
        assert!(!rows[0].is_inline);
        assert_eq!(rows[1].content_id.as_deref(), Some("ii_row14"));
        assert!(rows[1].is_inline);
        let sec = snippet_section(&bs).expect("section");
        assert_eq!(sec.0, "1");
        assert!(sec.1.contains("quoted-printable") || !sec.1.is_empty());
    }

    #[test]
    fn p11_message_labels_and_decoders() {
        let (ids, unread, starred, draft, sent) = map_labels(
            &["\\Inbox".to_string(), "Clients/Acme".to_string()],
            &["\\Seen".to_string()],
        );
        assert!(!unread);
        assert!(!starred);
        assert!(!draft && !sent);
        assert!(ids.contains(&"INBOX".to_string()));
        let (ids, unread, starred, _, _) =
            map_labels(&["Büro".to_string()], &["\\Flagged".to_string()]);
        assert!(unread && starred);
        assert!(ids.contains(&"Büro".to_string()));
        assert!(ids.contains(&"STARRED".to_string()));
        // decoders
        assert_eq!(
            decode_snippet(b"Hello =\r\nworld", "quoted-printable", "utf-8", false),
            "Hello world"
        );
        assert_eq!(
            decode_snippet("aGVsbG8=".as_bytes(), "base64", "utf-8", false),
            "hello"
        );
        assert_eq!(
            decode_snippet("<p>Hi <b>there</b></p>".as_bytes(), "7bit", "utf-8", true),
            "Hi there"
        );
        assert_eq!(
            decode_folder_name("[Gmail]/Entw&APw-rfe"),
            "[Gmail]/Entwürfe"
        );
        assert_eq!(uid_set(&[1, 2, 3, 7, 8, 9, 4522], 100), "4522,7:9,1:3");
    }
}
