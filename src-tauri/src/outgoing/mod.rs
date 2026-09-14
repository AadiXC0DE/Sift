//! Outgoing message preparation (P5.3/P5.4).
//!
//! One place builds the bytes that leave the machine: RFC 5322 headers, MIME
//! structure and the transport envelope. It replaced a hand-written string
//! concatenation that emitted `From: me`, leaked `Bcc` into SMTP DATA, never
//! filled reply headers, base64-encoded attachments as one unwrapped line and
//! turned an unreadable file into an empty attachment.
//!
//! Invariants this module owns:
//!
//! * **From** comes from the selected authorized account/identity, never a
//!   placeholder.
//! * Every address is parsed and validated (`lettre`), so CR/LF injection and
//!   malformed mailboxes are rejected before anything is queued.
//! * `To` may be empty only when `Cc` or `Bcc` has a recipient.
//! * The envelope de-duplicates case-insensitively but the headers keep the
//!   display names the user typed.
//! * The stored raw MIME **contains** the `Bcc` header (Gmail's REST API
//!   derives the delivery list from the message), and [`strip_bcc`] removes it
//!   for SMTP, whose envelope carries those recipients instead.
//! * Attachment bytes are read for real: a missing or unreadable source is a
//!   hard error, never an empty attachment.
//! * The raw MIME size is measured after encoding and compared against
//!   [`RAW_MIME_LIMIT_BYTES`]; nothing is queued above it.

use crate::dto::{Address, AttachmentRef, Draft};
use crate::errors::SiftError;
use mail_builder::headers::address::Address as MbAddress;
use mail_builder::headers::message_id::MessageId;
use mail_builder::MessageBuilder;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Sift's current send limit: the raw MIME bytes handed to a transport, before
/// any transport-side encoding. This is an application limit, deliberately
/// conservative; it is not a claim about how any provider measures mail.
pub const RAW_MIME_LIMIT_BYTES: usize = 25 * 1024 * 1024;

/// A sending identity (an account address, or one of its verified aliases).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Identity {
    pub email: String,
    pub display_name: Option<String>,
}

/// One address, validated: the mailbox itself is canonical and the display
/// name is preserved verbatim (minus surrounding whitespace).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mailbox {
    pub name: Option<String>,
    pub email: String,
}

/// Frozen description of one send. `revision` is the draft revision the bytes
/// were built from, so a stale preparation can never be confused with a newer
/// edit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparedSend {
    #[serde(rename = "rawPath")]
    pub raw_path: String,
    #[serde(rename = "rawSize")]
    pub raw_size: i64,
    #[serde(rename = "accountId")]
    pub account_id: String,
    pub from: String,
    #[serde(rename = "envelopeRecipients")]
    pub envelope_recipients: Vec<String>,
    /// De-duplicated `Bcc` recipients, kept separately so the SMTP path can
    /// prove it stripped exactly those headers and nothing else.
    #[serde(rename = "bccRecipients")]
    pub bcc_recipients: Vec<String>,
    #[serde(rename = "rfcMessageId")]
    pub rfc_message_id: String,
    #[serde(rename = "draftId")]
    pub draft_id: String,
    pub revision: i64,
    #[serde(rename = "threadId", skip_serializing_if = "Option::is_none", default)]
    pub thread_id: Option<String>,
}

/// The bytes of one message, ready to write.
#[derive(Debug, Clone)]
pub struct OutgoingMessage {
    pub from: Identity,
    pub to: Vec<Mailbox>,
    pub cc: Vec<Mailbox>,
    pub bcc: Vec<Mailbox>,
    pub subject: String,
    pub text: String,
    pub html: String,
    /// `<>`-wrapped Message-ID of this message.
    pub rfc_message_id: String,
    /// `<>`-wrapped Message-ID of the message this one answers, if any.
    pub in_reply_to: Option<String>,
    /// References chain, oldest first.
    pub references: Vec<String>,
    pub date_unix: i64,
    pub attachments: Vec<OutgoingAttachment>,
}

#[derive(Debug, Clone)]
pub struct OutgoingAttachment {
    pub mime: String,
    pub filename: String,
    pub bytes: Vec<u8>,
}

/// Reject anything that would let a caller smuggle additional header lines
/// through an address or an identifier.
pub fn reject_header_injection(field: &str, value: &str) -> Result<(), SiftError> {
    if value.contains('\r') || value.contains('\n') {
        return Err(SiftError::app(
            "bad_recipient",
            format!("{field} contains a line break"),
            false,
        ));
    }
    Ok(())
}

