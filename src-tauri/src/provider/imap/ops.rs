//! Outbox ops over IMAP (Phase 11 task 10).
//!
//! Every row of the spec table is covered; ops stay idempotent so the outbox
//! retry policy can replay them. Missing UIDs resolve via
//! `UID SEARCH X-GM-MSGID` first (messages that arrived via another client
//! since the last sync); a `NO` with `[NONEXISTENT]`-style text maps to
//! `AlreadyApplied`.

use super::{conn::ImapPool, folders::FolderMap, ids};
use crate::db::Db;
use crate::errors::SiftError;
use crate::provider::ApplyOutcome;
use std::collections::{HashMap, HashSet};

fn already_applied(text: &str) -> bool {
    let t = text.to_uppercase();
    t.contains("NONEXISTENT")
        || t.contains("NO SUCH MESSAGE")
        || t.contains("UNKNOWN MESSAGE")
        || t.contains("INVALID MESSAGE")
        || t.contains("NO SUCH UID")
}

fn imap_label_arg(label: &str) -> String {
    // System labels travel with their backslash form inside X-GM-LABELS;
    // user labels are quoted when they contain spaces.
    if label.starts_with('\\') {
        return label.to_string();
    }
    match label {
        "INBOX" => "\\Inbox".to_string(),
        "SENT" => "\\Sent".to_string(),
        "DRAFT" => "\\Draft".to_string(),
        "STARRED" => "\\Starred".to_string(),
        "IMPORTANT" => "\\Important".to_string(),
        "TRASH" => "\\Trash".to_string(),
        "SPAM" => "\\Spam".to_string(),
        _ => label.to_string(),
    }
}

fn quote_label(label: &str) -> String {
    let arg = imap_label_arg(label);
    if arg.starts_with('\\') || (!arg.contains(' ') && !arg.contains('"')) {
        arg
    } else {
        format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
    }
}

/// Resolve a message to (role, folder server name, uid), preferring `\All`.
/// Falls back to `UID SEARCH X-GM-MSGID` per folder and records the hit.
pub async fn locate(
    pool: &ImapPool,
    db: &Db,
    folders: &FolderMap,
    account_id: &str,
    message_hex: &str,
) -> Result<Option<(String, String, u32)>, SiftError> {
    let db_err = |e: anyhow::Error| SiftError::app("db", e.to_string(), false);
    let known = db
        .uids_for_message(account_id, message_hex)
        .await
        .map_err(db_err)?;
    let pick = known
        .iter()
        .find(|(r, _)| r == "all")
        .or_else(|| known.first());
    if let Some((role, uid)) = pick {
        if let Some(name) = folders.name_for_role(role) {
            return Ok(Some((role.clone(), name.to_string(), *uid as u32)));
        }
    }
    // Unknown locally: search every folder by numeric Gmail id.
    let Some(dec) = ids::from_hex(message_hex) else {
        return Ok(None);
    };
    for role in ["all", "trash", "junk"] {
        let Some(name) = folders.name_for_role(role) else {
            continue;
        };
        let found = {
            // The SELECT and the SEARCH that depends on it share one lease
            // (P4.3).
            let mut w = pool.with_selected_worker(name, true, &no_cancel()).await?;
            w.conn().uid_search_gmmsgid(dec).await?
        };
        if let Some(uid) = found.into_iter().next() {
            let _ = db
                .imap_put_uids(account_id, role, &[(uid as i64, message_hex.into())])
                .await;
            return Ok(Some((role.into(), name.into(), uid)));
        }
    }
    Ok(None)
}

/// Ops run under the account generation's own cancellation (the runtime drops
/// the whole future); the mailbox lease itself needs no second token.
fn no_cancel() -> tokio_util::sync::CancellationToken {
    tokio_util::sync::CancellationToken::new()
}

async fn store(
    pool: &ImapPool,
    folder: &str,
    uids: &[u32],
    mode: &str,
    values: &[String],
) -> Result<bool, SiftError> {
    if uids.is_empty() || values.is_empty() {
        return Ok(true);
    }
    let set = super::message::uid_set(uids, uids.len());
    let mut w = pool
        .with_selected_worker(folder, false, &no_cancel())
        .await?;
    match w.conn().uid_store(&set, mode, values).await {
        Ok(()) => Ok(true),
        Err(SiftError::App { message, .. }) if already_applied(&message) => Ok(false),
        Err(e) => Err(e),
    }
}

