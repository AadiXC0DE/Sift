//! Draft persistence (P5.1/P5.2).
//!
//! The draft row is the single local truth. Every save is monotonic: a save
//! that carries a stale `expectedRevision` is rejected rather than allowed to
//! overwrite newer content, and an autosave that changes nothing is a no-op.
//! Remote draft identity is bookkeeping the transport writes back through
//! [`Db::drafts_mark_remote`], which is revision-checked so a slow remote sync
//! can never claim a newer revision was saved.

use super::Db;
use crate::dto::{Draft, DraftPage, RemoteDraft};
use anyhow::Result;
use rusqlite::{params, Connection, OptionalExtension};

/// Every column a draft decode needs, listed explicitly so adding a column
/// cannot silently shift a positional read.
const COLS: &str = "local_id,account_id,from_email,remote_draft_id,remote_message_id,\
thread_id,in_reply_to_message_id,rfc_message_id,parent_rfc_message_id,references_json,mode,\
to_json,cc_json,bcc_json,subject,body_html,attachments_json,revision,saved_revision,\
remote_revision,state,not_before,scheduled_at,scheduled_timezone,scheduled_local_time,updated_at";

/// Draft rows in this state are finished: they exist only as the recovery
/// copy an acknowledged send leaves behind.
const SENT_STATE: &str = crate::dto::DRAFT_STATE_SENT;

/// Page size ceiling from appendix A: `drafts_list` never returns more than
/// this, whatever the caller asks for.
pub const DRAFTS_PAGE_MAX: i64 = 100;
const DRAFTS_PAGE_DEFAULT: i64 = 50;

fn json_vec<T: serde::de::DeserializeOwned>(s: String) -> Vec<T> {
    serde_json::from_str(&s).unwrap_or_default()
}

/// Outcome of Undo Send (P6.2).
///
/// `TooLate` carries the state the operation actually reached, so the composer
/// can say what happened ("Sift handed this to Gmail") instead of a generic
/// refusal — and never promises that nothing was sent.
#[derive(Debug, Clone)]
pub enum SendCancel {
    Cancelled(Box<Draft>),
    TooLate { state: String },
}

/// A privacy-safe recipient summary for the Outbox panel: the first display
/// name (or address) plus a count, never the message.
fn summarize_recipients(recipients: &[serde_json::Value]) -> String {
    let first = recipients.first().and_then(|r| {
        r.get("name")
            .and_then(|n| n.as_str())
            .filter(|n| !n.trim().is_empty())
            .or_else(|| r.get("email").and_then(|e| e.as_str()))
    });
    let total = recipients.len();
    match (first, total) {
        (Some(name), 1) => name.to_string(),
        (Some(name), n) => format!("{name} +{}", n - 1),
        (None, 0) => String::new(),
        (None, n) => format!("{n} recipients"),
    }
}

