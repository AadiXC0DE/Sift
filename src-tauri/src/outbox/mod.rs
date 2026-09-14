//! Outbox drain, send preparation and remote draft synchronization.
//!
//! The drain loop is transport-agnostic: the provider applies the op, the
//! durable state machine in [`crate::db::outbox`] owns the transitions and the
//! retry policy lives here. Two payload shapes exist for `send`:
//!
//! * the prepared shape written by `drafts_send` — the raw MIME lives in a
//!   draft-owned file and only its path, size and envelope travel through
//!   SQLite;
//! * the legacy shape from before P5.3 — the whole message base64url-encoded
//!   inside the payload, kept working so an upgrade cannot strand queued mail.
//!   Its envelope is derived from the headers, and it is *never* defaulted to
//!   the sender: a message with no usable recipient fails instead.
//!
//! Nothing in here resubmits a send whose acceptance is unknown. A failure the
//! transport cannot attribute to "before the message was submitted" moves the
//! operation to `uncertain` and leaves the decision to a reconciliation by
//! RFC Message-ID and, after that, to the user (P6.1).

use crate::db::outbox::Op;
use crate::db::Db;
use crate::dto::Draft;
use crate::errors::SiftError;
use crate::provider::{ApplyOutcome, OutboxOp, Provider, SendRequest};

/// How many times a side-effect-free failure is retried before the operation
/// becomes visible as failed.
const MAX_ATTEMPTS: i64 = 8;

fn backoff(attempts: i64) -> i64 {
    (1000 * 2i64.pow(attempts.min(6) as u32)).min(64_000)
}

fn db_error(e: anyhow::Error) -> SiftError {
    SiftError::app("db", e.to_string(), false)
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
    if let Some(path) = payload["rawPath"].as_str().filter(|p| !p.trim().is_empty()) {
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
        let recipients: Vec<String> =
            serde_json::from_value(payload["envelopeRecipients"].clone()).unwrap_or_default();
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
    let Some(draft) = db.drafts_get(&local_id).await.map_err(db_error)? else {
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
        .map_err(db_error)?
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
                .map_err(db_error)?;
            generated
        }
    };
    let raw = crate::outgoing::prepare_draft_bytes(&draft, &identity, crate::db::now_ms() / 1000)?;
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
        .map_err(db_error)?;
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
        .map_err(db_error)?;
    // Exactly one remote copy per draft lineage: an interrupted replacement
    // leaves the old draft behind, and Gmail would show two.
    for id in &report.stranded {
        let _ = provider.draft_delete(id).await;
    }
    Ok(report)
}

/// What executing one claimed operation produced, before it is written down.
enum Outcome {
    /// The provider accepted the operation (or proved it already applied).
    Done {
        already: bool,
        result: Option<serde_json::Value>,
    },
    /// A definite failure. `uncertain` means "the side effect may have
    /// happened": it is never retried automatically.
    Failure {
        code: String,
        message: String,
        retryable: bool,
        uncertain: bool,
    },
}

impl Outcome {
    fn failed(e: &SiftError) -> Self {
        Outcome::Failure {
            code: e.code(),
            message: e.to_string(),
            retryable: e.is_retryable(),
            uncertain: false,
        }
    }
}

/// Drain one runnable operation for an account. Returns true if work was done.
pub async fn drain_one(
    db: &Db,
    provider: &dyn Provider,
    account_id: &str,
    online: bool,
) -> Result<bool, SiftError> {
    if !online {
        return Ok(false);
    }
    // Reconciliation of an uncertain send comes first: it is the only thing
    // that can turn "we do not know" into a fact, and it never resubmits.
    if let Some(op) = db
        .outbox_claim_reconcile(account_id)
        .await
        .map_err(db_error)?
    {
        reconcile_send(db, provider, &op).await?;
        return Ok(true);
    }
    let Some(op) = db.outbox_claim(account_id).await.map_err(db_error)? else {
        return Ok(false);
    };
    run_claimed(db, provider, account_id, op).await
}