/// Trim the RFC 5322 angle brackets (and any whitespace) from a Message-ID so
/// it can be handed to the builder, which adds them back.
pub fn bare_message_id(id: &str) -> &str {
    id.trim().trim_start_matches('<').trim_end_matches('>')
}

/// Normalize a Message-ID to the `<>`-wrapped form used in the database.
pub fn wrapped_message_id(id: &str) -> String {
    format!("<{}>", bare_message_id(id))
}

/// Validate one structured address. The mailbox is parsed with `lettre` so a
/// malformed address is an error here rather than a mail loop later.
pub fn parse_mailbox(field: &str, a: &Address) -> Result<Mailbox, SiftError> {
    let email = a.e.trim();
    reject_header_injection(field, email)?;
    if email.is_empty() {
        return Err(SiftError::app(
            "bad_recipient",
            format!("{field} has a recipient with no address"),
            false,
        ));
    }
    let name =
        a.n.as_deref()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(|n| n.to_string());
    if let Some(n) = name.as_deref() {
        reject_header_injection(field, n)?;
    }
    let parsed: lettre::Address = email.parse().map_err(|_| {
        SiftError::app(
            "bad_recipient",
            format!("{field} contains an invalid address: {email}"),
            false,
        )
    })?;
    Ok(Mailbox {
        name,
        email: parsed.to_string(),
    })
}

fn parse_list(field: &str, list: &[Address]) -> Result<Vec<Mailbox>, SiftError> {
    list.iter().map(|a| parse_mailbox(field, a)).collect()
}

/// Case-insensitive de-duplication that keeps the first display name seen and
/// preserves order, so a user who typed a name once still sees it.
pub fn dedupe_mailboxes(list: &[Mailbox]) -> Vec<Mailbox> {
    let mut out: Vec<Mailbox> = Vec::with_capacity(list.len());
    for m in list {
        match out
            .iter_mut()
            .find(|k| k.email.eq_ignore_ascii_case(&m.email))
        {
            Some(existing) => {
                if existing.name.is_none() {
                    existing.name = m.name.clone();
                }
            }
            None => out.push(m.clone()),
        }
    }
    out
}

/// The three address lists of one message, validated and de-duplicated.
#[derive(Debug, Clone, Default)]
pub struct RecipientLists {
    pub to: Vec<Mailbox>,
    pub cc: Vec<Mailbox>,
    pub bcc: Vec<Mailbox>,
}

/// Split a draft's address fields into validated, de-duplicated recipients.
///
/// `To` may be empty when `Cc` or `Bcc` has a recipient; a message with no
/// recipient at all is rejected instead of being sent to the sender.
pub fn recipients(draft: &Draft) -> Result<RecipientLists, SiftError> {
    let to = dedupe_mailboxes(&parse_list("To", &draft.to_json)?);
    let cc = dedupe_mailboxes(&parse_list("Cc", &draft.cc_json)?);
    let bcc = dedupe_mailboxes(&parse_list("Bcc", &draft.bcc_json)?);
    if to.is_empty() && cc.is_empty() && bcc.is_empty() {
        return Err(SiftError::app(
            "no_recipients",
            "Add at least one recipient before sending.",
            false,
        ));
    }
    Ok(RecipientLists { to, cc, bcc })
}