async fn move_to(
    pool: &ImapPool,
    folders: &FolderMap,
    from_folder: &str,
    dest_role: &str,
    uids: &[u32],
) -> Result<bool, SiftError> {
    if uids.is_empty() {
        return Ok(true);
    }
    let Some(dest) = folders.name_for_role(dest_role) else {
        return Err(SiftError::app(
            "imap_protocol",
            "unknown folder role",
            false,
        ));
    };
    let set = super::message::uid_set(uids, uids.len());
    let caps = pool.caps().await;
    let mut w = pool
        .with_selected_worker(from_folder, false, &no_cancel())
        .await?;
    let conn = w.conn();
    if caps.mov {
        match conn.uid_move(&set, dest).await {
            Ok(()) => return Ok(true),
            Err(SiftError::App { message, .. }) if already_applied(&message) => return Ok(false),
            Err(e) => return Err(e),
        }
    }
    // Fallback when MOVE is missing: COPY + \Deleted + EXPUNGE.
    match conn.uid_copy(&set, dest).await {
        Ok(()) => {}
        Err(SiftError::App { message, .. }) if already_applied(&message) => return Ok(false),
        Err(e) => return Err(e),
    }
    let _ = conn
        .uid_store(&set, "+FLAGS", &["\\Deleted".to_string()])
        .await;
    let _ = conn.uid_expunge(Some(&set)).await;
    Ok(true)
}

/// Ensure a user label exists server-side (CREATE is idempotent; ALREADYEXISTS
/// is success). System labels and flag-mapped labels never reach here.
async fn ensure_label(pool: &ImapPool, name: &str) -> Result<(), SiftError> {
    if matches!(
        name,
        "INBOX" | "SENT" | "DRAFT" | "STARRED" | "IMPORTANT" | "TRASH" | "SPAM" | "UNREAD"
    ) {
        return Ok(());
    }
    // CREATE is not selected-state: hold the plain worker lease.
    let mut guard = pool.worker().await?;
    let conn = guard.as_mut().expect("connected");
    match conn.create(name).await {
        Ok(()) => Ok(()),
        Err(SiftError::App { message, .. })
            if message.to_uppercase().contains("ALREADYEXISTS")
                || message.to_uppercase().contains("ALREADY EXISTS") =>
        {
            Ok(())
        }
        Err(e) => Err(e),
    }
}