/// Execute one claimed operation and write the resulting transition.
async fn run_claimed(
    db: &Db,
    provider: &dyn Provider,
    account_id: &str,
    op: Op,
) -> Result<bool, SiftError> {
    // An operation whose payload cannot be read is a typed failure, never a
    // silent `{}`: the user has to see that Sift refused to act on it.
    let mut payload: serde_json::Value = match serde_json::from_str(&op.payload) {
        Ok(v) => v,
        Err(e) => {
            let message = format!("the queued operation is not readable ({e})");
            db.outbox_mark_failed(op.id, "payload_invalid", &message)
                .await
                .map_err(db_error)?;
            return Err(SiftError::app("payload_invalid", message, false));
        }
    };
    let outcome = match op.kind.as_str() {
        "send" => execute_send(db, provider, account_id, &op, &payload).await,
        // Permanent deletion is gated on what the provider actually granted
        // (P6.4): an OAuth consent that returned read access cannot authorize
        // removing mail from the account.
        "delete" => match db.accounts_get(account_id).await {
            Ok(Some(account)) if !account.allows_permanent_delete() => Outcome::Failure {
                code: "unsupported_operation".into(),
                message: "This Google account was not granted permission to delete mail, so Sift left it alone. Reconnect it and allow mail management to delete permanently.".into(),
                retryable: false,
                uncertain: false,
            },
            _ => {
                let applied = provider
                    .apply(&OutboxOp {
                        id: op.id,
                        account_id: op.account_id.clone(),
                        kind: op.kind.clone(),
                        payload: payload.clone(),
                    })
                    .await;
                match applied {
                    Ok(ApplyOutcome::Done) => Outcome::Done {
                        already: false,
                        result: None,
                    },
                    Ok(ApplyOutcome::AlreadyApplied) => Outcome::Done {
                        already: true,
                        result: None,
                    },
                    Err(e) => Outcome::failed(&e),
                }
            }
        },
        "draft_sync" => match apply_draft_sync(db, provider, account_id, &payload).await {
            Ok(ApplyOutcome::Done) => Outcome::Done {
                already: false,
                result: None,
            },
            Ok(ApplyOutcome::AlreadyApplied) => Outcome::Done {
                already: true,
                result: None,
            },
            Err(e) => Outcome::failed(&e),
        },
        "create_label" => execute_create_label(db, provider, account_id, &payload).await,
        _ => {
            // Symbolic label references (`name:Sift/Snoozed`) become real
            // provider ids here, after the create-label dependency has run.
            if !resolve_label_refs(db, account_id, &mut payload).await? {
                return Err(SiftError::app(
                    "label_not_ready",
                    "the label this change needs is not on the server yet",
                    true,
                ));
            }
            let applied = provider
                .apply(&OutboxOp {
                    id: op.id,
                    account_id: op.account_id.clone(),
                    kind: op.kind.clone(),
                    payload: payload.clone(),
                })
                .await;
            match applied {
                Ok(ApplyOutcome::Done) => Outcome::Done {
                    already: false,
                    result: None,
                },
                Ok(ApplyOutcome::AlreadyApplied) => Outcome::Done {
                    already: true,
                    result: None,
                },
                Err(e) => Outcome::failed(&e),
            }
        }
    };
    match outcome {
        Outcome::Done {
            mut result,
            already,
        } => {
            if already {
                // Evidence, not decoration: "the provider says this was
                // already in the requested state" is why no work was done.
                let mut v = result.take().unwrap_or_else(|| serde_json::json!({}));
                if let Some(obj) = v.as_object_mut() {
                    obj.insert("alreadyApplied".into(), serde_json::Value::Bool(true));
                }
                result = Some(v);
            }
            db.outbox_mark_done(op.id, result.as_ref().map(|v| v.to_string()).as_deref())
                .await
                .map_err(db_error)?;
            if op.kind == "send" {
                finish_send(db, provider, &op).await;
            }
            if op.kind == "delete" {
                // The server acknowledged the deletion: only now are the
                // cached attachment files and the stale UID rows released
                // (P6.4 — they are the recovery material until then).
                let payload = op.payload_value();
                let ids: Vec<String> = payload["messages"]
                    .as_array()
                    .map(|list| {
                        list.iter()
                            .filter_map(|m| {
                                m.get("id").and_then(|i| i.as_str()).map(str::to_string)
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let _ = db.imap_forget_messages(account_id, &ids).await;
                let paths: Vec<String> =
                    serde_json::from_value(payload["cachePaths"].clone()).unwrap_or_default();
                crate::actions::cleanup_deleted_cache(db, &paths).await;
            }
            Ok(true)
        }
        Outcome::Failure {
            code,
            message,
            retryable,
            uncertain,
        } => {
            if uncertain {
                db.outbox_mark_uncertain(op.id, &code, &message)
                    .await
                    .map_err(db_error)?;
                // Not a connectivity failure: the provider may well have
                // accepted the message. The UI is told through the operation.
                return Ok(true);
            }
            if retryable && op.attempts < MAX_ATTEMPTS {
                let not_before = crate::db::now_ms() + backoff(op.attempts);
                db.outbox_requeue(op.id, op.attempts, not_before, &code, &message)
                    .await
                    .map_err(db_error)?;
            } else {
                db.outbox_mark_failed(op.id, &code, &message)
                    .await
                    .map_err(db_error)?;
            }
            Err(describe_failure(&code, &message, retryable))
        }
    }
}

fn describe_failure(code: &str, message: &str, retryable: bool) -> SiftError {
    SiftError::app(code, message.to_string(), retryable)
}

/// Deliver one prepared message.
///
/// The distinction that matters: `send_uncertain` means the message may have
/// been handed over, so the operation must not be retried, and the draft must
/// not be marked sent either.
async fn execute_send(
    db: &Db,
    provider: &dyn Provider,
    account_id: &str,
    op: &Op,
    payload: &serde_json::Value,
) -> Outcome {
    let fallback = db
        .accounts_get(account_id)
        .await
        .unwrap_or(None)
        .map(|a| a.email)
        .unwrap_or_default();
    let req = match send_request_from_payload(payload, &fallback) {
        Ok(req) => req,
        // A message Sift cannot address is a definite, terminal failure: it
        // was never submitted and never will be.
        Err(e) => return Outcome::failed(&e),
    };
    match provider.send(&req).await {
        Ok(info) => Outcome::Done {
            already: false,
            result: Some(serde_json::json!({
                "providerId": info.id,
                "threadId": info.thread_id,
                "rfcMessageId": op.rfc_message_id,
            })),
        },
        Err(e) if e.code() == "send_uncertain" => Outcome::Failure {
            code: "send_uncertain".into(),
            message: e.to_string(),
            retryable: false,
            uncertain: true,
        },
        Err(e) => Outcome::failed(&e),
    }
}

/// Create (or adopt) the label a dependent operation needs.
async fn execute_create_label(
    db: &Db,
    provider: &dyn Provider,
    account_id: &str,
    payload: &serde_json::Value,
) -> Outcome {
    let name = payload["name"]
        .as_str()
        .unwrap_or_default()
        .trim()
        .to_string();
    if name.is_empty() {
        return Outcome::Failure {
            code: "payload_invalid".into(),
            message: "a label creation without a name".into(),
            retryable: false,
            uncertain: false,
        };
    }
    // Adopt an existing label rather than creating a second one.
    if let Some(existing) = db.label_id_by_name(account_id, &name).await.unwrap_or(None) {
        if !existing.starts_with("sift-local:") {
            return Outcome::Done {
                already: true,
                result: Some(serde_json::json!({"labelId": existing})),
            };
        }
    }
    match provider.create_label(&name).await {
        Ok(label) => {
            if let Err(e) = db.labels_adopt_created(&label).await {
                return Outcome::Failure {
                    code: "db".into(),
                    message: e.to_string(),
                    retryable: true,
                    uncertain: false,
                };
            }
            Outcome::Done {
                already: false,
                result: Some(serde_json::json!({"labelId": label.id})),
            }
        }
        Err(e) => Outcome::failed(&e),
    }
}

/// Post-send bookkeeping (P6.2): the draft is marked sent and its remote copy
/// removed, but the row and its staged content stay for the seven-day recovery
/// window. The archive-after-send operation is a dependent of this send and
/// becomes runnable only now, because the send is `done`.
async fn finish_send(db: &Db, provider: &dyn Provider, op: &Op) {
    let Some(draft_id) = op.draft_id.clone() else {
        return;
    };
    let revision = op.draft_revision.unwrap_or_default();
    if let Err(e) = db.drafts_mark_sent(&draft_id, revision).await {
        log::warn!("draft {draft_id} could not be marked sent: {e}");
    }
    // The remote draft is now a duplicate of a sent message.
    if let Ok(Some(draft)) = db.drafts_get(&draft_id).await {
        if let Some(remote) = draft.remote_draft_id.clone() {
            if let Err(e) = provider.draft_delete(&remote).await {
                log::debug!("remote draft {remote} of {draft_id} kept: {e}");
            }
        }
    }
}

/// Reconcile an uncertain send against the provider's Sent view (P6.1).
///
/// A match means the message was delivered: the operation becomes `done` with
/// the provider's identity as evidence. No match means only "not provable
/// yet", so the bounded schedule (5s, 30s, 120s) continues and afterwards the
/// user decides. Nothing here sends anything.
async fn reconcile_send(db: &Db, provider: &dyn Provider, op: &Op) -> Result<bool, SiftError> {
    let rfc = op
        .rfc_message_id
        .clone()
        .or_else(|| {
            op.payload_value()["rfcMessageId"]
                .as_str()
                .map(str::to_string)
        })
        .unwrap_or_default();
    if rfc.trim().is_empty() {
        db.outbox_reschedule_reconcile(op.id, i64::MAX)
            .await
            .map_err(db_error)?;
        return Ok(true);
    }
    match provider.sent_by_rfc_message_id(&rfc).await {
        Ok(Some(info)) => {
            db.outbox_mark_done(
                op.id,
                Some(
                    &serde_json::json!({
                        "reconciled": true,
                        "providerId": info.id,
                        "threadId": info.thread_id,
                        "rfcMessageId": rfc,
                    })
                    .to_string(),
                ),
            )
            .await
            .map_err(db_error)?;
            finish_send(db, provider, op).await;
            Ok(true)
        }
        Ok(None) => {
            db.outbox_reschedule_reconcile(op.id, op.reconcile_attempts)
                .await
                .map_err(db_error)?;
            Ok(true)
        }
        Err(e) => {
            db.outbox_reschedule_reconcile(op.id, op.reconcile_attempts)
                .await
                .map_err(db_error)?;
            Err(e)
        }
    }
}

/// Replace `name:<label>` references with the account's real label id.
///
/// Returns `false` when a label is still missing — the operation then waits
/// (retryable), which is how a queued `create_label` dependency is honoured
/// without ever inventing an id. The rewritten payload is persisted so the
/// label overlay used by sync sees the same ids the provider did.
pub async fn resolve_label_refs(
    db: &Db,
    account_id: &str,
    payload: &mut serde_json::Value,
) -> Result<bool, SiftError> {
    let mut unresolved = false;
    for field in ["add", "remove"] {
        let Some(items) = payload.get(field).and_then(|v| v.as_array()).cloned() else {
            continue;
        };
        let mut out: Vec<serde_json::Value> = Vec::with_capacity(items.len());
        for item in items {
            let Some(name) = item.as_str().and_then(|s| s.strip_prefix("name:")) else {
                out.push(item);
                continue;
            };
            match db
                .label_id_by_name(account_id, name)
                .await
                .map_err(db_error)?
            {
                Some(id) if !id.starts_with("sift-local:") => {
                    out.push(serde_json::Value::String(id))
                }
                _ => {
                    unresolved = true;
                    out.push(item);
                }
            }
        }
        payload[field] = serde_json::Value::Array(out);
    }
    Ok(!unresolved)
}

/// The operation key of a send: the draft and the *immutable frozen revision*
/// it froze. Two Send clicks for one revision are one operation (P6.2).
pub fn send_operation_key(draft_id: &str, revision: i64) -> String {
    format!("send:{draft_id}:{revision}")
}

/// The dependent archive-after-send key for the same frozen revision.
pub fn archive_operation_key(draft_id: &str, revision: i64) -> String {
    format!("archive-after-send:{draft_id}:{revision}")
}

/// The label-creation key: one `Sift/Snoozed` per account, whichever gesture
/// asked for it first.
pub fn label_operation_key(account_id: &str, name: &str) -> String {
    format!("label-create:{account_id}:{name}")
}

/// A draft's send identity, resolved from its account.
pub async fn draft_identity(
    db: &Db,
    draft: &Draft,
) -> Result<crate::outgoing::Identity, SiftError> {
    let account = db
        .accounts_get(&draft.account_id)
        .await
        .map_err(db_error)?
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

    #[tokio::test]
    async fn p6_1_unreadable_payload_is_a_typed_failure_not_an_empty_object() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let acc = db.new_account("a@x.com", None, None).await.unwrap();
        let op = db
            .outbox_enqueue(&acc.id, "modify_labels", "not json", None, 0)
            .await
            .unwrap();
        let provider = GmailApiProvider::new(acc.id.clone(), GmailClient::new("t".into()));
        let err = drain_one(&db, &provider, &acc.id, true).await.unwrap_err();
        assert_eq!(err.code(), "payload_invalid");
        let row = db.outbox_get(op).await.unwrap().unwrap();
        assert_eq!(row.state, crate::db::outbox::STATE_FAILED);
        assert_eq!(row.failure_code.as_deref(), Some("payload_invalid"));
    }
}