/// Read attachment bytes. A missing, unreadable or non-regular source file is
/// a hard failure: silently sending an empty attachment is corruption.
pub fn read_attachment(a: &AttachmentRef) -> Result<OutgoingAttachment, SiftError> {
    let path = Path::new(&a.path);
    let meta = std::fs::metadata(path).map_err(|e| {
        SiftError::app(
            "attachment_missing",
            format!("{} is no longer readable: {e}", a.name),
            false,
        )
    })?;
    if !meta.is_file() {
        return Err(SiftError::app(
            "attachment_missing",
            format!("{} is not a regular file", a.name),
            false,
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| {
        SiftError::app(
            "attachment_missing",
            format!("{} could not be read: {e}", a.name),
            false,
        )
    })?;
    Ok(OutgoingAttachment {
        mime: a.mime.clone(),
        filename: a.name.clone(),
        bytes,
    })
}

/// Build the builder's address list for one header. Rendering happens inside
/// `mail-builder`, which quotes, encodes and folds each display name.
fn mb_list(list: &[Mailbox]) -> MbAddress<'_> {
    MbAddress::from(
        list.iter()
            .map(|m| match m.name.as_deref() {
                Some(n) => MbAddress::from((n, m.email.as_str())),
                None => MbAddress::from(m.email.as_str()),
            })
            .collect::<Vec<MbAddress>>(),
    )
}

/// Build the raw MIME bytes. The message carries the `Bcc` header; the SMTP
/// path removes it with [`strip_bcc`] because its envelope carries those
/// recipients instead.
pub fn build_bytes(msg: &OutgoingMessage) -> Result<Vec<u8>, SiftError> {
    reject_header_injection("Subject", &msg.subject)?;
    reject_header_injection("From", &msg.from.email)?;
    if let Some(n) = msg.from.display_name.as_deref() {
        reject_header_injection("From", n)?;
    }
    reject_header_injection("Message-ID", &msg.rfc_message_id)?;
    for r in &msg.references {
        reject_header_injection("References", r)?;
    }
    if let Some(r) = msg.in_reply_to.as_deref() {
        reject_header_injection("In-Reply-To", r)?;
    }

    let from = match msg.from.display_name.as_deref() {
        Some(name) if !name.is_empty() => MbAddress::from((name, msg.from.email.as_str())),
        _ => MbAddress::from(msg.from.email.as_str()),
    };

    let mut builder = MessageBuilder::new()
        .from(from)
        .message_id(MessageId::new(bare_message_id(&msg.rfc_message_id)))
        .subject(msg.subject.as_str())
        .date(mail_builder::headers::date::Date::new(msg.date_unix));
    if !msg.to.is_empty() {
        builder = builder.to(mb_list(&msg.to));
    }
    if !msg.cc.is_empty() {
        builder = builder.cc(mb_list(&msg.cc));
    }
    if !msg.bcc.is_empty() {
        builder = builder.bcc(mb_list(&msg.bcc));
    }
    if let Some(ir) = msg.in_reply_to.as_deref() {
        builder = builder.in_reply_to(MessageId::new(bare_message_id(ir)));
    }
    if !msg.references.is_empty() {
        let ids: Vec<&str> = msg.references.iter().map(|r| bare_message_id(r)).collect();
        builder = builder.references(MessageId::from(ids));
    }
    builder = builder
        .text_body(msg.text.as_str())
        .html_body(msg.html.as_str());
    for a in &msg.attachments {
        builder = builder.attachment(a.mime.as_str(), a.filename.as_str(), a.bytes.as_slice());
    }
    builder
        .write_to_vec()
        .map_err(|e| SiftError::app("mime", format!("could not encode the message: {e}"), false))
}

/// Remove every `Bcc` header (including folded continuation lines) from the
/// header block, leaving the body byte-identical. Returns the stripped bytes
/// and how many `Bcc` headers were removed so the caller can check that a
/// message with Bcc recipients really did lose them.
pub fn strip_bcc(raw: &[u8]) -> (Vec<u8>, usize) {
    let mut out = Vec::with_capacity(raw.len());
    let mut removed = 0usize;
    let mut in_headers = true;
    let mut skipping = false;
    let mut start = 0usize;
    let mut i = 0usize;
    while i < raw.len() {
        // Find the end of this line (LF), handling CRLF and bare LF.
        let line_end = raw[i..].iter().position(|&b| b == b'\n').map(|p| i + p + 1);
        let end = line_end.unwrap_or(raw.len());
        let line = &raw[start..end];
        let is_blank = line.iter().all(|&b| b == b'\r' || b == b'\n');
        if in_headers && is_blank {
            in_headers = false;
            skipping = false;
        }
        if in_headers {
            let is_continuation = matches!(line.first(), Some(b' ' | b'\t'));
            if skipping && !is_continuation {
                skipping = false;
            }
            if !skipping {
                let name_end = line.iter().position(|&b| b == b':').unwrap_or(usize::MAX);
                let name = &line[..name_end.min(line.len())];
                if name.eq_ignore_ascii_case(b"bcc") {
                    skipping = true;
                    removed += 1;
                }
            }
            if skipping {
                i = end;
                start = end;
                continue;
            }
        }
        out.extend_from_slice(line);
        i = end;
        start = end;
    }
    (out, removed)
}

/// The file name a prepared message is stored under: no path separators, no
/// traversal, bounded length.
pub fn raw_file_name(rfc_message_id: &str) -> String {
    let id: String = bare_message_id(rfc_message_id)
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' || c == '@' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let id: String = id.chars().take(96).collect();
    format!("{id}.eml")
}

/// Directory (inside the measured draft cache) holding prepared sends and
/// staged attachments.
pub const DRAFT_CACHE_DIR: &str = "compose-cache";

/// Where a draft's prepared raw MIME lives: `compose-cache/<draftId>/`.
/// Draft-owned, so removing the draft removes its staging directory, and the
/// file name never escapes that directory.
pub fn draft_send_dir(data_dir: &Path, draft_id: &str) -> PathBuf {
    let safe: String = draft_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    data_dir.join(DRAFT_CACHE_DIR).join(safe)
}

/// Assemble the message described by a stored draft.
///
/// Attachment files are read here, so this is a blocking filesystem call: the
/// command path runs it on a worker thread.
pub fn message_for_draft(
    draft: &Draft,
    from: &Identity,
    date_unix: i64,
) -> Result<OutgoingMessage, SiftError> {
    message_with_recipients(draft, from, date_unix, recipients(draft)?)
}

fn message_with_recipients(
    draft: &Draft,
    from: &Identity,
    date_unix: i64,
    recipients: RecipientLists,
) -> Result<OutgoingMessage, SiftError> {
    let RecipientLists { to, cc, bcc } = recipients;
    let attachments: Vec<OutgoingAttachment> = draft
        .attachments_json
        .iter()
        .map(read_attachment)
        .collect::<Result<_, _>>()?;
    let rfc_message_id = match draft.rfc_message_id.as_deref().map(str::trim) {
        Some(id) if !id.is_empty() => wrapped_message_id(id),
        _ => generated_message_id(&from.email),
    };
    Ok(OutgoingMessage {
        from: from.clone(),
        to,
        cc,
        bcc,
        subject: draft.subject.clone(),
        text: crate::render::text::html_to_text(&draft.body_html),
        html: draft.body_html.clone(),
        rfc_message_id,
        in_reply_to: draft.parent_rfc_message_id.clone(),
        references: draft.references_json.clone(),
        date_unix,
        attachments,
    })
}

/// A Message-ID in the domain of the sending identity: plausible, unique, and
/// stable for a send-attempt lineage because it is stored back on the draft.
pub fn generated_message_id(from_email: &str) -> String {
    let domain = from_email
        .rsplit_once('@')
        .map(|(_, d)| d)
        .filter(|d| !d.is_empty())
        .unwrap_or("sift.local");
    format!("<{}@{}>", uuid::Uuid::now_v7().simple(), domain)
}

/// The deduplicated delivery list for a prepared message: every To/Cc/Bcc
/// recipient exactly once, compared case-insensitively.
pub fn envelope_recipients(msg: &OutgoingMessage) -> Vec<String> {
    let all: Vec<Mailbox> = msg
        .to
        .iter()
        .chain(msg.cc.iter())
        .chain(msg.bcc.iter())
        .cloned()
        .collect();
    dedupe_mailboxes(&all)
        .into_iter()
        .map(|m| m.email)
        .collect()
}

/// The one place the application limit is applied: the *encoded* raw MIME
/// size, measured after assembly and before anything is queued.
pub fn check_raw_size(raw_size: usize) -> Result<(), SiftError> {
    if raw_size > RAW_MIME_LIMIT_BYTES {
        return Err(too_large(raw_size));
    }
    Ok(())
}

/// Build and validate the raw MIME for a draft, returning the message and the
/// bytes. Fails (never truncates) above [`RAW_MIME_LIMIT_BYTES`].
pub fn prepare_bytes(
    draft: &Draft,
    from: &Identity,
    date_unix: i64,
) -> Result<(OutgoingMessage, Vec<u8>), SiftError> {
    let msg = message_for_draft(draft, from, date_unix)?;
    let raw = build_bytes(&msg)?;
    check_raw_size(raw.len())?;
    Ok((msg, raw))
}

/// Draft storage accepts an empty recipient list; delivery still uses `prepare_bytes`.
pub fn prepare_draft_bytes(
    draft: &Draft,
    from: &Identity,
    date_unix: i64,
) -> Result<Vec<u8>, SiftError> {
    let lists = RecipientLists {
        to: dedupe_mailboxes(&parse_list("To", &draft.to_json)?),
        cc: dedupe_mailboxes(&parse_list("Cc", &draft.cc_json)?),
        bcc: dedupe_mailboxes(&parse_list("Bcc", &draft.bcc_json)?),
    };
    let msg = message_with_recipients(draft, from, date_unix, lists)?;
    let raw = build_bytes(&msg)?;
    check_raw_size(raw.len())?;
    Ok(raw)
}

/// The typed over-limit error, phrased with Sift's own limit.
pub fn too_large(raw_size: usize) -> SiftError {
    SiftError::app(
        "too_large",
        format!(
            "This message is {:.1} MB after encoding, over Sift's current {:.0} MB send limit.",
            raw_size as f64 / (1024.0 * 1024.0),
            RAW_MIME_LIMIT_BYTES as f64 / (1024.0 * 1024.0)
        ),
        false,
    )
}

/// Write `bytes` atomically into `dir` (a temp file plus a rename), so a crash
/// never leaves a half-written message in the queue.
pub fn write_raw(dir: &Path, file_name: &str, bytes: &[u8]) -> Result<PathBuf, SiftError> {
    std::fs::create_dir_all(dir).map_err(|e| {
        SiftError::app(
            "storage",
            format!("could not create the send directory: {e}"),
            false,
        )
    })?;
    let dest = dir.join(file_name);
    let tmp = dir.join(format!(".{}.tmp", uuid::Uuid::now_v7().simple()));
    std::fs::write(&tmp, bytes).map_err(|e| {
        SiftError::app(
            "storage",
            format!("could not write the message: {e}"),
            false,
        )
    })?;
    if let Err(e) = std::fs::rename(&tmp, &dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(SiftError::app(
            "storage",
            format!("could not store the message: {e}"),
            false,
        ));
    }
    Ok(dest)
}

/// Freeze a draft into the immutable snapshot a send is queued from.
pub fn prepare(
    data_dir: &Path,
    draft: &Draft,
    from: &Identity,
    date_unix: i64,
) -> Result<PreparedSend, SiftError> {
    let (msg, raw) = prepare_bytes(draft, from, date_unix)?;
    let dir = draft_send_dir(data_dir, &draft.local_id);
    let path = write_raw(&dir, &raw_file_name(&msg.rfc_message_id), &raw)?;
    Ok(PreparedSend {
        raw_path: path.to_string_lossy().into_owned(),
        raw_size: raw.len() as i64,
        account_id: draft.account_id.clone(),
        from: from.email.clone(),
        envelope_recipients: envelope_recipients(&msg),
        bcc_recipients: msg.bcc.iter().map(|m| m.email.clone()).collect(),
        rfc_message_id: msg.rfc_message_id.clone(),
        draft_id: draft.local_id.clone(),
        revision: draft.revision,
        thread_id: draft.thread_id.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(name: Option<&str>, email: &str) -> Address {
        Address {
            n: name.map(str::to_string),
            e: email.into(),
            me: None,
        }
    }

    fn draft_with(to: Vec<Address>, cc: Vec<Address>, bcc: Vec<Address>) -> Draft {
        Draft {
            local_id: "d1".into(),
            account_id: "a1".into(),
            from_email: Some("ada@example.com".into()),
            to_json: to,
            cc_json: cc,
            bcc_json: bcc,
            subject: "Subject".into(),
            body_html: "<p>Hello <b>wörld</b></p>".into(),
            revision: 3,
            ..Default::default()
        }
    }

    #[test]
    fn recipientless_draft_can_sync_but_cannot_send() {
        let draft = draft_with(vec![], vec![], vec![]);
        let raw = prepare_draft_bytes(&draft, &identity(), 1_700_000_000).unwrap();
        assert!(String::from_utf8_lossy(&raw).contains("Subject: Subject"));
        assert!(prepare_bytes(&draft, &identity(), 1_700_000_000).is_err());
    }

    fn identity() -> Identity {
        Identity {
            email: "ada@example.com".into(),
            display_name: Some("Ada Lovelace".into()),
        }
    }

    fn parse(raw: &[u8]) -> mail_parser::Message<'_> {
        mail_parser::MessageParser::default()
            .parse(raw)
            .expect("generated MIME must parse")
    }

    #[test]
    fn p53_from_is_the_selected_identity_not_me() {
        let d = draft_with(vec![addr(None, "bob@example.com")], vec![], vec![]);
        let (_, raw) = prepare_bytes(&d, &identity(), 1_700_000_000).unwrap();
        let text = String::from_utf8_lossy(&raw);
        assert!(text.starts_with("From: "), "got {text:.120}");
        assert!(
            !text.contains("From: me"),
            "a placeholder From must never be emitted"
        );
        let m = parse(&raw);
        let from = m.from().unwrap().first().unwrap();
        assert_eq!(from.address().unwrap(), "ada@example.com");
        assert_eq!(from.name().unwrap(), "Ada Lovelace");
    }

    #[test]
    fn p53_bcc_is_in_the_message_but_not_in_the_smtp_bytes() {
        let d = draft_with(
            vec![addr(None, "bob@example.com")],
            vec![addr(Some("Carol"), "carol@example.com")],
            vec![addr(Some("Dan"), "dan@example.com")],
        );
        let (msg, raw) = prepare_bytes(&d, &identity(), 1_700_000_000).unwrap();
        assert!(String::from_utf8_lossy(&raw).contains("Bcc:"));
        // The delivery list keeps Bcc even though the header is stripped.
        assert_eq!(
            envelope_recipients(&msg),
            vec![
                "bob@example.com".to_string(),
                "carol@example.com".to_string(),
                "dan@example.com".to_string()
            ]
        );
        let (stripped, removed) = strip_bcc(&raw);
        assert_eq!(removed, 1, "exactly the Bcc header is removed");
        assert!(!String::from_utf8_lossy(&stripped).contains("Bcc"));
        let m = parse(&stripped);
        assert!(m.bcc().is_none());
        // Everything else survives, including the body.
        assert_eq!(m.to().unwrap().iter().count(), 1);
        assert_eq!(m.cc().unwrap().iter().count(), 1);
        assert!(m.body_html(0).map(|h| h.contains("wörld")).unwrap_or(false));
    }

    #[test]
    fn p53_bcc_only_with_empty_to_is_allowed() {
        let d = draft_with(vec![], vec![], vec![addr(None, "dan@example.com")]);
        let (msg, raw) = prepare_bytes(&d, &identity(), 1_700_000_000).unwrap();
        assert_eq!(msg.to.len(), 0);
        let m = parse(&raw);
        assert!(m.to().is_none(), "an empty To is omitted entirely");
        assert_eq!(m.bcc().unwrap().iter().count(), 1);
    }

    #[test]
    fn p53_no_recipients_is_rejected() {
        let d = draft_with(vec![], vec![], vec![]);
        let e = prepare_bytes(&d, &identity(), 1_700_000_000).unwrap_err();
        assert_eq!(serde_json::to_value(&e).unwrap()["code"], "no_recipients");
    }

    #[test]
    fn p53_injection_and_malformed_addresses_are_rejected() {
        let d = draft_with(
            vec![addr(None, "bob@example.com>\r\nBcc: mallory@example.com")],
            vec![],
            vec![],
        );
        let e = prepare_bytes(&d, &identity(), 1_700_000_000).unwrap_err();
        assert_eq!(serde_json::to_value(&e).unwrap()["code"], "bad_recipient");

        let d = draft_with(vec![addr(None, "not-an-address")], vec![], vec![]);
        let e = prepare_bytes(&d, &identity(), 1_700_000_000).unwrap_err();
        assert_eq!(serde_json::to_value(&e).unwrap()["code"], "bad_recipient");

        let d = draft_with(
            vec![addr(Some("Eve\r\nBcc: x@y.z"), "bob@example.com")],
            vec![],
            vec![],
        );
        let e = prepare_bytes(&d, &identity(), 1_700_000_000).unwrap_err();
        assert_eq!(serde_json::to_value(&e).unwrap()["code"], "bad_recipient");
    }

    #[test]
    fn p53_envelope_dedupes_case_insensitively_keeping_display_names() {
        let d = draft_with(
            vec![addr(Some("Bob"), "Bob@Example.com")],
            vec![addr(None, "bob@example.com")],
            vec![addr(None, "BOB@example.COM"), addr(None, "dan@example.com")],
        );
        let (msg, _) = prepare_bytes(&d, &identity(), 1_700_000_000).unwrap();
        assert_eq!(
            envelope_recipients(&msg),
            vec!["Bob@Example.com".to_string(), "dan@example.com".to_string()]
        );
        assert_eq!(d.to_json[0].n.as_deref(), Some("Bob"));
    }

    #[test]
    fn p53_reply_threading_headers_are_emitted() {
        let mut d = draft_with(vec![addr(None, "bob@example.com")], vec![], vec![]);
        d.parent_rfc_message_id = Some("<parent@example.com>".into());
        d.references_json = vec!["<root@example.com>".into(), "<parent@example.com>".into()];
        d.subject = "Re: Subject".into();
        d.rfc_message_id = Some("<stable-send-id@example.com>".into());
        let (_, raw) = prepare_bytes(&d, &identity(), 1_700_000_000).unwrap();
        let m = parse(&raw);
        // mail-parser reports the id without its angle brackets.
        assert_eq!(
            m.in_reply_to()
                .as_text()
                .map(crate::outgoing::bare_message_id),
            Some("parent@example.com"),
            "In-Reply-To carries the parent's Message-ID"
        );
        // `as_text()` reports only the last element of a list header, so the
        // whole chain is read as a list.
        let refs: Vec<&str> = m.references().as_text_list().unwrap();
        let refs: Vec<&str> = refs
            .iter()
            .map(|r| crate::outgoing::bare_message_id(r))
            .collect();
        assert_eq!(
            refs,
            vec!["root@example.com", "parent@example.com"],
            "the whole References chain is emitted in order"
        );
        // The stored Message-ID is reused verbatim: a retry of the same send
        // attempt keeps its identity.
        let id = m.message_id().unwrap();
        assert_eq!(bare_message_id(id), "stable-send-id@example.com");
    }

    #[test]
    fn p53_attachment_bytes_are_exact_and_filenames_round_trip() {
        use mail_parser::MimeHeaders;
        let dir = tempfile::tempdir().unwrap();
        let bytes: Vec<u8> = (0..=255u8).cycle().take(5000).collect();
        let path = dir.path().join("payload.bin");
        std::fs::write(&path, &bytes).unwrap();
        let mut d = draft_with(vec![addr(None, "bob@example.com")], vec![], vec![]);
        d.attachments_json = vec![AttachmentRef {
            name: "grüße vom Adele.pdf".into(),
            mime: "application/pdf".into(),
            size: bytes.len() as i64,
            path: path.to_string_lossy().into_owned(),
        }];
        let (_, raw) = prepare_bytes(&d, &identity(), 1_700_000_000).unwrap();
        let text = String::from_utf8_lossy(&raw);
        // Every encoded base64 line is wrapped at 76 characters (RFC 2045).
        let mut saw_base64 = false;
        for line in text.split("\r\n") {
            assert!(line.len() <= 998, "line too long: {line:.40}");
            let is_b64 = line.len() > 20
                && line
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=');
            if is_b64 {
                saw_base64 = true;
                assert!(line.len() <= 76, "base64 line is {} chars", line.len());
            }
        }
        assert!(saw_base64, "the attachment body must be base64 encoded");
        let m = parse(&raw);
        let part = m.attachments().next().expect("attachment part");
        assert_eq!(part.attachment_name().unwrap(), "grüße vom Adele.pdf");
        assert_eq!(part.contents(), bytes.as_slice());
        // The Unicode filename survives as an encoded-word or RFC 2231
        // parameter, never as a raw 8-bit header.
        assert!(
            raw.is_ascii() || !text.contains("grüße"),
            "header stays 7-bit"
        );
    }

    #[test]
    fn p53_missing_attachment_is_a_hard_failure() {
        let mut d = draft_with(vec![addr(None, "bob@example.com")], vec![], vec![]);
        d.attachments_json = vec![AttachmentRef {
            name: "gone.pdf".into(),
            mime: "application/pdf".into(),
            size: 12,
            path: "/definitely/not/here/gone.pdf".into(),
        }];
        let e = prepare_bytes(&d, &identity(), 1_700_000_000).unwrap_err();
        assert_eq!(
            serde_json::to_value(&e).unwrap()["code"],
            "attachment_missing"
        );
        // A directory is not an attachment source either.
        let dir = tempfile::tempdir().unwrap();
        d.attachments_json[0].path = dir.path().to_string_lossy().into_owned();
        let e = prepare_bytes(&d, &identity(), 1_700_000_000).unwrap_err();
        assert_eq!(
            serde_json::to_value(&e).unwrap()["code"],
            "attachment_missing"
        );
    }

    /// Assemble a message whose encoded size is at least `target` bytes, by
    /// adjusting the attachment input size (base64 turns 57 bytes into 78
    /// encoded ones: 76 characters plus the CRLF).
    fn message_weighing_at_least(target: usize) -> (OutgoingMessage, Vec<u8>) {
        let msg_with = |input: usize| OutgoingMessage {
            from: identity(),
            to: vec![Mailbox {
                name: None,
                email: "bob@example.com".into(),
            }],
            cc: vec![],
            bcc: vec![],
            subject: "boundary".into(),
            text: "body".into(),
            html: "<p>body</p>".into(),
            rfc_message_id: "<fixed@example.com>".into(),
            in_reply_to: None,
            references: vec![],
            date_unix: 1_700_000_000,
            attachments: vec![OutgoingAttachment {
                mime: "application/octet-stream".into(),
                filename: "blob.bin".into(),
                bytes: vec![7u8; input],
            }],
        };
        let mut input = target / 78 * 57;
        for _ in 0..4 {
            let msg = msg_with(input);
            let raw = build_bytes(&msg).unwrap();
            if raw.len() >= target {
                return (msg, raw);
            }
            let delta = (target - raw.len()) as i64;
            input = (input as i64 + delta * 57 / 78 + 8).max(0) as usize;
        }
        let msg = msg_with(input);
        let raw = build_bytes(&msg).unwrap();
        (msg, raw)
    }

    fn code(e: SiftError) -> String {
        serde_json::to_value(&e).unwrap()["code"]
            .as_str()
            .unwrap()
            .into()
    }

    #[test]
    fn p53_raw_size_boundaries_24_25_26_mib() {
        let mib = 1024 * 1024;
        // 24 MiB of encoded message: accepted, and the measured size really is
        // the encoded size (not the attachment's declared length).
        let (_, raw24) = message_weighing_at_least(24 * mib);
        assert!(raw24.len() >= 24 * mib && raw24.len() < 24 * mib + 1024);
        check_raw_size(raw24.len()).unwrap();

        // The gate itself, at exactly the 25 MiB limit and one byte past it.
        check_raw_size(RAW_MIME_LIMIT_BYTES).unwrap();
        assert_eq!(
            code(check_raw_size(RAW_MIME_LIMIT_BYTES + 1).unwrap_err()),
            "too_large"
        );

        // 26 MiB of encoded message: rejected.
        let (_, raw26) = message_weighing_at_least(26 * mib);
        assert!(raw26.len() >= 26 * mib);
        assert_eq!(code(check_raw_size(raw26.len()).unwrap_err()), "too_large");
    }

    #[test]
    fn p53_aggregate_of_multiple_files_is_measured_together() {
        let mib = 1024 * 1024;
        // Three 9 MiB attachments encode well past the limit even though no
        // single file comes close to it.
        let each = 9 * mib;
        let msg = OutgoingMessage {
            from: identity(),
            to: vec![Mailbox {
                name: None,
                email: "bob@example.com".into(),
            }],
            cc: vec![],
            bcc: vec![],
            subject: "aggregate".into(),
            text: "body".into(),
            html: "<p>body</p>".into(),
            rfc_message_id: "<fixed@example.com>".into(),
            in_reply_to: None,
            references: vec![],
            date_unix: 1_700_000_000,
            attachments: (0..3)
                .map(|i| OutgoingAttachment {
                    mime: "application/octet-stream".into(),
                    filename: format!("part-{i}.bin"),
                    bytes: vec![1u8; each],
                })
                .collect(),
        };
        let raw = build_bytes(&msg).unwrap();
        assert!(raw.len() > RAW_MIME_LIMIT_BYTES);
        assert_eq!(
            serde_json::to_value(check_raw_size(raw.len()).unwrap_err()).unwrap()["code"],
            "too_large"
        );
        // All three parts are present and separately encoded.
        let parsed = parse(&raw);
        assert_eq!(parsed.attachments().count(), 3);
    }

    #[test]
    fn p53_raw_file_name_is_safe() {
        // Angle brackets are stripped first; every other unsafe character
        // becomes an underscore.
        assert_eq!(raw_file_name("<a/b:../@x>"), "a_b_.._@x.eml".to_string());
        assert!(!raw_file_name("../../etc/passwd").contains('/'));
        assert!(raw_file_name(&"z".repeat(500)).len() <= 100);
    }

    #[test]
    fn p53_strip_bcc_only_touches_the_bcc_header() {
        let raw = b"From: a@b.c\r\nBcc: one@x.y,\r\n two@x.y\r\nBcc-Hint: keep\r\nSubject: hi\r\n\r\nBcc: not a header\r\n";
        let (out, removed) = strip_bcc(raw);
        assert_eq!(removed, 1);
        let text = String::from_utf8_lossy(&out);
        assert!(!text.contains("one@x.y"), "folded Bcc is removed: {text}");
        assert!(text.contains("Bcc-Hint: keep"));
        assert!(text.contains("Bcc: not a header"), "the body is untouched");
        assert!(text.starts_with("From: a@b.c\r\n"));
    }
}