/// Generic triage mutation from the outbox `modify_labels` shape.
/// Handles archive, trash/spam (MOVE), read/star/labels (STORE) per message,
/// grouped by folder for one SELECT per folder per operation class.
pub async fn apply_modify_labels(
    pool: &ImapPool,
    db: &Db,
    folders: &FolderMap,
    account_id: &str,
    ids: &[String],
    add: &[String],
    remove: &[String],
) -> Result<ApplyOutcome, SiftError> {
    if ids.is_empty() {
        return Ok(ApplyOutcome::Done);
    }
    let add_set: HashSet<&str> = add.iter().map(|s| s.as_str()).collect();
    let remove_set: HashSet<&str> = remove.iter().map(|s| s.as_str()).collect();

    let want_trash = add_set.contains("TRASH");
    let want_spam = add_set.contains("SPAM");
    let want_untrash = remove_set.contains("TRASH") && add_set.contains("INBOX");
    let want_unspam = remove_set.contains("SPAM") && add_set.contains("INBOX");

    // TRASH / SPAM / UNTRASH / UNSPAM take the MOVE path per message.
    if want_trash || want_spam || want_untrash || want_unspam {
        let mut any = false;
        let mut moved_any = false;
        for mid in ids {
            let Some((role, folder, uid)) = locate(pool, db, folders, account_id, mid).await?
            else {
                continue; // already gone → idempotent success
            };
            if want_trash && role == "trash" {
                continue;
            }
            if want_spam && role == "junk" {
                continue;
            }
            any = true;
            if want_trash {
                // From \All (or wherever it lives) to Trash; `move_to` leases
                // the connection and SELECTs the source inside the lease.
                move_to(pool, folders, &folder, "trash", &[uid]).await?;
                moved_any = true;
                let _ = db.imap_delete_uids(account_id, &role, &[uid as i64]).await;
            } else if want_spam {
                move_to(pool, folders, &folder, "junk", &[uid]).await?;
                moved_any = true;
                let _ = db.imap_delete_uids(account_id, &role, &[uid as i64]).await;
            } else if want_untrash {
                // In \Trash → MOVE to \All, then restore INBOX (payload may
                // carry extra labels; those fall through to STORE below).
                let trash_name = folders
                    .name_for_role("trash")
                    .unwrap_or(&folder)
                    .to_string();
                let from = if role == "trash" {
                    folder.clone()
                } else {
                    trash_name.clone()
                };
                // Best effort: the message may already be back in \All.
                let moved = move_to(pool, folders, &from, "all", &[uid])
                    .await
                    .unwrap_or(false);
                if moved {
                    moved_any = true;
                }
                let _ = db.imap_delete_uids(account_id, &role, &[uid as i64]).await;
                // Fall through to STORE the remaining add/remove in \All.
                let all_name = folders.name_for_role("all").unwrap_or(&from).to_string();
                let rest_add: Vec<String> = add
                    .iter()
                    .filter(|l| *l != "INBOX" && *l != "TRASH")
                    .cloned()
                    .collect();
                let rest_remove: Vec<String> = remove
                    .iter()
                    .filter(|l| *l != "INBOX" && *l != "TRASH")
                    .cloned()
                    .collect();
                // Re-resolve UID in \All (MOVE assigns a fresh UID).
                if let Some((_, all_folder, new_uid)) =
                    locate(pool, db, folders, account_id, mid).await?
                {
                    let _ = (all_name, all_folder);
                    apply_store_labels(pool, folders, &new_uid, mid, &rest_add, &rest_remove)
                        .await?;
                    // Ensure INBOX is present after untrash.
                    let _ = store(
                        pool,
                        &folders.all,
                        &[new_uid],
                        "+X-GM-LABELS",
                        &["\\Inbox".to_string()],
                    )
                    .await;
                }
                continue;
            } else if want_unspam {
                let junk_name = folders.name_for_role("junk").unwrap_or(&folder).to_string();
                let from = if role == "junk" {
                    folder.clone()
                } else {
                    junk_name.clone()
                };
                let _ = move_to(pool, folders, &from, "all", &[uid])
                    .await
                    .unwrap_or(false);
                let _ = db.imap_delete_uids(account_id, &role, &[uid as i64]).await;
                if let Some((_, _, new_uid)) = locate(pool, db, folders, account_id, mid).await? {
                    let _ = store(
                        pool,
                        &folders.all,
                        &[new_uid],
                        "+X-GM-LABELS",
                        &["\\Inbox".to_string()],
                    )
                    .await;
                }
                continue;
            }
        }
        if !any {
            return Ok(ApplyOutcome::AlreadyApplied);
        }
        if !moved_any {
            return Ok(ApplyOutcome::AlreadyApplied);
        }
        return Ok(ApplyOutcome::Done);
    }

    // Non-move path: group UIDs by folder, one SELECT per folder.
    let mut by_folder: HashMap<String, Vec<(String, u32)>> = HashMap::new();
    for mid in ids {
        if let Some((_, folder, uid)) = locate(pool, db, folders, account_id, mid).await? {
            by_folder
                .entry(folder)
                .or_default()
                .push((mid.clone(), uid));
        }
    }
    if by_folder.is_empty() {
        return Ok(ApplyOutcome::AlreadyApplied);
    }
    // CREATE unknown user labels first (idempotent).
    for label in add.iter().chain(remove.iter()) {
        if !matches!(
            label.as_str(),
            "INBOX" | "SENT" | "DRAFT" | "STARRED" | "IMPORTANT" | "TRASH" | "SPAM" | "UNREAD"
        ) {
            let _ = ensure_label(pool, label).await;
        }
    }
    for (folder, pairs) in &by_folder {
        let uids: Vec<u32> = pairs.iter().map(|(_, u)| *u).collect();
        // Split add/remove into FLAG vs X-GM-LABELS operations.
        let mut flag_add: Vec<String> = vec![];
        let mut flag_remove: Vec<String> = vec![];
        let mut gm_add: Vec<String> = vec![];
        let mut gm_remove: Vec<String> = vec![];
        for l in add {
            match l.as_str() {
                "STARRED" => flag_add.push("\\Flagged".to_string()),
                "UNREAD" => flag_remove.push("\\Seen".to_string()),
                "INBOX" | "SENT" | "DRAFT" | "IMPORTANT" => gm_add.push(quote_label(l)),
                "TRASH" | "SPAM" => gm_add.push(quote_label(l)),
                _ => gm_add.push(quote_label(l)),
            }
        }
        for l in remove {
            match l.as_str() {
                "STARRED" => flag_remove.push("\\Flagged".to_string()),
                "UNREAD" => flag_add.push("\\Seen".to_string()),
                "INBOX" | "SENT" | "DRAFT" | "IMPORTANT" => gm_remove.push(quote_label(l)),
                "TRASH" | "SPAM" => gm_remove.push(quote_label(l)),
                _ => gm_remove.push(quote_label(l)),
            }
        }
        if !flag_add.is_empty() {
            let _ = store(pool, folder, &uids, "+FLAGS", &flag_add).await?;
        }
        if !flag_remove.is_empty() {
            let _ = store(pool, folder, &uids, "-FLAGS", &flag_remove).await?;
        }
        if !gm_add.is_empty() {
            let _ = store(pool, folder, &uids, "+X-GM-LABELS", &gm_add).await?;
        }
        if !gm_remove.is_empty() {
            let _ = store(pool, folder, &uids, "-X-GM-LABELS", &gm_remove).await?;
        }
    }
    Ok(ApplyOutcome::Done)
}

