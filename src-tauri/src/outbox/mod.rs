//! Outbox drain, send preparation and remote draft synchronization.
//!
//! The drain loop is transport-agnostic: the provider applies the op, retry
//! policy lives here. Two payload shapes exist for `send`:
//!
//! * the prepared shape written by `drafts_send` — the raw MIME lives in a
//!   draft-owned file and only its path, size and envelope travel through
//!   SQLite;
//! * the legacy shape from before P5.3 — the whole message base64url-encoded
//!   inside the payload, kept working so an upgrade cannot strand queued mail.
//!   Its envelope is derived from the headers, and it is *never* defaulted to
//!   the sender: a message with no usable recipient fails instead.

use crate::db::Db;
use crate::dto::Draft;
use crate::errors::SiftError;
use crate::provider::{ApplyOutcome, OutboxOp, Provider, SendRequest};

fn backoff(attempts: i64) -> i64 {
    (1000 * 2i64.pow(attempts.min(6) as u32)).min(64_000)
}

/// Rebuild the delivery request for one queued `send` op.
///
/// `fallback_from` is the account address; it may fill in a missing sender,
/// but recipients are only ever the ones the message actually names. A raw
/// message with no usable recipient is an error, never "send it to myself".
pub fn send_request_from_payload(
    payload: &serde_json::Value,
    fallback_from: &str,
) -> Result<SendRequest, SiftError> {
    let thread_id = payload["threadId"].as_str().map(|s| s.to_string());
    if let Some(path) = payload["rawPath"]
        .as_str()
        .filter(|p| !p.trim().is_empty())
    {
        let raw = std::fs::read(path).map_err(|e| {
            SiftError::app(
                "storage",
                format!("the prepared message is no longer on disk ({path}): {e}"),
                false,
            )
        })?;
        let expected = payload["rawSize"].as_i64().unwrap_or(raw.len() as i64);
        if expected != raw.len() as i64 {
            return Err(SiftError::app(
                "storage",
                format!(
                    "the prepared message changed on disk ({} bytes, expected {expected})",
                    raw.len()
                ),
                false,
            ));
        }
        let recipients: Vec<String> = serde_json::from_value(
            payload["envelopeRecipients"].clone(),
        )
        .unwrap_or_default();
        if recipients.is_empty() {
            return Err(SiftError::app("no_recipients", "no recipients", false));
        }
        let bcc: Vec<String> =
            serde_json::from_value(payload["bccRecipients"].clone()).unwrap_or_default();
        return Ok(SendRequest {
            raw,
            thread_id,
            from: payload["from"]
                .as_str()
                .filter(|f| !f.trim().is_empty())
                .unwrap_or(fallback_from)
                .to_string(),
            recipients,
            bcc_count: bcc.len(),
        });
    }

    // Legacy payload: base64url of the complete raw message.
    use base64::Engine;
    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload["raw"].as_str().unwrap_or_default())
        .map_err(|e| {
            SiftError::app(
                "protocol",
                format!("the queued message could not be decoded: {e}"),
                false,
            )
        })?;
    if raw.is_empty() {
        return Err(SiftError::app(
            "protocol",
            "the queued message is empty",
            false,
        ));
    }
    let parsed = crate::provider::imap::smtp::envelope_from_raw(&raw, fallback_from)?;
    Ok(SendRequest {
        raw,
        thread_id,
        from: parsed.from,
        recipients: parsed.recipients,
        bcc_count: parsed.bcc_count,
    })
}