/// The prior label membership of every message in a thread, captured before a
/// gesture changes it: the exact values an Undo restores (P6.3).
pub(crate) fn capture_thread_state(
    tx: &rusqlite::Transaction<'_>,
    account_id: &str,
    thread_id: &str,
) -> Result<(Vec<String>, Option<String>)> {
    let mut ids: Vec<String> = vec![];
    let mut messages: Vec<serde_json::Value> = vec![];
    let mut st = tx.prepare(
        "SELECT id,label_ids,is_unread,is_starred FROM messages \
         WHERE account_id=? AND thread_id=? ORDER BY id",
    )?;
    let rows = st
        .query_map(params![account_id, thread_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)? != 0,
                r.get::<_, i64>(3)? != 0,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (id, labels, unread, starred) in rows {
        let labels: Vec<String> = serde_json::from_str(&labels).unwrap_or_default();
        messages.push(serde_json::json!({
            "id": id,
            "labels": labels,
            "unread": unread,
            "starred": starred,
        }));
        ids.push(id);
    }
    let previous = if messages.is_empty() {
        None
    } else {
        Some(serde_json::json!({ "messages": messages }).to_string())
    };
    Ok((ids, previous))
}

fn row_to_draft(r: &rusqlite::Row) -> rusqlite::Result<Draft> {
    Ok(Draft {
        local_id: r.get("local_id")?,
        account_id: r.get("account_id")?,
        from_email: r.get("from_email")?,
        remote_draft_id: r.get("remote_draft_id")?,
        remote_message_id: r.get("remote_message_id")?,
        thread_id: r.get("thread_id")?,
        in_reply_to_message_id: r.get("in_reply_to_message_id")?,
        rfc_message_id: r.get("rfc_message_id")?,
        parent_rfc_message_id: r.get("parent_rfc_message_id")?,
        references_json: json_vec(r.get::<_, String>("references_json")?),
        mode: r.get("mode")?,
        to_json: json_vec(r.get::<_, String>("to_json")?),
        cc_json: json_vec(r.get::<_, String>("cc_json")?),
        bcc_json: json_vec(r.get::<_, String>("bcc_json")?),
        subject: r.get("subject")?,
        body_html: r.get("body_html")?,
        attachments_json: json_vec(r.get::<_, String>("attachments_json")?),
        revision: r.get("revision")?,
        saved_revision: r.get("saved_revision")?,
        remote_revision: r.get("remote_revision")?,
        state: r.get("state")?,
        not_before: r.get("not_before")?,
        scheduled_at: r.get("scheduled_at")?,
        scheduled_timezone: r.get("scheduled_timezone")?,
        scheduled_local_time: r.get("scheduled_local_time")?,
        updated_at: r.get("updated_at")?,
    })
}

/// The parent message fields a reply's threading context is built from.
struct ParentRow {
    rfc_message_id: Option<String>,
    references_json: String,
    thread_id: Option<String>,
}

/// The account a draft belongs to, plus the threading context resolved inside
/// that account. Parent ids and threads are account-scoped, so a From switch
/// that moves a draft to another account must resolve them again.
struct DraftContext {
    account_id: String,
    thread_id: Option<String>,
    parent_rfc_message_id: Option<String>,
    references: Vec<String>,
}

fn account_email(c: &Connection, account_id: &str) -> Result<Option<(String, String)>, rusqlite::Error> {
    c.query_row(
        "SELECT id,email FROM accounts WHERE id=?",
        params![account_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .optional()
}

/// Resolve which account a save lands in, and the threading fields that follow
/// from it.
///
/// * The incoming account is the default.
/// * A `fromEmail` that belongs to a different configured account is an
///   intentional From switch: the draft moves with it.
/// * A `fromEmail` that matches no account is rejected — From is always an
///   authorized identity, never an arbitrary header.
/// * `in_reply_to_message_id` is resolved inside the target account so the
///   stored parent Message-ID and References chain are the real ones. When the
///   parent cannot be resolved there (a cross-account reply, or a parent that
///   is not synced yet), no foreign threading context is carried over.
fn resolve_context(
    c: &Connection,
    incoming: &Draft,
) -> Result<DraftContext, anyhow::Error> {
    let mut account_id = incoming.account_id.clone();
    if account_id.trim().is_empty() {
        anyhow::bail!("draft has no account");
    }
    let account_email_value: String = match account_email(c, &account_id)? {
        Some((_, email)) => email,
        None => anyhow::bail!("draft account not found"),
    };
    if let Some(from) = incoming
        .from_email
        .as_deref()
        .map(str::trim)
        .filter(|f| !f.is_empty())
    {
        if !from.eq_ignore_ascii_case(&account_email_value) {
            let switched: Option<String> = c
                .query_row(
                    "SELECT id FROM accounts WHERE lower(email)=lower(?)",
                    params![from],
                    |r| r.get(0),
                )
                .optional()?;
            match switched {
                // Intentional From switch: the draft follows the identity, and
                // its threading context is rebuilt for the new account below.
                Some(id) => account_id = id,
                None => {
                    return Err(crate::errors::SiftError::app(
                        "from_not_authorized",
                        format!("{from} is not a sending identity of any signed-in account"),
                        false,
                    )
                    .into())
                }
            }
        }
    }

    let mut thread_id = incoming.thread_id.clone();
    let mut parent_rfc = None;
    let mut references: Vec<String> = Vec::new();
    if let Some(mid) = incoming
        .in_reply_to_message_id
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        let parent: Option<ParentRow> = c
            .query_row(
                "SELECT rfc_message_id, references_json, thread_id FROM messages \
                 WHERE account_id=? AND id=?",
                params![account_id, mid],
                |r| {
                    Ok(ParentRow {
                        rfc_message_id: r.get(0)?,
                        references_json: r.get(1)?,
                        thread_id: r.get(2)?,
                    })
                },
            )
            .optional()?;
        if let Some(ParentRow {
            rfc_message_id: rfc,
            references_json: refs,
            thread_id: thread,
        }) = parent
        {
            parent_rfc = rfc;
            references = json_vec::<String>(refs);
            if let Some(p) = parent_rfc.as_deref() {
                if !references.iter().any(|r| r == p) {
                    references.push(p.to_string());
                }
            }
            thread_id = thread;
        } else {
            thread_id = None;
        }
    }
    Ok(DraftContext {
        account_id,
        thread_id,
        parent_rfc_message_id: parent_rfc,
        references,
    })
}

impl Db {
    /// Save one draft. Returns the stored row, whose `revision` is what the
    /// next save must pass as `expectedRevision`.
    ///
    /// A stale `expected_revision` is a typed `draft_conflict` error: the newer
    /// content stays untouched and the caller re-reads the draft.
    pub async fn drafts_upsert(
        &self,
        d: &Draft,
        expected_revision: Option<i64>,
    ) -> Result<Draft> {
        let mut incoming = d.clone();
        if incoming.local_id.trim().is_empty() {
            incoming.local_id = uuid::Uuid::now_v7().to_string();
        }
        self.write(move |c| {
            let tx = c.unchecked_transaction()?;
            let ctx = resolve_context(&tx, &incoming)?;
            let now = super::now_ms();
            let existing: Option<Draft> = tx
                .query_row(
                    &format!("SELECT {COLS} FROM drafts WHERE local_id=?"),
                    params![incoming.local_id],
                    row_to_draft,
                )
                .optional()?;

            let stored = match existing {
                None => {
                    // A brand-new draft is already revision 1: it has content
                    // that the remote copy does not have.
                    let mut row = incoming.clone();
                    row.account_id = ctx.account_id.clone();
                    row.thread_id = ctx.thread_id.clone();
                    row.parent_rfc_message_id = ctx.parent_rfc_message_id.clone();
                    row.references_json = ctx.references.clone();
                    row.revision = 1;
                    row.saved_revision = 0;
                    row.remote_revision = 0;
                    if row.state.trim().is_empty() {
                        row.state = crate::dto::DRAFT_STATE_EDITING.to_string();
                    }
                    row.updated_at = Some(now);
                    tx.execute(
                        "INSERT INTO drafts (local_id,account_id,from_email,remote_draft_id,\
                         remote_message_id,thread_id,in_reply_to_message_id,rfc_message_id,\
                         parent_rfc_message_id,references_json,mode,to_json,cc_json,bcc_json,\
                         subject,body_html,attachments_json,revision,saved_revision,\
                         remote_revision,state,not_before,scheduled_at,scheduled_timezone,\
                         scheduled_local_time,updated_at,dirty) \
                         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,1)",
                        params![
                            row.local_id,
                            row.account_id,
                            row.from_email,
                            row.remote_draft_id,
                            row.remote_message_id,
                            row.thread_id,
                            row.in_reply_to_message_id,
                            row.rfc_message_id,
                            row.parent_rfc_message_id,
                            serde_json::to_string(&row.references_json)?,
                            row.mode,
                            serde_json::to_string(&row.to_json)?,
                            serde_json::to_string(&row.cc_json)?,
                            serde_json::to_string(&row.bcc_json)?,
                            row.subject,
                            row.body_html,
                            serde_json::to_string(&row.attachments_json)?,
                            row.revision,
                            row.saved_revision,
                            row.remote_revision,
                            row.state,
                            row.not_before,
                            row.scheduled_at,
                            row.scheduled_timezone,
                            row.scheduled_local_time,
                            row.updated_at,
                        ],
                    )?;
                    row
                }
                Some(current) => {
                    if let Some(expected) = expected_revision {
                        if expected != current.revision {
                            return Err(crate::errors::SiftError::app(
                                "draft_conflict",
                                format!(
                                    "This draft changed elsewhere (saved revision {} , your save was based on {}). Reopen it to continue.",
                                    current.revision, expected
                                ),
                                false,
                            )
                            .into());
                        }
                    }
                    let mut row = incoming.clone();
                    row.account_id = ctx.account_id.clone();
                    row.thread_id = ctx.thread_id.clone();
                    row.parent_rfc_message_id = ctx.parent_rfc_message_id.clone();
                    row.references_json = ctx.references.clone();
                    row.remote_draft_id = current.remote_draft_id.clone();
                    row.remote_message_id = current.remote_message_id.clone();
                    row.saved_revision = current.saved_revision;
                    row.remote_revision = current.remote_revision;
                    // The send lineage identity is the server's to keep: a
                    // client that echoes back an older value must not reset it.
                    row.rfc_message_id = current
                        .rfc_message_id
                        .clone()
                        .or(incoming.rfc_message_id.clone());
                    if current.content_eq(&row) {
                        // Nothing changed: no revision burn, no remote wake-up.
                        return Ok(current);
                    }
                    row.revision = current.revision + 1;
                    row.scheduled_at = incoming.scheduled_at.or(current.scheduled_at);
                    row.scheduled_timezone = incoming
                        .scheduled_timezone
                        .clone()
                        .or(current.scheduled_timezone.clone());
                    row.scheduled_local_time = incoming
                        .scheduled_local_time
                        .clone()
                        .or(current.scheduled_local_time.clone());
                    if current.state == crate::dto::DRAFT_STATE_QUEUED {
                        // Editing a queued draft must not rewrite the frozen
                        // payload (P6.2): the pending send is cancelled first,
                        // then this edit becomes a new revision. An operation
                        // that already left `pending` cannot be taken back, so
                        // the edit is refused with the honest state instead.
                        let send: Option<(i64, String)> = tx
                            .query_row(
                                "SELECT id,state FROM outbox_ops WHERE kind='send' \
                                 AND draft_id=? AND draft_revision=? \
                                 ORDER BY id DESC LIMIT 1",
                                params![row.local_id, current.revision],
                                |r| Ok((r.get(0)?, r.get(1)?)),
                            )
                            .optional()?;
                        if let Some((op_id, state)) = send {
                            if !matches!(
                                state.as_str(),
                                crate::db::outbox::STATE_PENDING
                                    | crate::db::outbox::STATE_CANCELLED
                                    | crate::db::outbox::STATE_FAILED
                            ) {
                                return Err(crate::errors::SiftError::typed(
                                    "draft_queued",
                                    "This message is already on its way. Sift will not change what it is sending.",
                                    serde_json::json!({"opId": op_id, "state": state}),
                                )
                                .into());
                            }
                            if state == crate::db::outbox::STATE_PENDING {
                                tx.execute(
                                    "UPDATE outbox_ops SET state='cancelled', completed_at=?1, \
                                       failure_code='superseded_by_edit', operation_key=NULL \
                                     WHERE id=?2 AND state='pending'",
                                    params![now, op_id],
                                )?;
                                tx.execute(
                                    "UPDATE outbox_ops SET state='cancelled', completed_at=?1, \
                                       failure_code='dependency_failed' \
                                     WHERE depends_on_op_id=?2 AND state='pending'",
                                    params![now, op_id],
                                )?;
                            }
                        }
                        row.state = crate::dto::DRAFT_STATE_EDITING.to_string();
                        row.not_before = None;
                    } else {
                        row.state = current.state.clone();
                        row.not_before = current.not_before;
                    }
                    let dirty = if row.revision > row.saved_revision { 1 } else { 0 };
                    row.updated_at = Some(now);
                    tx.execute(
                        "UPDATE drafts SET account_id=?,from_email=?,remote_draft_id=?,\
                         remote_message_id=?,thread_id=?,in_reply_to_message_id=?,rfc_message_id=?,\
                         parent_rfc_message_id=?,references_json=?,mode=?,to_json=?,cc_json=?,\
                         bcc_json=?,subject=?,body_html=?,attachments_json=?,revision=?,\
                         saved_revision=?,remote_revision=?,state=?,not_before=?,scheduled_at=?,\
                         scheduled_timezone=?,scheduled_local_time=?,updated_at=?,dirty=? \
                         WHERE local_id=?",
                        params![
                            row.account_id,
                            row.from_email,
                            row.remote_draft_id,
                            row.remote_message_id,
                            row.thread_id,
                            row.in_reply_to_message_id,
                            row.rfc_message_id,
                            row.parent_rfc_message_id,
                            serde_json::to_string(&row.references_json)?,
                            row.mode,
                            serde_json::to_string(&row.to_json)?,
                            serde_json::to_string(&row.cc_json)?,
                            serde_json::to_string(&row.bcc_json)?,
                            row.subject,
                            row.body_html,
                            serde_json::to_string(&row.attachments_json)?,
                            row.revision,
                            row.saved_revision,
                            row.remote_revision,
                            row.state,
                            row.not_before,
                            row.scheduled_at,
                            row.scheduled_timezone,
                            row.scheduled_local_time,
                            row.updated_at,
                            dirty,
                            row.local_id,
                        ],
                    )?;
                    row
                }
            };
            tx.commit()?;
            Ok(stored)
        })
        .await
    }

    pub async fn drafts_get(&self, local_id: &str) -> Result<Option<Draft>> {
        let id = local_id.to_string();
        self.read(move |c| {
            Ok(c.query_row(
                &format!("SELECT {COLS} FROM drafts WHERE local_id=?"),
                params![id],
                row_to_draft,
            )
            .optional()?)
        })
        .await
    }

    /// One keyset page of drafts across the selected accounts, newest first.
    ///
    /// An empty `account_ids` means every account (the "All accounts" scope).
    /// The cursor is opaque: `updatedAt:localId`, matching the sort key, so a
    /// concurrent edit cannot duplicate or skip a row.
    pub async fn drafts_list(
        &self,
        account_ids: &[String],
        cursor: Option<&str>,
        limit: i64,
    ) -> Result<DraftPage> {
        let limit = if limit <= 0 {
            DRAFTS_PAGE_DEFAULT
        } else {
            limit.min(DRAFTS_PAGE_MAX)
        };
        let accounts = account_ids.to_vec();
        let cursor = cursor.map(|s| s.to_string());
        self.read(move |c| {
            // A sent draft is a seven-day recovery copy, not a draft: it must
            // never reappear in the Drafts view (P6.2).
            let mut sql = format!("SELECT {COLS} FROM drafts WHERE state<>'{SENT_STATE}'");
            if !accounts.is_empty() {
                let ph = vec!["?"; accounts.len()].join(",");
                sql.push_str(&format!(" AND account_id IN ({ph})"));
            }
            let mut args: Vec<Box<dyn rusqlite::ToSql>> = accounts
                .iter()
                .map(|a| Box::new(a.clone()) as Box<dyn rusqlite::ToSql>)
                .collect();
            if let Some(cur) = cursor.as_deref() {
                if let Some((ts, id)) = cur.split_once(':') {
                    if let Ok(ts) = ts.parse::<i64>() {
                        sql.push_str(
                            " AND (updated_at < ? OR (updated_at = ? AND local_id < ?))",
                        );
                        args.push(Box::new(ts));
                        args.push(Box::new(ts));
                        args.push(Box::new(id.to_string()));
                    }
                }
            }
            sql.push_str(" ORDER BY updated_at DESC, local_id DESC LIMIT ?");
            args.push(Box::new(limit + 1));
            let mut st = c.prepare(&sql)?;
            let refs: Vec<&dyn rusqlite::ToSql> = args.iter().map(|b| b.as_ref()).collect();
            let mut rows: Vec<Draft> = st
                .query_map(refs.as_slice(), row_to_draft)?
                .collect::<Result<_, _>>()?;
            let next_cursor = if rows.len() as i64 > limit {
                rows.truncate(limit as usize);
                rows.last().map(|d| {
                    format!("{}:{}", d.updated_at.unwrap_or(0), d.local_id)
                })
            } else {
                None
            };
            Ok(DraftPage {
                drafts: rows,
                next_cursor,
            })
        })
        .await
    }

    /// How many drafts an account holds. Used by the account-removal preview,
    /// which must not page through the whole list.
    pub async fn drafts_count(&self, account_id: &str) -> Result<i64> {
        let a = account_id.to_string();
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT count(*) FROM drafts WHERE account_id=?",
                params![a],
                |r| r.get(0),
            )?)
        })
        .await
    }

    pub async fn drafts_delete(&self, local_id: &str) -> Result<()> {
        let id = local_id.to_string();
        self.write(move |c| {
            c.execute("DELETE FROM drafts WHERE local_id=?", params![id])?;
            Ok(())
        })
        .await
    }

    /// Mark a draft queued for send and remember its deadline. Returns the
    /// stored row after the transition.
    pub async fn drafts_mark_queued(
        &self,
        local_id: &str,
        revision: i64,
        not_before: i64,
        rfc_message_id: &str,
    ) -> Result<Draft> {
        let (id, rfc) = (local_id.to_string(), rfc_message_id.to_string());
        self.write(move |c| {
            let changed = c.execute(
                "UPDATE drafts SET state=?, not_before=?, rfc_message_id=? \
                 WHERE local_id=? AND revision=?",
                params![
                    crate::dto::DRAFT_STATE_QUEUED,
                    not_before,
                    rfc,
                    id,
                    revision
                ],
            )?;
            if changed == 0 {
                return Err(crate::errors::SiftError::app(
                    "draft_conflict",
                    "This draft changed since you hit send. Reopen it and send again.",
                    false,
                )
                .into());
            }
            c.query_row(
                &format!("SELECT {COLS} FROM drafts WHERE local_id=?"),
                params![id],
                row_to_draft,
            )
            .map_err(Into::into)
        })
        .await
    }

    /// Return a draft to the editor after a cancelled send.
    pub async fn drafts_reopen(&self, local_id: &str) -> Result<Draft> {
        let id = local_id.to_string();
        self.write(move |c| {
            c.execute(
                "UPDATE drafts SET state=?, not_before=NULL WHERE local_id=?",
                params![crate::dto::DRAFT_STATE_EDITING, id],
            )?;
            c.query_row(
                &format!("SELECT {COLS} FROM drafts WHERE local_id=?"),
                params![id],
                row_to_draft,
            )
            .map_err(Into::into)
        })
        .await
    }

    /// Record the remote identity a draft sync produced.
    ///
    /// Revision-checked (compare-and-swap): when the local content moved on
    /// while the sync was in flight, nothing is written, so a finishing old
    /// task can never mark a newer revision as saved — and the caller then
    /// deletes the remote draft it just created instead of adopting it.
    /// Returns true when the row was updated.
    pub async fn drafts_mark_remote(
        &self,
        local_id: &str,
        revision: i64,
        remote_draft_id: &str,
        remote_message_id: Option<&str>,
        remote_revision: i64,
    ) -> Result<bool> {
        let (id, rid) = (local_id.to_string(), remote_draft_id.to_string());
        let rmid = remote_message_id.map(|s| s.to_string());
        self.write(move |c| {
            let changed = c.execute(
                "UPDATE drafts SET remote_draft_id=?, remote_message_id=?, \
                 saved_revision=?, remote_revision=?, dirty=0 \
                 WHERE local_id=? AND revision=?",
                params![rid, rmid, revision, remote_revision, id, revision],
            )?;
            Ok(changed > 0)
        })
        .await
    }

    /// Queue a prepared send.
    ///
    /// One transaction: the operation, the draft's `queued` state and — for
    /// Send & Archive — the dependent label operation all land together, so a
    /// crash can never leave a frozen revision with no operation, an operation
    /// whose draft is still editable, or an archive that could run before its
    /// send.
    ///
    /// The operation key is `send:<draft>:<frozen revision>`: a second Send
    /// click for the same revision reuses the operation instead of queueing a
    /// duplicate, and a send that supersedes it must address a new revision.
    pub async fn drafts_enqueue_send(
        &self,
        prepared: &crate::outgoing::PreparedSend,
        not_before: i64,
        archive_after_send: bool,
    ) -> Result<crate::dto::SendHandle> {
        let p = prepared.clone();
        self.write(move |c| {
            let tx = c.unchecked_transaction()?;
            let key = crate::outbox::send_operation_key(&p.draft_id, p.revision);
            // Double Send: the operation already exists and owns this revision.
            let existing: Option<(i64, i64, String)> = tx
                .query_row(
                    "SELECT id,not_before,state FROM outbox_ops WHERE operation_key=?",
                    params![key],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            if let Some((id, not_before, state)) = existing {
                if crate::db::outbox::STATE_CANCELLED != state {
                    return Ok(crate::dto::SendHandle {
                        op_id: id,
                        not_before,
                    });
                }
                // A cancelled attempt released its key (an Undo returns the
                // draft to editing); this Send is a fresh operation.
                tx.execute(
                    "UPDATE outbox_ops SET operation_key=NULL WHERE id=?",
                    params![id],
                )?;
            }
            // The frozen content: subject and a privacy-safe recipient summary
            // for the Outbox panel, never the message itself.
            let (subject, to_json): (String, String) = tx
                .query_row(
                    "SELECT subject,to_json FROM drafts WHERE local_id=?",
                    params![p.draft_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
                .unwrap_or_default();
            let recipients: Vec<serde_json::Value> =
                serde_json::from_str(&to_json).unwrap_or_default();
            let recipient_summary = summarize_recipients(&recipients);
            let changed = tx.execute(
                "UPDATE drafts SET state=?, not_before=?, rfc_message_id=? \
                 WHERE local_id=? AND revision=?",
                params![
                    crate::dto::DRAFT_STATE_QUEUED,
                    not_before,
                    p.rfc_message_id,
                    p.draft_id,
                    p.revision
                ],
            )?;
            if changed == 0 {
                return Err(crate::errors::SiftError::app(
                    "draft_conflict",
                    "This draft changed since you hit send. Reopen it and send again.",
                    false,
                )
                .into());
            }
            // The send already carries the newest content; a pending remote
            // draft sync would only publish an older revision.
            tx.execute(
                "UPDATE outbox_ops SET state='cancelled', completed_at=?1, failure_code='superseded_by_send' \
                 WHERE account_id=?2 AND kind='draft_sync' \
                 AND state IN ('pending','inflight') AND json_extract(payload,'$.localId')=?3",
                params![super::now_ms(), p.account_id, p.draft_id],
            )?;
            let payload = serde_json::json!({
                "localId": p.draft_id,
                "revision": p.revision,
                "rawPath": p.raw_path,
                "rawSize": p.raw_size,
                "from": p.from,
                "envelopeRecipients": p.envelope_recipients,
                "bccRecipients": p.bcc_recipients,
                "rfcMessageId": p.rfc_message_id,
                "threadId": p.thread_id,
                "recipientSummary": recipient_summary,
                "subject": subject,
                "archiveAfterSend": archive_after_send,
            })
            .to_string();
            tx.execute(
                "INSERT INTO outbox_ops (account_id,kind,payload,not_before,created_at,\
                   operation_key,draft_id,draft_revision,rfc_message_id,summary_action,\
                   summary_recipient,summary_subject) \
                 VALUES (?, 'send', ?, ?, ?, ?, ?, ?, ?, 'Sending', ?, ?)",
                params![
                    p.account_id,
                    payload,
                    not_before,
                    super::now_ms(),
                    key,
                    p.draft_id,
                    p.revision,
                    p.rfc_message_id,
                    recipient_summary,
                    subject
                ],
            )?;
            let op_id = tx.last_insert_rowid();
            // Send & Archive: a dependent label operation that only becomes
            // runnable once the send is done (P6.2).
            if archive_after_send {
                if let Some(thread_id) = p.thread_id.clone() {
                    let (ids, previous) =
                        capture_thread_state(&tx, &p.account_id, &thread_id)?;
                    if !ids.is_empty() {
                        let payload = serde_json::json!({
                            "ids": ids, "add": [], "remove": ["INBOX"],
                        })
                        .to_string();
                        tx.execute(
                            "INSERT INTO outbox_ops (account_id,kind,payload,state,attempts,not_before,created_at,\
                               operation_key,depends_on_op_id,previous_state_json,summary_action) \
                             VALUES (?, 'modify_labels', ?, 'pending', 0, 0, ?, ?, ?, ?, 'Archiving')",
                            params![
                                p.account_id,
                                payload,
                                super::now_ms(),
                                crate::outbox::archive_operation_key(&p.draft_id, p.revision),
                                op_id,
                                previous
                            ],
                        )?;
                    }
                }
            }
            tx.commit()?;
            Ok(crate::dto::SendHandle { op_id, not_before })
        })
        .await
    }

    /// Cancel a send that has not been claimed yet. Returns `None` when the
    /// operation already left `pending` — the caller must not claim the mail
    /// was unsent.
    pub async fn drafts_cancel_send(&self, op_id: i64) -> Result<Option<Draft>> {
        Ok(match self.drafts_cancel_send_detailed(op_id).await? {
            SendCancel::Cancelled(draft) => Some(*draft),
            SendCancel::TooLate { .. } => None,
        })
    }

    /// Undo Send with the honest state of the operation when it is too late
    /// (P6.2). The conditional `pending -> cancelled` update is the winner: a
    /// claim that landed first wins, and the caller is told which state it
    /// lost to instead of a bare failure.
    pub async fn drafts_cancel_send_detailed(&self, op_id: i64) -> Result<SendCancel> {
        self.write(move |c| {
            let tx = c.unchecked_transaction()?;
            let row: Option<(String, String)> = tx
                .query_row(
                    "SELECT state,payload FROM outbox_ops WHERE id=?",
                    params![op_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let Some((state, payload)) = row else {
                return Ok(SendCancel::TooLate {
                    state: "missing".into(),
                });
            };
            if state != crate::db::outbox::STATE_PENDING {
                return Ok(SendCancel::TooLate { state });
            }
            let changed = tx.execute(
                "UPDATE outbox_ops SET state='cancelled', completed_at=?1, failure_code='undone', \
                   operation_key=NULL WHERE id=?2 AND state=?3",
                params![super::now_ms(), op_id, crate::db::outbox::STATE_PENDING],
            )?;
            if changed == 0 {
                let state: String = tx.query_row(
                    "SELECT state FROM outbox_ops WHERE id=?",
                    params![op_id],
                    |r| r.get(0),
                )?;
                return Ok(SendCancel::TooLate { state });
            }
            // The archive that waited on this send can never run now.
            tx.execute(
                "UPDATE outbox_ops SET state='cancelled', completed_at=?1, \
                   failure_code='dependency_failed' \
                 WHERE depends_on_op_id=?2 AND state='pending'",
                params![super::now_ms(), op_id],
            )?;
            let payload: serde_json::Value = serde_json::from_str(&payload).unwrap_or_default();
            let local_id = payload["localId"].as_str().unwrap_or_default().to_string();
            tx.execute(
                "UPDATE drafts SET state=?, not_before=NULL WHERE local_id=?",
                params![crate::dto::DRAFT_STATE_EDITING, local_id],
            )?;
            let draft = tx
                .query_row(
                    &format!("SELECT {COLS} FROM drafts WHERE local_id=?"),
                    params![local_id],
                    row_to_draft,
                )
                .optional()?
                .ok_or_else(|| anyhow::anyhow!("the draft of operation {op_id} is gone"))?;
            tx.commit()?;
            Ok(SendCancel::Cancelled(Box::new(draft)))
        })
        .await
    }

    /// The send was accepted (or reconciled as accepted): the draft's content
    /// stays for the seven-day recovery window, and the row is terminal so it
    /// can no longer be queued or edited as an unsent draft (P6.2).
    pub async fn drafts_mark_sent(&self, local_id: &str, revision: i64) -> Result<()> {
        let l = local_id.to_string();
        self.write(move |c| {
            c.execute(
                "UPDATE drafts SET state=?, not_before=NULL WHERE local_id=? AND revision=?",
                params![crate::dto::DRAFT_STATE_SENT, l, revision],
            )?;
            Ok(())
        })
        .await
    }

    /// Sent drafts past their recovery window, oldest first. The caller
    /// removes the staged files and the frozen MIME of each returned id.
    pub async fn drafts_prune_sent(&self, retention_ms: i64) -> Result<Vec<String>> {
        self.write(move |c| {
            let cutoff = super::now_ms() - retention_ms;
            let ids: Vec<String> = c
                .prepare("SELECT local_id FROM drafts WHERE state=?1 AND updated_at < ?2")?
                .query_map(params![crate::dto::DRAFT_STATE_SENT, cutoff], |r| r.get(0))?
                .collect::<Result<Vec<String>, _>>()?;
            for id in &ids {
                c.execute("DELETE FROM drafts WHERE local_id=?", params![id])?;
            }
            Ok(ids)
        })
        .await
    }

    /// Remember the stable Message-ID of a draft's send lineage. Server-owned
    /// bookkeeping: it is written once, before the first remote push, and never
    /// overwritten by a client.
    pub async fn drafts_set_rfc_message_id(&self, local_id: &str, id: &str) -> Result<()> {
        let (l, i) = (local_id.to_string(), id.to_string());
        self.write(move |c| {
            c.execute(
                "UPDATE drafts SET rfc_message_id=COALESCE(rfc_message_id, ?) WHERE local_id=?",
                params![i, l],
            )?;
            Ok(())
        })
        .await
    }

    /// Does any *other* draft reference files under this directory? A
    /// "Recovered copy" shares the staged files of the draft it was recovered
    /// from, so discarding one must not delete the other's attachments.
    pub async fn drafts_share_staging_dir(&self, dir: &str, excluding: &str) -> Result<bool> {
        let (d, e) = (format!("%{dir}%"), excluding.to_string());
        self.read(move |c| {
            let n: i64 = c.query_row(
                "SELECT count(*) FROM drafts WHERE local_id<>? AND attachments_json LIKE ?",
                params![e, d],
                |r| r.get(0),
            )?;
            Ok(n > 0)
        })
        .await
    }

    /// Remote draft ids this account's local drafts claim. A remote sync uses
    /// it to tell "our own draft" from "a draft that only exists on the
    /// server".
    pub async fn drafts_remote_ids(&self, account_id: &str) -> Result<Vec<String>> {
        let a = account_id.to_string();
        self.read(move |c| {
            Ok(c.prepare(
                "SELECT remote_draft_id FROM drafts WHERE account_id=? AND remote_draft_id IS NOT NULL",
            )?
            .query_map(params![a], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<String>, _>>()?)
        })
        .await
    }

    /// Reconcile this account's local drafts with the drafts the server
    /// reports (P5.2).
    ///
    /// Three outcomes, none of which loses content:
    ///
    /// * a draft we pushed but whose remote id we never recorded adopts that
    ///   id (a transport that appended successfully and then failed to report
    ///   the new id);
    /// * a remote draft that changed under us while we held unsaved local
    ///   edits keeps the local content as a separate **Recovered copy**, and
    ///   the row itself takes the remote content — never last-writer-wins;
    /// * a draft that only exists on the server becomes a new editable local
    ///   draft, once its message is synced locally.
    ///
    /// A remote draft that duplicates a lineage we already track is reported
    /// as `stranded` so the caller can delete it: an interrupted replacement
    /// must not leave two copies of the same draft in Gmail.
    pub async fn drafts_reconcile_remote(
        &self,
        account_id: &str,
        remote: &[RemoteDraft],
    ) -> Result<ReconcileReport> {
        let account = account_id.to_string();
        let remote = remote.to_vec();
        self.write(move |c| {
            let tx = c.unchecked_transaction()?;
            let mut locals: Vec<Draft> = tx
                .prepare(&format!(
                    "SELECT {COLS} FROM drafts WHERE account_id=? ORDER BY updated_at DESC"
                ))?
                .query_map(params![account], row_to_draft)?
                .collect::<Result<_, _>>()?;
            let mut report = ReconcileReport::default();
            let mut claimed: Vec<String> = locals
                .iter()
                .filter_map(|l| l.remote_draft_id.clone())
                .collect();
            let now = super::now_ms();

            for r in &remote {
                let linked = locals.iter().position(|l| {
                    l.remote_draft_id.as_deref() == Some(r.remote_draft_id.as_str())
                });
                let lineage = locals.iter().position(|l| {
                    l.remote_draft_id.as_deref() != Some(r.remote_draft_id.as_str())
                        && (r
                            .rfc_message_id
                            .as_deref()
                            .is_some_and(|id| l.rfc_message_id.as_deref() == Some(id))
                            || r.message_id
                                .as_deref()
                                .is_some_and(|id| l.remote_message_id.as_deref() == Some(id)))
                });

                match (linked, lineage) {
                    // Our own draft, possibly changed on the server.
                    (Some(idx), _) => {
                        let l = locals[idx].clone();
                        let remote_message = r.message_id.as_deref();
                        let changed_remotely = remote_message
                            .is_some_and(|m| l.remote_message_id.as_deref() != Some(m));
                        if !changed_remotely {
                            continue;
                        }
                        let Some(content) = message_content(&tx, &account, remote_message.unwrap())
                        else {
                            // The edited copy is not synced yet: leave the row
                            // alone rather than guess, and try again later.
                            report.pending_import.push(r.remote_draft_id.clone());
                            continue;
                        };
                        if l.has_unsaved_revision() && !content.same_content(&l) {
                            // Conflict. Keep the local work under a new id.
                            let recovered = recovered_copy(&l, now);
                            insert_draft(&tx, &recovered)?;
                            report.recovered.push(recovered.local_id.clone());
                            locals.push(recovered);
                        }
                        let adopted = content.into_draft(
                            &l,
                            &r.remote_draft_id,
                            remote_message,
                            l.revision + 1,
                            now,
                        );
                        write_draft(&tx, &adopted)?;
                        report.adopted.push(l.local_id.clone());
                        locals[idx] = adopted;
                    }
                    // A draft we pushed whose remote id we never recorded — but
                    // only if this row is not already linked to another remote
                    // draft. Otherwise this entry is a stale duplicate of a
                    // lineage we already track, which we report (not adopt).
                    (None, Some(idx)) if locals[idx].remote_draft_id.is_some() => {
                        report.stranded.push(r.remote_draft_id.clone());
                        continue;
                    }
                    (None, Some(idx)) => {
                        let mut l = locals[idx].clone();
                        l.remote_draft_id = Some(r.remote_draft_id.clone());
                        l.remote_message_id = r.message_id.clone().or(l.remote_message_id);
                        l.saved_revision = l.revision;
                        l.remote_revision += 1;
                        write_draft(&tx, &l)?;
                        report.adopted.push(l.local_id.clone());
                        claimed.push(r.remote_draft_id.clone());
                        locals[idx] = l;
                    }
                    // A draft that only exists on the server.
                    (None, None) => {
                        let Some(message_id) = r.message_id.as_deref() else {
                            continue;
                        };
                        let Some(content) = message_content(&tx, &account, message_id) else {
                            report.pending_import.push(r.remote_draft_id.clone());
                            continue;
                        };
                        let mut imported = content.into_draft(
                            &Draft {
                                account_id: account.clone(),
                                ..Default::default()
                            },
                            &r.remote_draft_id,
                            Some(message_id),
                            1,
                            now,
                        );
                        imported.saved_revision = 1;
                        imported.local_id = uuid::Uuid::now_v7().to_string();
                        insert_draft(&tx, &imported)?;
                        report.imported.push(imported.local_id.clone());
                        claimed.push(r.remote_draft_id.clone());
                        locals.push(imported);
                    }
                }
            }

            tx.commit()?;
            Ok(report)
        })
        .await
    }
}

/// Content of a draft as it exists on the server, read from the message copy
/// the normal sync already stored.
struct MessageContent {
    subject: String,
    to: Vec<crate::dto::Address>,
    cc: Vec<crate::dto::Address>,
    bcc: Vec<crate::dto::Address>,
    html: String,
    rfc_message_id: Option<String>,
    thread_id: Option<String>,
}

fn message_content(
    c: &Connection,
    account_id: &str,
    message_id: &str,
) -> Option<MessageContent> {
    let row = c
        .query_row(
            "SELECT m.subject, m.to_json, m.cc_json, m.bcc_json, m.rfc_message_id, m.thread_id, \
             b.html_z, b.text_z \
             FROM messages m LEFT JOIN bodies b ON b.account_id=m.account_id AND b.message_id=m.id \
             WHERE m.account_id=? AND m.id=?",
            params![account_id, message_id],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<String>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, Option<Vec<u8>>>(6)?,
                    r.get::<_, Option<Vec<u8>>>(7)?,
                ))
            },
        )
        .optional()
        .ok()??;
    let html = row
        .6
        .as_deref()
        .map(super::bodies::uz)
        .filter(|h| !h.is_empty())
        .or_else(|| {
            row.7.as_deref().map(super::bodies::uz).filter(|t| !t.is_empty()).map(|t| {
                crate::render::text::to_html(&t).0
            })
        })
        .unwrap_or_default();
    Some(MessageContent {
        subject: row.0,
        to: json_vec(row.1),
        cc: json_vec(row.2),
        bcc: json_vec(row.3),
        rfc_message_id: row.4,
        thread_id: row.5,
        html,
    })
}

impl MessageContent {
    /// Whether the remote copy carries exactly the content the local row has.
    fn same_content(&self, l: &Draft) -> bool {
        self.subject == l.subject
            && self.html == l.body_html
            && self.to == l.to_json
            && self.cc == l.cc_json
            && self.bcc == l.bcc_json
    }

    /// Project the remote content onto a draft row.
    fn into_draft(
        self,
        base: &Draft,
        remote_draft_id: &str,
        remote_message_id: Option<&str>,
        revision: i64,
        now: i64,
    ) -> Draft {
        Draft {
            subject: self.subject,
            to_json: self.to,
            cc_json: self.cc,
            bcc_json: self.bcc,
            body_html: self.html,
            thread_id: self.thread_id.or(base.thread_id.clone()),
            rfc_message_id: self.rfc_message_id.or(base.rfc_message_id.clone()),
            remote_draft_id: Some(remote_draft_id.to_string()),
            remote_message_id: remote_message_id
                .map(str::to_string)
                .or(base.remote_message_id.clone()),
            revision,
            saved_revision: revision,
            remote_revision: base.remote_revision + 1,
            state: crate::dto::DRAFT_STATE_EDITING.to_string(),
            not_before: None,
            updated_at: Some(now),
            ..base.clone()
        }
    }
}

/// The local content of `l`, preserved verbatim as an editable new draft.
fn recovered_copy(l: &Draft, now: i64) -> Draft {
    Draft {
        local_id: uuid::Uuid::now_v7().to_string(),
        subject: format!("Recovered copy — {}", l.subject),
        remote_draft_id: None,
        remote_message_id: None,
        revision: 1,
        saved_revision: 0,
        remote_revision: 0,
        state: crate::dto::DRAFT_STATE_EDITING.to_string(),
        not_before: None,
        updated_at: Some(now),
        ..l.clone()
    }
}

fn insert_draft(c: &Connection, d: &Draft) -> Result<()> {
    c.execute(
        "INSERT INTO drafts (local_id,account_id,from_email,remote_draft_id,remote_message_id,\
         thread_id,in_reply_to_message_id,rfc_message_id,parent_rfc_message_id,references_json,\
         mode,to_json,cc_json,bcc_json,subject,body_html,attachments_json,revision,\
         saved_revision,remote_revision,state,not_before,scheduled_at,scheduled_timezone,\
         scheduled_local_time,updated_at,dirty) \
         VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
        params![
            d.local_id,
            d.account_id,
            d.from_email,
            d.remote_draft_id,
            d.remote_message_id,
            d.thread_id,
            d.in_reply_to_message_id,
            d.rfc_message_id,
            d.parent_rfc_message_id,
            serde_json::to_string(&d.references_json)?,
            d.mode,
            serde_json::to_string(&d.to_json)?,
            serde_json::to_string(&d.cc_json)?,
            serde_json::to_string(&d.bcc_json)?,
            d.subject,
            d.body_html,
            serde_json::to_string(&d.attachments_json)?,
            d.revision,
            d.saved_revision,
            d.remote_revision,
            d.state,
            d.not_before,
            d.scheduled_at,
            d.scheduled_timezone,
            d.scheduled_local_time,
            d.updated_at,
            i64::from(d.revision > d.saved_revision),
        ],
    )?;
    Ok(())
}

/// Only the fields a reconciliation is allowed to change; ids, attachments and
/// threading context of the local row are left exactly as they were.
fn write_draft(c: &Connection, d: &Draft) -> Result<()> {
    let dirty = i64::from(d.revision > d.saved_revision);
    c.execute(
        "UPDATE drafts SET subject=?,to_json=?,cc_json=?,bcc_json=?,body_html=?,thread_id=?,\
         rfc_message_id=?,remote_draft_id=?,remote_message_id=?,revision=?,saved_revision=?,\
         remote_revision=?,state=?,not_before=?,updated_at=?,dirty=? WHERE local_id=?",
        params![
            d.subject,
            serde_json::to_string(&d.to_json)?,
            serde_json::to_string(&d.cc_json)?,
            serde_json::to_string(&d.bcc_json)?,
            d.body_html,
            d.thread_id,
            d.rfc_message_id,
            d.remote_draft_id,
            d.remote_message_id,
            d.revision,
            d.saved_revision,
            d.remote_revision,
            d.state,
            d.not_before,
            d.updated_at,
            dirty,
            d.local_id,
        ],
    )?;
    Ok(())
}

/// What one remote-draft reconciliation changed, for logging and tests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReconcileReport {
    /// Local drafts that adopted a remote identity.
    pub adopted: Vec<String>,
    /// Local drafts created from a draft that only existed on the server.
    pub imported: Vec<String>,
    /// Local drafts preserved as "Recovered copy" rows.
    pub recovered: Vec<String>,
    /// Remote drafts with nothing local to import yet (message not synced).
    pub pending_import: Vec<String>,
    /// Remote drafts that duplicate a lineage already tracked locally: the
    /// caller deletes them so Gmail keeps exactly one draft per lineage.
    pub stranded: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn db() -> (tempfile::TempDir, Db, String) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let acc = db.new_account("ada@example.com", None, None).await.unwrap();
        (dir, db, acc.id)
    }

    #[tokio::test]
    async fn p51_upsert_is_monotonic_and_conflict_aware() {
        let (_d, db, acc) = db().await;
        let first = db
            .drafts_upsert(
                &Draft {
                    account_id: acc.clone(),
                    subject: "one".into(),
                    body_html: "<p>a</p>".into(),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(first.revision, 1);
        assert_eq!(first.state, "editing");

        let second = db
            .drafts_upsert(
                &Draft {
                    local_id: first.local_id.clone(),
                    subject: "two".into(),
                    body_html: "<p>ab</p>".into(),
                    ..first.clone()
                },
                Some(1),
            )
            .await
            .unwrap();
        assert_eq!(second.revision, 2);
        assert_eq!(second.local_id, first.local_id);

        // A save built on revision 1 arriving after revision 2 is rejected and
        // the newer content survives.
        let stale = db
            .drafts_upsert(
                &Draft {
                    subject: "stale".into(),
                    body_html: "<p>stale</p>".into(),
                    ..first.clone()
                },
                Some(1),
            )
            .await
            .unwrap_err();
        let code = serde_json::to_value(
            stale
                .downcast_ref::<crate::errors::SiftError>()
                .expect("typed"),
        )
        .unwrap()["code"]
            .clone();
        assert_eq!(code, "draft_conflict");
        let stored = db.drafts_get(&first.local_id).await.unwrap().unwrap();
        assert_eq!(stored.subject, "two");
        assert_eq!(stored.revision, 2);

        // An identical snapshot is a no-op: no revision, no new row.
        let again = db
            .drafts_upsert(
                &Draft {
                    subject: "two".into(),
                    body_html: "<p>ab</p>".into(),
                    ..second.clone()
                },
                Some(2),
            )
            .await
            .unwrap();
        assert_eq!(again.revision, 2);
        let page = db.drafts_list(&[acc], None, 10).await.unwrap();
        assert_eq!(page.drafts.len(), 1);
    }

    #[tokio::test]
    async fn p51_subject_only_edit_persists_exactly_one_draft() {
        let (_d, db, acc) = db().await;
        // The composer always sends the same local id; the autosave re-sends
        // the same snapshot when nothing changed.
        let mut local_id = String::new();
        for (i, subject) in ["a", "a", "ab", "ab"].iter().enumerate() {
            let saved = db
                .drafts_upsert(
                    &Draft {
                        local_id: local_id.clone(),
                        account_id: acc.clone(),
                        subject: subject.to_string(),
                        ..Default::default()
                    },
                    None,
                )
                .await
                .unwrap();
            local_id = saved.local_id.clone();
            assert_eq!(saved.subject, *subject, "save {i}");
        }
        let page = db.drafts_list(&[acc], None, 10).await.unwrap();
        assert_eq!(page.drafts.len(), 1, "exactly one durable draft");
        assert_eq!(page.drafts[0].revision, 2, "two real changes, two revisions");
    }

    #[tokio::test]
    async fn p51_from_switch_moves_the_account_and_rebuilds_threading() {
        let (_d, db, acc) = db().await;
        let other = db.new_account("grace@example.com", None, None).await.unwrap();
        db.messages_upsert(crate::db::messages::MsgUpsert {
            id: "m1".into(),
            account_id: other.id.clone(),
            thread_id: "t1".into(),
            subject: "Parent".into(),
            rfc_message_id: Some("<parent@example.com>".into()),
            references_json: "[\"<root@example.com>\"]".into(),
            ..Default::default()
        })
        .await
        .unwrap();
        let saved = db
            .drafts_upsert(
                &Draft {
                    account_id: acc.clone(),
                    from_email: Some("grace@example.com".into()),
                    in_reply_to_message_id: Some("m1".into()),
                    subject: "Re: Parent".into(),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(
            saved.account_id, other.id,
            "an intentional From switch moves the draft to the identity's account"
        );
        assert_eq!(saved.thread_id.as_deref(), Some("t1"));
        assert_eq!(
            saved.parent_rfc_message_id.as_deref(),
            Some("<parent@example.com>")
        );
        assert_eq!(
            saved.references_json,
            vec![
                "<root@example.com>".to_string(),
                "<parent@example.com>".to_string()
            ]
        );
        // An unauthorized identity is refused instead of being written.
        let err = db
            .drafts_upsert(
                &Draft {
                    account_id: acc.clone(),
                    from_email: Some("stranger@example.com".into()),
                    subject: "x".into(),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(
            serde_json::to_value(err.downcast_ref::<crate::errors::SiftError>().unwrap()).unwrap()
                ["code"],
            "from_not_authorized"
        );
    }

    #[tokio::test]
    async fn p51_list_pages_by_updated_at_and_id_with_a_bounded_limit() {
        let (_d, db, acc) = db().await;
        for i in 0..5 {
            db.drafts_upsert(
                &Draft {
                    account_id: acc.clone(),
                    subject: format!("s{i}"),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        }
        let page = db.drafts_list(std::slice::from_ref(&acc), None, 2).await.unwrap();
        assert_eq!(page.drafts.len(), 2);
        let cursor = page.next_cursor.clone().expect("more pages");
        let page2 = db
            .drafts_list(std::slice::from_ref(&acc), Some(&cursor), 2)
            .await
            .unwrap();
        assert_eq!(page2.drafts.len(), 2);
        // No row appears twice across pages.
        let ids: Vec<String> = page
            .drafts
            .iter()
            .chain(page2.drafts.iter())
            .map(|d| d.local_id.clone())
            .collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len());

        // The limit is clamped to the contract's ceiling.
        let huge = db.drafts_list(std::slice::from_ref(&acc), None, 10_000).await.unwrap();
        assert!(huge.drafts.len() <= DRAFTS_PAGE_MAX as usize);
        // Empty account ids = every account.
        let all = db.drafts_list(&[], None, 10).await.unwrap();
        assert_eq!(all.drafts.len(), 5);
    }

    async fn put_message(db: &Db, acc: &str, id: &str, subject: &str, html: &str) {
        db.messages_upsert(crate::db::messages::MsgUpsert {
            id: id.into(),
            account_id: acc.into(),
            thread_id: "t1".into(),
            subject: subject.into(),
            rfc_message_id: Some(format!("<{id}@example.com>")),
            ..Default::default()
        })
        .await
        .unwrap();
        db.bodies_put(crate::db::bodies::BodyPut {
            account_id: acc.into(),
            message_id: id.into(),
            html: Some(html.into()),
            text: None,
            remote_images: 0,
            trackers: 0,
            dark_safe: true,
            quoted_from: None,
            unsubscribe: Default::default(),
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn p52_remote_only_draft_is_imported_as_editable_local() {
        let (_d, db, acc) = db().await;
        put_message(&db, &acc, "m1", "Server draft", "<p>from gmail</p>").await;
        let report = db
            .drafts_reconcile_remote(
                &acc,
                &[RemoteDraft {
                    remote_draft_id: "r1".into(),
                    message_id: Some("m1".into()),
                    thread_id: Some("t1".into()),
                    rfc_message_id: None,
                }],
            )
            .await
            .unwrap();
        assert_eq!(report.imported.len(), 1);
        let page = db.drafts_list(std::slice::from_ref(&acc), None, 10).await.unwrap();
        let d = &page.drafts[0];
        assert_eq!(d.subject, "Server draft");
        assert_eq!(d.body_html, "<p>from gmail</p>");
        assert_eq!(d.remote_draft_id.as_deref(), Some("r1"));
        assert_eq!(d.remote_message_id.as_deref(), Some("m1"));
        assert_eq!(d.state, "editing");
        assert_eq!(
            d.saved_revision, d.revision,
            "an imported remote draft has no unsaved local revision"
        );
        // Re-running is idempotent: it is now our own remote draft.
        let again = db
            .drafts_reconcile_remote(
                &acc,
                &[RemoteDraft {
                    remote_draft_id: "r1".into(),
                    message_id: Some("m1".into()),
                    thread_id: None,
                    rfc_message_id: None,
                }],
            )
            .await
            .unwrap();
        assert!(again.imported.is_empty());
        assert!(again.adopted.is_empty());
        assert_eq!(db.drafts_list(&[acc], None, 10).await.unwrap().drafts.len(), 1);
    }

    #[tokio::test]
    async fn p52_remote_change_with_unsaved_local_edits_keeps_a_recovered_copy() {
        let (_d, db, acc) = db().await;
        put_message(&db, &acc, "m1", "Draft", "<p>v1</p>").await;
        put_message(&db, &acc, "m2", "Draft edited elsewhere", "<p>v2</p>").await;
        let local = db
            .drafts_upsert(
                &Draft {
                    account_id: acc.clone(),
                    subject: "Draft".into(),
                    body_html: "<p>v1</p>".into(),
                    remote_draft_id: Some("r1".into()),
                    remote_message_id: Some("m1".into()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        // The user keeps typing while another client edits the same draft.
        let local = db
            .drafts_upsert(
                &Draft {
                    body_html: "<p>v1 plus my work</p>".into(),
                    ..local.clone()
                },
                Some(1),
            )
            .await
            .unwrap();
        assert!(local.has_unsaved_revision());

        let report = db
            .drafts_reconcile_remote(
                &acc,
                &[RemoteDraft {
                    remote_draft_id: "r1".into(),
                    message_id: Some("m2".into()),
                    thread_id: None,
                    rfc_message_id: None,
                }],
            )
            .await
            .unwrap();
        assert_eq!(report.recovered.len(), 1, "the local work is preserved");
        let page = db.drafts_list(std::slice::from_ref(&acc), None, 10).await.unwrap();
        assert_eq!(page.drafts.len(), 2);
        let recovered = page
            .drafts
            .iter()
            .find(|d| d.local_id == report.recovered[0])
            .unwrap();
        assert!(recovered.subject.starts_with("Recovered copy"));
        assert_eq!(recovered.body_html, "<p>v1 plus my work</p>");
        assert!(recovered.remote_draft_id.is_none());
        let current = page
            .drafts
            .iter()
            .find(|d| d.local_id == local.local_id)
            .unwrap();
        assert_eq!(current.body_html, "<p>v2</p>", "the row takes the remote content");
        assert_eq!(current.remote_message_id.as_deref(), Some("m2"));
        assert!(!current.has_unsaved_revision());
    }

    #[tokio::test]
    async fn p52_clean_local_row_adopts_a_remote_change_without_a_copy() {
        let (_d, db, acc) = db().await;
        put_message(&db, &acc, "m1", "Draft", "<p>v1</p>").await;
        put_message(&db, &acc, "m2", "Draft", "<p>v2</p>").await;
        let local = db
            .drafts_upsert(
                &Draft {
                    account_id: acc.clone(),
                    subject: "Draft".into(),
                    body_html: "<p>v1</p>".into(),
                    remote_draft_id: Some("r1".into()),
                    remote_message_id: Some("m1".into()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        db.drafts_mark_remote(&local.local_id, 1, "r1", Some("m1"), 1)
            .await
            .unwrap();
        let report = db
            .drafts_reconcile_remote(
                &acc,
                &[RemoteDraft {
                    remote_draft_id: "r1".into(),
                    message_id: Some("m2".into()),
                    thread_id: None,
                    rfc_message_id: None,
                }],
            )
            .await
            .unwrap();
        assert!(report.recovered.is_empty());
        assert_eq!(report.adopted, vec![local.local_id.clone()]);
        let d = db.drafts_get(&local.local_id).await.unwrap().unwrap();
        assert_eq!(d.body_html, "<p>v2</p>");
        assert_eq!(d.revision, 2);
        assert_eq!(d.saved_revision, 2);
    }

    #[tokio::test]
    async fn p52_stale_duplicate_remote_draft_is_reported_as_stranded() {
        let (_d, db, acc) = db().await;
        put_message(&db, &acc, "m1", "Draft", "<p>v1</p>").await;
        let local = db
            .drafts_upsert(
                &Draft {
                    account_id: acc.clone(),
                    subject: "Draft".into(),
                    body_html: "<p>v1</p>".into(),
                    remote_draft_id: Some("r1".into()),
                    rfc_message_id: Some("<m1@example.com>".into()),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        let report = db
            .drafts_reconcile_remote(
                &acc,
                &[
                    RemoteDraft {
                        remote_draft_id: "r1".into(),
                        message_id: Some("m1".into()),
                        thread_id: None,
                        rfc_message_id: Some("<m1@example.com>".into()),
                    },
                    RemoteDraft {
                        remote_draft_id: "r2".into(),
                        message_id: Some("m9".into()),
                        thread_id: None,
                        rfc_message_id: Some("<m1@example.com>".into()),
                    },
                ],
            )
            .await
            .unwrap();
        assert_eq!(report.stranded, vec!["r2".to_string()]);
        assert_eq!(report.imported.len(), 0);
        let d = db.drafts_get(&local.local_id).await.unwrap().unwrap();
        assert_eq!(d.remote_draft_id.as_deref(), Some("r1"));
    }

    #[tokio::test]
    async fn p52_mark_remote_is_revision_checked() {
        let (_d, db, acc) = db().await;
        let d = db
            .drafts_upsert(
                &Draft {
                    account_id: acc.clone(),
                    subject: "s".into(),
                    ..Default::default()
                },
                None,
            )
            .await
            .unwrap();
        assert_eq!(d.revision, 1);
        assert!(db
            .drafts_mark_remote(&d.local_id, 1, "remote-1", Some("msg-1"), 3)
            .await
            .unwrap());
        let after = db.drafts_get(&d.local_id).await.unwrap().unwrap();
        assert_eq!(after.saved_revision, 1);
        assert_eq!(after.remote_revision, 3);
        assert_eq!(after.remote_draft_id.as_deref(), Some("remote-1"));

        // The user edits again, then a slow sync for revision 1 finishes.
        let edited = db
            .drafts_upsert(
                &Draft {
                    subject: "s2".into(),
                    ..after.clone()
                },
                Some(1),
            )
            .await
            .unwrap();
        assert_eq!(edited.revision, 2);
        assert!(
            !db.drafts_mark_remote(&d.local_id, 1, "remote-2", None, 4)
                .await
                .unwrap(),
            "an old task must not claim a newer revision"
        );
        let after2 = db.drafts_get(&d.local_id).await.unwrap().unwrap();
        assert_eq!(after2.remote_draft_id.as_deref(), Some("remote-1"));
        assert_eq!(after2.saved_revision, 1);
        assert!(after2.has_unsaved_revision());
    }
}