async fn apply_store_labels(
    pool: &ImapPool,
    folders: &FolderMap,
    uid: &u32,
    _mid: &str,
    add: &[String],
    remove: &[String],
) -> Result<(), SiftError> {
    if add.is_empty() && remove.is_empty() {
        return Ok(());
    }
    for label in add.iter().chain(remove.iter()) {
        if !matches!(
            label.as_str(),
            "INBOX" | "SENT" | "DRAFT" | "STARRED" | "IMPORTANT" | "TRASH" | "SPAM" | "UNREAD"
        ) {
            let _ = ensure_label(pool, label).await;
        }
    }
    let gm_add: Vec<String> = add.iter().map(|l| quote_label(l)).collect();
    let gm_remove: Vec<String> = remove.iter().map(|l| quote_label(l)).collect();
    if !gm_add.is_empty() {
        let _ = store(pool, &folders.all, &[*uid], "+X-GM-LABELS", &gm_add).await?;
    }
    if !gm_remove.is_empty() {
        let _ = store(pool, &folders.all, &[*uid], "-X-GM-LABELS", &gm_remove).await?;
    }
    Ok(())
}

/// Thread-based trash: every message in each thread moves to Trash.
pub async fn apply_trash_threads(
    pool: &ImapPool,
    db: &Db,
    folders: &FolderMap,
    account_id: &str,
    threads: &[String],
) -> Result<ApplyOutcome, SiftError> {
    if threads.is_empty() {
        return Ok(ApplyOutcome::Done);
    }
    let db_err = |e: anyhow::Error| SiftError::app("db", e.to_string(), false);
    for tid in threads {
        let mids: Vec<String> = db
            .read({
                let (a, t) = (account_id.to_string(), tid.clone());
                move |c| {
                    Ok(
                        c.prepare("SELECT id FROM messages WHERE account_id=? AND thread_id=?")?
                            .query_map(rusqlite::params![a, t], |r| r.get(0))?
                            .collect::<Result<Vec<String>, _>>()?,
                    )
                }
            })
            .await
            .map_err(db_err)?;
        for mid in mids {
            if let Some((role, folder, uid)) = locate(pool, db, folders, account_id, &mid).await? {
                if role == "trash" {
                    continue;
                }
                let _ = move_to(pool, folders, &folder, "trash", &[uid]).await;
                let _ = db.imap_delete_uids(account_id, &role, &[uid as i64]).await;
            }
        }
    }
    Ok(ApplyOutcome::Done)
}

/// One message named by an immutable identity, captured *before* its display
/// rows were removed (P6.4).
///
/// The local id is the stable Gmail identity (hex, converted to decimal at the
/// IMAP boundary); `uid`/`uidvalidity`/`folder` are only hints recorded at
/// gesture time, and are used solely when the epoch still matches.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct DeleteTarget {
    pub id: String,
    #[serde(default, rename = "rfcMessageId")]
    pub rfc_message_id: Option<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub folder: Option<String>,
    #[serde(default)]
    pub uid: Option<u32>,
    #[serde(default)]
    pub uidvalidity: Option<u32>,
}