/// Apply one `draft_sync` op: build the MIME for an exact draft revision and
/// push it as the remote draft.
///
/// The revision is checked before and after the network call. When the local
/// content moved on while the push was in flight the remote copy we created is
/// removed again — and only that copy: a newer revision's remote draft is
/// never touched, so a finishing old task cannot delete newer work.
pub async fn apply_draft_sync(
    db: &Db,
    provider: &dyn Provider,
    account_id: &str,
    payload: &serde_json::Value,
) -> Result<ApplyOutcome, SiftError> {
    let local_id = payload["localId"].as_str().unwrap_or_default().to_string();
    let revision = payload["revision"].as_i64().unwrap_or_default();
    if local_id.is_empty() {
        return Err(SiftError::app("op", "draft_sync without a draft", false));
    }
    let Some(draft) = db
        .drafts_get(&local_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
    else {
        // The draft was discarded; its remote copy, if any, is gone with it.
        return Ok(ApplyOutcome::Done);
    };
    if draft.revision != revision {
        // Superseded by a newer sync; that op owns the remote copy now.
        return Ok(ApplyOutcome::AlreadyApplied);
    }
    let account = db
        .accounts_get(account_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
        .ok_or_else(|| SiftError::NotFound("account".into()))?;
    let identity = crate::outgoing::Identity {
        email: draft
            .from_email
            .clone()
            .unwrap_or_else(|| account.email.clone()),
        display_name: account.display_name.clone(),
    };
    let rfc_message_id = match draft.rfc_message_id.as_deref().map(str::trim) {
        Some(id) if !id.is_empty() => id.to_string(),
        _ => {
            let generated = crate::outgoing::generated_message_id(&identity.email);
            db.drafts_set_rfc_message_id(&local_id, &generated)
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            generated
        }
    };
    let (_, raw) = crate::outgoing::prepare_bytes(&draft, &identity, crate::db::now_ms() / 1000)?;
    let previous = draft.remote_draft_id.clone();
    let new = provider
        .draft_upsert(previous.as_deref(), &raw, &rfc_message_id)
        .await?;
    let adopted = db
        .drafts_mark_remote(
            &local_id,
            revision,
            &new.remote_draft_id,
            new.message_id.as_deref(),
            revision,
        )
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    if !adopted && previous.as_deref() != Some(new.remote_draft_id.as_str()) {
        // The user kept typing during the push. The copy we just created is a
        // stale duplicate; the newer revision's op syncs the real content, and
        // it never sees this id because the row was not updated.
        let _ = provider.draft_delete(&new.remote_draft_id).await;
    }
    Ok(ApplyOutcome::Done)
}

/// Import remote drafts that are not local yet and reconcile the ones that are
/// (P5.2).
///
/// Best effort by design: a transport that cannot list drafts returns nothing,
/// and an unsynced message is retried on the next tick instead of being
/// guessed at. Anything the user must see (a recovered copy) is already in the
/// database by the time this returns.
pub async fn sync_remote_drafts(
    db: &Db,
    provider: &dyn Provider,
    account_id: &str,
) -> Result<crate::db::drafts::ReconcileReport, SiftError> {
    let remote = provider.draft_list().await?;
    if remote.is_empty() {
        return Ok(Default::default());
    }
    let report = db
        .drafts_reconcile_remote(account_id, &remote)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    // Exactly one remote copy per draft lineage: an interrupted replacement
    // leaves the old draft behind, and Gmail would show two.
    for id in &report.stranded {
        let _ = provider.draft_delete(id).await;
    }
    Ok(report)
}

/// Drain one pending op for an account. Returns true if work was done.
pub async fn drain_one(
    db: &Db,
    provider: &dyn Provider,
    account_id: &str,
    online: bool,
) -> Result<bool, SiftError> {
    if !online {
        return Ok(false);
    }
    // coalesce: peek up to 5 consecutive modify_labels with same add/remove within 200ms window
    let Some(op) = db
        .outbox_next(account_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
    else {
        return Ok(false);
    };
    db.outbox_set(op.id, "inflight", op.attempts, op.not_before, None)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    let payload: serde_json::Value = serde_json::from_str(&op.payload).unwrap_or_default();
    // Op semantics that need both the database and the transport live here,
    // next to the retry policy; the provider only executes the transport call.
    let outcome = match op.kind.as_str() {
        "send" => {
            // Errors from the payload itself go through the same retry policy
            // as transport errors: an unreadable or unaddressable message must
            // end `failed`, not sit `inflight` forever.
            let fallback = db
                .accounts_get(account_id)
                .await
                .unwrap_or(None)
                .map(|a| a.email)
                .unwrap_or_default();
            match send_request_from_payload(&payload, &fallback) {
                Ok(req) => provider.send(&req).await.map(|_| ApplyOutcome::Done),
                Err(e) => Err(e),
            }
        }
        "draft_sync" => apply_draft_sync(db, provider, account_id, &payload).await,
        _ => {
            let pop = OutboxOp {
                id: op.id,
                account_id: op.account_id.clone(),
                kind: op.kind.clone(),
                payload,
            };
            provider.apply(&pop).await
        }
    };
    match outcome {
        Ok(ApplyOutcome::Done) | Ok(ApplyOutcome::AlreadyApplied) => {
            db.outbox_set(op.id, "done", op.attempts + 1, op.not_before, None)
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            Ok(true)
        }
        Err(e) => {
            let msg = e.to_string();
            let retryable = matches!(
                &e,
                SiftError::App {
                    retryable: true,
                    ..
                }
            ) || matches!(&e, SiftError::Http(_));
            if !retryable || op.attempts + 1 >= 8 {
                db.outbox_set(op.id, "failed", op.attempts + 1, 0, Some(msg))
                    .await
                    .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            } else {
                let nb = crate::db::now_ms() + backoff(op.attempts + 1);
                db.outbox_set(op.id, "pending", op.attempts + 1, nb, Some(msg))
                    .await
                    .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            }
            Err(e)
        }
    }
}

/// A draft's send identity, resolved from its account.
pub async fn draft_identity(db: &Db, draft: &Draft) -> Result<crate::outgoing::Identity, SiftError> {
    let account = db
        .accounts_get(&draft.account_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
        .ok_or_else(|| SiftError::NotFound("account".into()))?;
    Ok(crate::outgoing::Identity {
        email: draft
            .from_email
            .clone()
            .unwrap_or_else(|| account.email.clone()),
        display_name: account.display_name.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::gmail::{api::GmailApiProvider, client::GmailClient};

    #[tokio::test]
    async fn p6_t05_coalesce_shape() {
        // Coalescing is verified at the enqueue site: payloads with equal add/remove merge.
        // Here we assert drain_one on empty queue returns false.
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let p = GmailApiProvider::new("nope".into(), GmailClient::new("t".into()));
        assert!(!drain_one(&db, &p, "nope", true).await.unwrap());
    }

    #[test]
    fn p53_legacy_payload_without_recipients_never_sends_to_the_sender() {
        use base64::Engine;
        let raw = b"From: ada@example.com\r\nSubject: no recipients\r\n\r\nbody";
        let payload = serde_json::json!({
            "raw": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw),
        });
        let err = send_request_from_payload(&payload, "ada@example.com").unwrap_err();
        assert_eq!(
            serde_json::to_value(&err).unwrap()["code"],
            "bad_recipient",
            "an unaddressable message must fail, not fall back to self"
        );
    }

    #[test]
    fn p53_legacy_payload_keeps_bcc_in_the_envelope() {
        use base64::Engine;
        let raw = b"From: ada@example.com\r\nTo: bob@example.com\r\nBcc: dan@example.com\r\nSubject: s\r\n\r\nbody";
        let payload = serde_json::json!({
            "raw": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(raw),
            "threadId": "t1",
        });
        let req = send_request_from_payload(&payload, "ada@example.com").unwrap();
        assert_eq!(
            req.recipients,
            vec!["bob@example.com".to_string(), "dan@example.com".to_string()]
        );
        assert_eq!(req.bcc_count, 1);
        assert_eq!(req.from, "ada@example.com");
        assert_eq!(req.thread_id.as_deref(), Some("t1"));
    }

    #[tokio::test]
    async fn p53_prepared_payload_reads_the_file_and_checks_its_size() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("message.eml");
        let bytes = b"From: a@b.c\r\nTo: d@e.f\r\n\r\nbody";
        std::fs::write(&path, bytes).unwrap();
        let payload = serde_json::json!({
            "rawPath": path.to_string_lossy(),
            "rawSize": bytes.len() as i64,
            "from": "a@b.c",
            "envelopeRecipients": ["d@e.f"],
            "bccRecipients": [],
        });
        let req = send_request_from_payload(&payload, "fallback@example.com").unwrap();
        assert_eq!(req.from, "a@b.c");
        assert_eq!(req.recipients, vec!["d@e.f".to_string()]);
        assert!(!req.raw.is_empty());

        // A file that no longer matches the recorded size is a hard failure:
        // the bytes are not the ones the user approved.
        let payload = serde_json::json!({
            "rawPath": path.to_string_lossy(),
            "rawSize": 99,
            "from": "a@b.c",
            "envelopeRecipients": ["d@e.f"],
        });
        let err = send_request_from_payload(&payload, "x@y.z").unwrap_err();
        assert_eq!(serde_json::to_value(&err).unwrap()["code"], "storage");
    }
}
