//! IMAP → local-row mapping shared by full and partial sync.
//!
//! One FETCH row becomes one [`MsgUpsert`]; BODYSTRUCTURE becomes attachment
//! rows + snippet section selection; transfer encodings + charsets decode to
//! snippet text. Gmail-specific semantics (labels, hex ids) live here so
//! `full.rs`/`partial.rs` stay orchestration-only.
use super::conn::{Conn, FetchItems};
use super::ids::ImapSection;
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

/// Header fields the metadata FETCH requests; mirrors the REST
/// `metadataHeaders` set so replies/threads/unsubscribe all work.
pub const META_HEADER_FIELDS: &str = "FROM TO CC BCC REPLY-TO SUBJECT DATE MESSAGE-ID IN-REPLY-TO REFERENCES LIST-UNSUBSCRIBE LIST-UNSUBSCRIBE-POST";

/// The metadata FETCH item list (task 6 step 3), assembled by the typed
/// builder so the multi-item command is always parenthesized (P1.1).
pub fn meta_items() -> FetchItems {
    FetchItems::new()
        .uid()
        .flags()
        .internaldate()
        .rfc822_size()
        .modseq()
        .gmail_msgid()
        .gmail_thrid()
        .gmail_labels()
        .bodystructure()
        .raw(format!("BODY.PEEK[HEADER.FIELDS ({META_HEADER_FIELDS})]"))
}

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
        .uid_fetch_items(&uid_set(uids, uids.len()), &meta_items())
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
    let list_unsub_post_value = {
        let raw = get("list-unsubscribe-post");
        if raw.is_empty() {
            None
        } else {
            Some(raw)
        }
    };
    let list_unsub_post = crate::unsubscribe::post_is_one_click(list_unsub_post_value.as_deref());
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
        subject: crate::provider::gmail::mime::decode_rfc2047(&get("subject")),
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
        list_unsubscribe_post_value: list_unsub_post_value,
        list_unsubscribe_post: list_unsub_post,
        // An IMAP message is exactly the bytes the sender transmitted: an
        // `Authentication-Results` field inside it cannot be trusted, so the
        // evidence is recorded but marked untrusted and one-click is refused
        // on this transport.
        auth_results: {
            let r = get("authentication-results");
            if r.is_empty() {
                None
            } else {
                Some(r)
            }
        },
        auth_results_trusted: false,
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
pub fn attachment_rows(account_id: &str, message_hex: &str, bs: &BodyStruct) -> Vec<AttPut> {
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
            // Text without a name or attachment disposition is body content,
            // not an attachment. Everything else addressable is kept: named
            // parts, unnamed binary parts, and an encapsulated message/rfc822
            // (forwarded `.eml`) whose RAW message is section `num` (P2.7).
            if filename.is_none() && is_text && !is_attachment {
                continue;
            }
            let filename = filename.or_else(|| {
                (full_mime == "message/rfc822").then(|| "forwarded-message.eml".to_string())
            });
            let inline = !is_attachment
                && (id.is_some()
                    || disposition
                        .as_ref()
                        .is_some_and(|(k, _)| k.eq_ignore_ascii_case("inline")));
            out.push(AttPut {
                id: format!("{message_hex}:{num}"),
                account_id: account_id.to_string(),
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

/// Strict, chunked MIME transfer decoder (P2.4).
///
/// Unlike [`decode_transfer`] (best-effort snippet decoding, which tolerates
/// a partial escape), this is the attachment path: unsupported or malformed
/// encodings are an error, never the encoded text. State is carried across
/// [`feed`](Self::feed) calls so a base64 quantum or a quoted-printable
/// escape split by a 256 KiB partial read decodes exactly, and the reported
/// total is the DECODED length (never the BODYSTRUCTURE octet count).
#[derive(Debug)]
pub(crate) struct TransferDecoder {
    kind: TransferKind,
    /// Encoded bytes not yet forming a complete unit.
    carry: Vec<u8>,
    /// base64: a padding character has been decoded; no more data may follow.
    padded: bool,
}

#[derive(Debug, Clone, Copy)]
enum TransferKind {
    Passthrough,
    Base64,
    QuotedPrintable,
}

impl TransferDecoder {
    pub(crate) fn new(encoding: &str) -> Result<Self, SiftError> {
        let e = encoding.trim().to_ascii_lowercase();
        let kind = match e.as_str() {
            // Explicitly supported no-op encodings.
            "" | "7bit" | "8bit" | "binary" => TransferKind::Passthrough,
            "base64" => TransferKind::Base64,
            "quoted-printable" => TransferKind::QuotedPrintable,
            // uuencode, x-gzip, … are not decodable: fail instead of handing
            // back encoded bytes as if they were the attachment.
            _ => return Err(decode_failed(&format!("unsupported encoding {e}"))),
        };
        Ok(Self {
            kind,
            carry: Vec::new(),
            padded: false,
        })
    }

    /// Decode a complete payload in one shot (the single-read path).
    pub(crate) fn decode_all(encoding: &str, bytes: &[u8]) -> Result<Vec<u8>, SiftError> {
        let mut d = Self::new(encoding)?;
        let mut out = d.feed(bytes)?;
        out.extend(d.finish()?);
        Ok(out)
    }

    pub(crate) fn feed(&mut self, encoded: &[u8]) -> Result<Vec<u8>, SiftError> {
        match self.kind {
            TransferKind::Passthrough => Ok(encoded.to_vec()),
            TransferKind::Base64 => self.feed_base64(encoded),
            TransferKind::QuotedPrintable => self.feed_qp(encoded),
        }
    }

    /// Flush a trailing incomplete unit; any residue is malformed.
    pub(crate) fn finish(self) -> Result<Vec<u8>, SiftError> {
        match self.kind {
            TransferKind::Passthrough => Ok(Vec::new()),
            TransferKind::Base64 => self.finish_base64(),
            TransferKind::QuotedPrintable => self.finish_qp(),
        }
    }

    fn feed_base64(&mut self, encoded: &[u8]) -> Result<Vec<u8>, SiftError> {
        let mut out = Vec::with_capacity(encoded.len() / 4 * 3 + 3);
        for &b in encoded {
            // Legal MIME base64 may be wrapped at any column: whitespace
            // between quanta is ignored, including a CRLF split across reads.
            if b.is_ascii_whitespace() {
                continue;
            }
            if self.padded {
                return Err(decode_failed("base64 data after padding"));
            }
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'+' | b'/' | b'=' => {
                    self.carry.push(b);
                }
                _ => return Err(decode_failed("base64 contains a non-base64 byte")),
            }
            if self.carry.len() == 4 {
                self.decode_base64_group(&mut out)?;
            }
        }
        Ok(out)
    }

    fn decode_base64_group(&mut self, out: &mut Vec<u8>) -> Result<(), SiftError> {
        use base64::Engine;
        let group = std::mem::take(&mut self.carry);
        let padded = group.contains(&b'=');
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(group)
            .map_err(|_| decode_failed("base64 quantum is malformed"))?;
        out.extend_from_slice(&decoded);
        if padded {
            self.padded = true;
        }
        Ok(())
    }

    fn finish_base64(self) -> Result<Vec<u8>, SiftError> {
        use base64::Engine;
        if self.padded {
            return if self.carry.is_empty() {
                Ok(Vec::new())
            } else {
                Err(decode_failed("base64 data after padding"))
            };
        }
        match self.carry.len() {
            0 => Ok(Vec::new()),
            // A lone trailing character cannot encode a byte.
            1 => Err(decode_failed("truncated base64: 1 trailing character")),
            // A final unpadded quantum: tolerate omitted padding (common in
            // real mail) but still reject non-base64 residue.
            n => {
                debug_assert!(n == 2 || n == 3);
                let mut tail = self.carry;
                while tail.len() % 4 != 0 {
                    tail.push(b'=');
                }
                base64::engine::general_purpose::STANDARD
                    .decode(tail)
                    .map_err(|_| decode_failed("truncated base64 quantum"))
            }
        }
    }

    fn feed_qp(&mut self, encoded: &[u8]) -> Result<Vec<u8>, SiftError> {
        self.carry.extend_from_slice(encoded);
        let (out, consumed) = {
            let b = &self.carry;
            let mut out = Vec::with_capacity(b.len());
            let mut i = 0usize;
            while i < b.len() {
                if b[i] != b'=' {
                    out.push(b[i]);
                    i += 1;
                    continue;
                }
                // `=` starts an escape; wait for the rest of it if needed.
                let Some(&next) = b.get(i + 1) else { break };
                match next {
                    b'\r' => {
                        let Some(&after) = b.get(i + 2) else { break };
                        if after == b'\n' {
                            i += 3; // soft line break
                        } else {
                            return Err(decode_failed("quoted-printable soft break is not CRLF"));
                        }
                    }
                    b'\n' => i += 2,
                    h => {
                        let Some(&l) = b.get(i + 2) else { break };
                        match (hex_val(h), hex_val(l)) {
                            (Some(h), Some(l)) => {
                                out.push((h << 4) | l);
                                i += 3;
                            }
                            _ => {
                                return Err(decode_failed(
                                    "quoted-printable = escape is not two hex digits",
                                ))
                            }
                        }
                    }
                }
            }
            (out, i)
        };
        self.carry.drain(..consumed);
        Ok(out)
    }

    fn finish_qp(self) -> Result<Vec<u8>, SiftError> {
        if self.carry.is_empty() {
            Ok(Vec::new())
        } else {
            // Plain bytes are always consumed; only an incomplete `=` escape
            // (and its `\r`) can remain.
            Err(decode_failed("truncated quoted-printable escape"))
        }
    }
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn decode_failed(detail: &str) -> SiftError {
    super::errors::attachment_decode_failed(detail)
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
///
/// The folder is explicit (P4.3): every partial FETCH runs inside a lease that
/// SELECTs `folder` first, so a snippet read can never execute against a
/// mailbox another task selected in the meantime.
pub(crate) async fn fetch_snippet_groups(
    pool: &super::conn::ImapPool,
    folder: &str,
    account_id: &str,
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
        let Some(section) = ImapSection::parse(section) else {
            log::warn!(
                target: "sift::imap",
                "skipping snippet fetch for invalid section locator"
            );
            return;
        };
        let fetched = {
            let mut w = match pool.with_selected_worker(folder, true, cancel).await {
                Ok(w) => w,
                Err(_) => return,
            };
            // Typed item list (P1.1): the partial section read is one item,
            // and a multi-item FETCH is always parenthesized.
            let items = FetchItems::new()
                .uid()
                .peek_partial(Some(section), 0, Some(2048));
            match w
                .conn()
                .uid_fetch_items(&uid_set(&uids, uids.len()), &items)
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
            // Only a numbered body section may supply snippet bytes: a
            // HEADER.FIELDS literal must never be decoded as body text.
            let bytes = attrs.iter().find_map(|a| match a {
                FetchAttr::BodySection { bytes, .. } => Some(bytes),
                _ => None,
            });
            if let (Some(uid), Some(bytes)) = (uid, bytes) {
                if let Some(t) = by_uid.get(&uid) {
                    let text = decode_snippet(bytes, &t.enc, &t.charset, t.is_html);
                    if !text.is_empty() {
                        let _ = sink
                            .set_snippet(
                                &crate::dto::MessageRef::new(account_id, &t.message_hex),
                                &text,
                            )
                            .await;
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
        let rows = attachment_rows("acc", "abc123", &bs);
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
    fn p27_single_part_root_rows_and_snippet() {
        let pdf = BodyStruct::Single {
            mime: "application".into(),
            subtype: "pdf".into(),
            params: vec![("name".into(), "doc.pdf".into())],
            id: None,
            encoding: "base64".into(),
            size: 9,
            lines: None,
            disposition: Some((
                "attachment".into(),
                vec![("filename".into(), "doc.pdf".into())],
            )),
        };
        let rows = attachment_rows("acc", "abc", &pdf);
        assert_eq!(rows.len(), 1, "single-part attachment must be addressable");
        assert_eq!(rows[0].id, "abc:1");
        assert_eq!(rows[0].part_id, "1");
        assert_eq!(rows[0].filename.as_deref(), Some("doc.pdf"));
        assert!(!rows[0].is_inline);

        // Root text body: no attachment row, snippet section is 1.
        let text = BodyStruct::Single {
            mime: "text".into(),
            subtype: "plain".into(),
            params: vec![("charset".into(), "utf-8".into())],
            id: None,
            encoding: "7bit".into(),
            size: 5,
            lines: Some(1),
            disposition: None,
        };
        assert!(attachment_rows("acc", "abc", &text).is_empty());
        assert_eq!(snippet_section(&text).map(|t| t.0), Some("1".to_string()));

        // Unnamed root binary is kept as an attachment (P2.7).
        let bin = BodyStruct::Single {
            mime: "application".into(),
            subtype: "octet-stream".into(),
            params: vec![],
            id: None,
            encoding: "binary".into(),
            size: 4,
            lines: None,
            disposition: None,
        };
        let rows = attachment_rows("acc", "abc", &bin);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].part_id, "1");
        assert!(rows[0].filename.is_none());

        // Forwarded message/rfc822: one raw-message attachment at section 1.
        let fwd = BodyStruct::Single {
            mime: "message".into(),
            subtype: "rfc822".into(),
            params: vec![],
            id: None,
            encoding: "7bit".into(),
            size: 500,
            lines: None,
            disposition: None,
        };
        let rows = attachment_rows("acc", "abc", &fwd);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].part_id, "1");
        assert_eq!(rows[0].filename.as_deref(), Some("forwarded-message.eml"));
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

    fn err_code(e: &SiftError) -> String {
        serde_json::to_value(e).unwrap()["code"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn b64(bytes: &[u8]) -> Vec<u8> {
        use base64::Engine;
        base64::engine::general_purpose::STANDARD
            .encode(bytes)
            .into_bytes()
    }

    #[test]
    fn p24_streaming_decoder_base64_is_chunk_boundary_safe() {
        let payload: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
        let encoded = b64(&payload);
        // Split at every awkward position: inside a quantum, between the two
        // padding characters, and at a CRLF.
        for split in [1, 2, 3, 4, 5, 7, 100, 1023, 2048, encoded.len() - 1] {
            let mut d = TransferDecoder::new("base64").unwrap();
            let mut out = d.feed(&encoded[..split]).unwrap();
            out.extend(d.feed(&encoded[split..]).unwrap());
            out.extend(d.finish().unwrap());
            assert_eq!(out, payload, "split at {split}");
        }
        // Wrapped at 76 columns with CRLF, decoded as one feed.
        let mut wrapped = Vec::new();
        for chunk in encoded.chunks(76) {
            wrapped.extend_from_slice(chunk);
            wrapped.extend_from_slice(b"\r\n");
        }
        assert_eq!(
            TransferDecoder::decode_all("base64", &wrapped).unwrap(),
            payload
        );
        // CRLF split across two feeds.
        let mut d = TransferDecoder::new("base64").unwrap();
        let mut out = d.feed(&encoded[..40]).unwrap();
        out.extend(d.feed(b"\r").unwrap());
        out.extend(d.feed(b"\n").unwrap());
        out.extend(d.feed(&encoded[40..]).unwrap());
        out.extend(d.finish().unwrap());
        assert_eq!(out, payload);
        // 0 and 1 bytes.
        assert!(TransferDecoder::decode_all("base64", b"")
            .unwrap()
            .is_empty());
        assert!(TransferDecoder::decode_all("base64", b"\r\n")
            .unwrap()
            .is_empty());
        // Omitted final padding still decodes ("aGk" -> "hi").
        assert_eq!(
            TransferDecoder::decode_all("base64", b"aGk").unwrap(),
            b"hi"
        );
        assert_eq!(
            TransferDecoder::decode_all("base64", b"aGVsbG8=").unwrap(),
            b"hello"
        );
        // A 1-char residue after a padded quantum is rejected.
        let e = TransferDecoder::decode_all("base64", b"aGVsbG8=A").unwrap_err();
        assert_eq!(err_code(&e), "attachment_decode_failed");
        // Invalid characters never decode to encoded text.
        for bad in [&b"aGVs*bG8="[..], &b"a==="[..], &b"aGVsbG8=\xff"[..]] {
            let e = TransferDecoder::decode_all("base64", bad).unwrap_err();
            assert_eq!(err_code(&e), "attachment_decode_failed", "input {bad:?}");
        }
    }

    #[test]
    fn p24_streaming_decoder_quoted_printable_is_chunk_boundary_safe() {
        let encoded = b"Hello=20world=0A=\r\ncontinued =3D sign";
        let want = b"Hello world\ncontinued = sign";
        for split in 1..encoded.len() {
            let mut d = TransferDecoder::new("quoted-printable").unwrap();
            let mut out = d.feed(&encoded[..split]).unwrap();
            out.extend(d.feed(&encoded[split..]).unwrap());
            out.extend(d.finish().unwrap());
            assert_eq!(out, want, "split at {split}");
        }
        // Bare-LF soft break is tolerated; truncated escape is not.
        assert_eq!(
            TransferDecoder::decode_all("quoted-printable", b"a=\nb").unwrap(),
            b"ab"
        );
        for bad in [&b"a=Zb"[..], &b"abc="[..], &b"abc=\r"[..]] {
            let e = TransferDecoder::decode_all("quoted-printable", bad).unwrap_err();
            assert_eq!(err_code(&e), "attachment_decode_failed", "input {bad:?}");
        }
    }

    #[test]
    fn p24_streaming_decoder_encodings_and_unsupported() {
        // Explicitly supported no-op encodings.
        for enc in ["", "7bit", "8bit", "binary", "BASE64 "] {
            assert!(TransferDecoder::new(enc).is_ok(), "{enc}");
        }
        assert_eq!(
            TransferDecoder::decode_all("binary", &[0x80, 0xff, 0x00]).unwrap(),
            vec![0x80, 0xff, 0x00]
        );
        // Unsupported transfer encodings are rejected, never passed through.
        for enc in ["x-uuencode", "uuencode", "gzip", "base64x"] {
            let e = TransferDecoder::new(enc).unwrap_err();
            assert_eq!(err_code(&e), "attachment_decode_failed", "{enc}");
        }
    }
}