/// Permanent deletion, addressed by identity rather than by thread (P6.4).
///
/// `DeleteForever` removes the local rows as soon as the user confirms, so the
/// old implementation — which looked the messages up from the thread at drain
/// time — could find nothing and delete nothing remotely. Here the op carries
/// the message identities, the provider resolves each one's *current* locator
/// before deleting, verifies that the message is really in Trash/Junk, and
/// requires UIDPLUS for the targeted `UID EXPUNGE` that removes exactly those
/// messages.
pub async fn apply_delete_messages(
    pool: &ImapPool,
    db: &Db,
    folders: &FolderMap,
    account_id: &str,
    targets: &[DeleteTarget],
) -> Result<ApplyOutcome, SiftError> {
    if targets.is_empty() {
        return Ok(ApplyOutcome::Done);
    }
    // A targeted EXPUNGE is the only safe form: a broad EXPUNGE would take the
    // other client's \Deleted messages with it.
    if !pool.caps().await.uidplus {
        return Err(SiftError::app(
            "unsupported_operation",
            "This Gmail account does not advertise UIDPLUS, so Sift cannot remove exactly the messages you confirmed.",
            false,
        ));
    }
    let mut expunged = 0usize;
    let mut gone = 0usize;
    for target in targets {
        let located = resolve_for_delete(pool, folders, target).await?;
        let Some(located) = located else {
            // Searched by identity in every folder that could hold it and found
            // nothing: it really is already gone.
            gone += 1;
            continue;
        };
        match located {
            DeleteLocation::InTrash { role, folder, uid } => {
                let set = super::message::uid_set(&[uid], 1);
                {
                    let mut w = pool
                        .with_selected_worker(&folder, false, &no_cancel())
                        .await?;
                    // Flag and targeted EXPUNGE errors propagate: a deletion
                    // that did not happen must never be reported as done, and
                    // it stays visible so the user can retry it.
                    match w
                        .conn()
                        .uid_store(&set, "+FLAGS", &["\\Deleted".to_string()])
                        .await
                    {
                        Ok(()) => {}
                        Err(SiftError::App { message, .. }) if already_applied(&message) => {
                            gone += 1;
                            continue;
                        }
                        Err(e) => return Err(e),
                    }
                    match w.conn().uid_expunge(Some(&set)).await {
                        Ok(()) => {}
                        Err(SiftError::App { message, .. }) if already_applied(&message) => {}
                        Err(e) => return Err(e),
                    }
                }
                let _ = db.imap_delete_uids(account_id, &role, &[uid as i64]).await;
                expunged += 1;
            }
            DeleteLocation::OutsideTrash => {
                // Per-message membership, not a thread aggregate. The message
                // is somewhere it must not be permanently deleted from, so it
                // is left alone and the user is told why.
                return Err(SiftError::app(
                    "delete_target_moved",
                    "A message you confirmed for permanent deletion is no longer in Trash or Spam, so Sift left it alone.",
                    false,
                ));
            }
        }
    }
    if expunged == 0 && gone > 0 {
        return Ok(ApplyOutcome::AlreadyApplied);
    }
    Ok(ApplyOutcome::Done)
}

/// Where a message that is being deleted actually is, right now.
enum DeleteLocation {
    InTrash {
        role: String,
        folder: String,
        uid: u32,
    },
    /// It exists, but not where it may be permanently deleted from.
    OutsideTrash,
}

/// Resolve the *current* locator of a message that is being deleted (P6.4).
///
/// The recorded location is only a hint, and it is verified by a search before
/// it is used: a UID from an older epoch, or one whose message was moved by
/// another client, would otherwise be used to expunge whatever now holds it —
/// or, worse, to report a deletion that never happened. Trash and Junk are
/// searched before \All because on Gmail a trashed message is in both.
async fn resolve_for_delete(
    pool: &ImapPool,
    folders: &FolderMap,
    target: &DeleteTarget,
) -> Result<Option<DeleteLocation>, SiftError> {
    let Some(dec) = ids::from_hex(&target.id) else {
        // Not a Gmail identity: fall back to the recorded locator, which is all
        // this payload carries.
        return Ok(match (target.role.clone(), target.uid) {
            (Some(role), Some(uid)) if role == "trash" || role == "junk" => folders
                .name_for_role(&role)
                .map(|folder| DeleteLocation::InTrash {
                    role,
                    folder: folder.to_string(),
                    uid,
                }),
            (Some(_), Some(_)) => Some(DeleteLocation::OutsideTrash),
            _ => None,
        });
    };
    // Trash first, then Junk, then \All: the order is what makes "is it still
    // in Trash?" the question we answer.
    let mut outside = false;
    for role in ["trash", "junk", "all"] {
        let Some(name) = folders.name_for_role(role).map(str::to_string) else {
            continue;
        };
        let uids = {
            let mut w = pool.with_selected_worker(&name, true, &no_cancel()).await?;
            w.conn().uid_search_gmmsgid(dec).await?
        };
        let Some(uid) = uids.first().copied() else {
            continue;
        };
        if role == "all" {
            outside = true;
            continue;
        }
        return Ok(Some(DeleteLocation::InTrash {
            role: role.to_string(),
            folder: name,
            uid,
        }));
    }
    if outside {
        return Ok(Some(DeleteLocation::OutsideTrash));
    }
    Ok(None)
}

/// Targets for a permanent deletion, from an outbox `delete` payload.
///
/// The current shape is the immutable identity list written before the local
/// rows were removed. A payload left by the pre-P6.4 build only has thread
/// ids: those messages (if their rows still exist) are read once, up front, so
/// the operation still names exact messages instead of asking the database
/// during the delete.
pub async fn delete_targets_from_payload(
    db: &Db,
    account_id: &str,
    payload: &serde_json::Value,
) -> Result<Vec<DeleteTarget>, SiftError> {
    if payload.get("messages").and_then(|m| m.as_array()).is_some() {
        return serde_json::from_value(payload["messages"].clone()).map_err(|e| {
            SiftError::app(
                "payload_invalid",
                format!("the deletion list could not be read: {e}"),
                false,
            )
        });
    }
    let threads: Vec<String> =
        serde_json::from_value(payload["threads"].clone()).unwrap_or_default();
    if threads.is_empty() {
        return Ok(Vec::new());
    }
    let db_err = |e: anyhow::Error| SiftError::app("db", e.to_string(), false);
    let mut out = Vec::new();
    for tid in threads {
        let ids: Vec<String> = db
            .read({
                let (a, t) = (account_id.to_string(), tid.clone());
                move |c| {
                    Ok(
                        c.prepare("SELECT id FROM messages WHERE account_id=? AND thread_id=?")?
                            .query_map(rusqlite::params![a, t], |r| r.get(0))?
                            .collect::<Result<Vec<String>, _>>()?,
                    )
                }
            })
            .await
            .map_err(db_err)?;
        for id in ids {
            out.push(DeleteTarget {
                id,
                ..Default::default()
            });
        }
    }
    Ok(out)
}

/// Reconcile an `uncertain` send against the server's own index (P6.1).
///
/// The prepared message carries a stable RFC Message-ID; when a copy of that
/// message exists in Sent, the operation is provably done. An empty result is
/// *not* proof of non-delivery — the caller keeps the operation uncertain and
/// re-checks on the bounded schedule.
pub async fn sent_by_rfc_message_id(
    pool: &ImapPool,
    folders: &FolderMap,
    rfc_message_id: &str,
) -> Result<Option<(String, String)>, SiftError> {
    let wanted = crate::outgoing::bare_message_id(rfc_message_id).to_string();
    if wanted.is_empty() {
        return Ok(None);
    }
    for role in ["sent", "all"] {
        let Some(name) = folders.name_for_role(role).map(str::to_string) else {
            continue;
        };
        let uids = {
            let mut w = pool.with_selected_worker(&name, true, &no_cancel()).await?;
            w.conn().uid_search_header("Message-ID", &wanted).await?
        };
        let Some(uid) = uids.first().copied() else {
            continue;
        };
        let hex = resolve_hex(pool, &name, uid).await;
        return Ok(Some((
            if hex.is_empty() {
                format!("{role}:{uid}")
            } else {
                hex
            },
            String::new(),
        )));
    }
    Ok(None)
}

/// Stable locator for an IMAP draft: `uidvalidity:uid:hex-message-id`.
///
/// A UID is only meaningful inside its UIDVALIDITY epoch, so the epoch travels
/// with it. When the epoch later differs, the draft is re-located by its stable
/// Gmail message id instead of expunging whatever now holds that UID (P1.3/P5.2:
/// an old UID must never address a different message).
pub fn encode_draft_locator(uidvalidity: u32, uid: u32, hex_msgid: &str) -> String {
    format!("{uidvalidity}:{uid}:{hex_msgid}")
}

/// Decode a draft locator. Anything else (an older build stored a bare hex
/// message id) returns `None`, and callers fall back to id-based lookup.
pub fn decode_draft_locator(s: &str) -> Option<(u32, u32, String)> {
    let mut parts = s.splitn(3, ':');
    let uidvalidity = parts.next()?.parse::<u32>().ok()?;
    let uid = parts.next()?.parse::<u32>().ok()?;
    let hex = parts.next().unwrap_or_default().to_string();
    Some((uidvalidity, uid, hex))
}

/// Draft upsert: APPEND to Drafts with (\Draft \Seen), resolve the copy to a
/// stable locator, then delete the previous revision.
///
/// Ordering is deliberate: the replacement is appended **before** the previous
/// draft is deleted, so an interruption can only leave a duplicate, never
/// nothing. An ambiguous APPEND (the connection dropped, or the server
/// completed the transfer without APPENDUID) is reconciled by the Message-ID
/// the message was built with — stable for the draft's lineage — so a retry
/// adopts the copy that already landed instead of appending a second one.
pub async fn draft_upsert(
    pool: &ImapPool,
    db: &Db,
    folders: &FolderMap,
    account_id: &str,
    prev_remote: Option<&str>,
    raw: &[u8],
    rfc_message_id: &str,
) -> Result<crate::dto::RemoteDraft, SiftError> {
    let drafts = folders
        .name_for_role("drafts")
        .unwrap_or(&folders.all)
        .to_string();
    let appended = {
        let mut guard = pool.worker().await?;
        let conn = guard.as_mut().expect("connected");
        conn.append(&drafts, &["\\Draft".to_string(), "\\Seen".to_string()], raw)
            .await
    };
    let (uidvalidity, uid, leftovers) = match appended {
        Ok((uidvalidity, uid)) => (Some(uidvalidity), uid, Vec::new()),
        Err(e) => {
            // The APPEND may or may not have landed. Reconcile by Message-ID
            // before deciding: adopting a landed copy is correct, and if
            // nothing landed the error propagates and the outbox retries.
            log::warn!("IMAP draft APPEND was ambiguous ({e}); reconciling by Message-ID");
            let copies = find_draft_copies(pool, &drafts, rfc_message_id).await;
            match copies.split_first() {
                Some((first, rest)) => (None, *first, rest.to_vec()),
                None => {
                    return Err(SiftError::app(
                        "draft_sync_failed",
                        "Gmail did not confirm saving this draft. Sift will retry.",
                        true,
                    ))
                }
            }
        }
    };
    // One draft per lineage: a copy left behind by an earlier ambiguous APPEND
    // is expunged now that this revision is confirmed.
    for other in leftovers {
        let _ = expunge_uid(pool, db, account_id, &drafts, other).await;
    }

    let hex = resolve_hex(pool, &drafts, uid).await;
    let _ = db
        .imap_put_uids(account_id, "drafts", &[(uid as i64, hex.clone())])
        .await;
    if let Some(prev) = prev_remote {
        let _ = draft_delete(pool, db, folders, account_id, prev).await;
    }
    Ok(crate::dto::RemoteDraft {
        remote_draft_id: encode_draft_locator(uidvalidity.unwrap_or_default(), uid, &hex),
        message_id: (!hex.is_empty()).then(|| hex.clone()),
        thread_id: None,
        rfc_message_id: Some(rfc_message_id.to_string()),
    })
}

/// UIDs of draft copies carrying this Message-ID in the Drafts mailbox.
async fn find_draft_copies(pool: &ImapPool, drafts: &str, rfc_message_id: &str) -> Vec<u32> {
    let id = crate::outgoing::bare_message_id(rfc_message_id).to_string();
    if id.is_empty() {
        return Vec::new();
    }
    let Ok(mut w) = pool.with_selected_worker(drafts, true, &no_cancel()).await else {
        return Vec::new();
    };
    w.conn()
        .uid_search_raw(&format!("HEADER Message-ID \"<{id}>\""))
        .await
        .unwrap_or_default()
}

/// The Gmail message id of a freshly appended draft, which lets the locator
/// re-locate it after a UIDVALIDITY change. Empty when the server has no
/// X-GM-MSGID (a non-Gmail server); the Message-ID is then the only key.
async fn resolve_hex(pool: &ImapPool, drafts: &str, uid: u32) -> String {
    let Ok(mut w) = pool.with_selected_worker(drafts, true, &no_cancel()).await else {
        return String::new();
    };
    let Ok(fetched) = w
        .conn()
        .uid_fetch_items(
            &uid.to_string(),
            &super::conn::FetchItems::new().gmail_msgid(),
        )
        .await
    else {
        return String::new();
    };
    fetched
        .into_iter()
        .find_map(|(_, attrs)| {
            attrs.into_iter().find_map(|a| match a {
                super::proto::FetchAttr::GmailMsgId(id) => Some(id),
                _ => None,
            })
        })
        .map(|id| format!("{id:x}"))
        .unwrap_or_default()
}

/// Flag and expunge one draft UID in the Drafts mailbox.
async fn expunge_uid(
    pool: &ImapPool,
    db: &Db,
    account_id: &str,
    drafts: &str,
    uid: u32,
) -> Result<(), SiftError> {
    let mut w = pool
        .with_selected_worker(drafts, false, &no_cancel())
        .await?;
    let conn = w.conn();
    let set = super::message::uid_set(&[uid], 1);
    let _ = conn
        .uid_store(&set, "+FLAGS", &["\\Deleted".to_string()])
        .await;
    match conn.uid_expunge(Some(&set)).await {
        Ok(()) => {}
        Err(SiftError::App { message, .. }) if already_applied(&message) => {}
        Err(e) => return Err(e),
    }
    let _ = db
        .imap_delete_uids(account_id, "drafts", &[uid as i64])
        .await;
    Ok(())
}

/// Draft delete: expunge the draft's copy in the Drafts mailbox.
///
/// The locator's UID is used only while its UIDVALIDITY epoch still matches the
/// folder; otherwise the draft is re-located by its stable Gmail message id. A
/// draft that is already gone is a success.
pub async fn draft_delete(
    pool: &ImapPool,
    db: &Db,
    folders: &FolderMap,
    account_id: &str,
    remote_id: &str,
) -> Result<(), SiftError> {
    let drafts = folders
        .name_for_role("drafts")
        .unwrap_or(&folders.all)
        .to_string();
    let locator = decode_draft_locator(remote_id);
    let epoch = db
        .imap_get_folder(account_id, "drafts")
        .await
        .ok()
        .flatten()
        .map(|c| c.uidvalidity);
    // 1. The locator's UID, while its epoch is the one the folder currently has.
    let mut uid = match &locator {
        Some((uidvalidity, uid, _)) if epoch == Some(*uidvalidity as i64) => Some(*uid),
        _ => None,
    };
    // 2. A copy the uid map knows about.
    if uid.is_none() {
        let holders = db
            .uids_for_message(account_id, remote_id)
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        uid = holders
            .iter()
            .find(|(r, _)| r == "drafts")
            .map(|(_, u)| *u as u32);
    }
    // 3. Re-locate by the stable Gmail message id.
    if uid.is_none() {
        let hex = locator
            .as_ref()
            .map(|(_, _, hex)| hex.clone())
            .filter(|h| !h.is_empty())
            .or_else(|| (!remote_id.contains(':')).then(|| remote_id.to_string()));
        if let Some(dec) = hex.and_then(|h| ids::from_hex(&h)) {
            let mut w = pool
                .with_selected_worker(&drafts, false, &no_cancel())
                .await?;
            uid = w
                .conn()
                .uid_search_gmmsgid(dec)
                .await
                .ok()
                .and_then(|v| v.into_iter().next());
        }
    }
    match uid {
        Some(u) => expunge_uid(pool, db, account_id, &drafts, u).await,
        // Already deleted elsewhere: nothing to do, and nothing to report.
        None => Ok(()),
    }
}
