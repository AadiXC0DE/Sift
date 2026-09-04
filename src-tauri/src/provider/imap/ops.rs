//! Outbox ops over IMAP (Phase 11 task 10).
//!
//! Every row of the spec table is covered; ops stay idempotent so the outbox
//! retry policy can replay them. Missing UIDs resolve via
//! `UID SEARCH X-GM-MSGID` first (messages that arrived via another client
//! since the last sync); a `NO` with `[NONEXISTENT]`-style text maps to
//! `AlreadyApplied`.

use super::{conn::ImapPool, folders::FolderMap};
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
    let Ok(dec) = u64::from_str_radix(message_hex, 16) else {
        return Ok(None);
    };
    for role in ["all", "trash", "junk"] {
        let Some(name) = folders.name_for_role(role) else {
            continue;
        };
        let found = {
            let mut guard = pool.worker().await?;
            let conn = guard.as_mut().expect("connected");
            conn.select(name, true).await?;
            conn.uid_search_gmmsgid(dec).await?
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

async fn select(pool: &ImapPool, folder: &str, readonly: bool) -> Result<(), SiftError> {
    let mut guard = pool.worker().await?;
    guard
        .as_mut()
        .expect("connected")
        .select(folder, readonly)
        .await?;
    Ok(())
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
    let mut guard = pool.worker().await?;
    let conn = guard.as_mut().expect("connected");
    conn.select(folder, false).await?;
    match conn.uid_store(&set, mode, values).await {
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
    let mut guard = pool.worker().await?;
    let conn = guard.as_mut().expect("connected");
    conn.select(from_folder, false).await?;
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
                // From \All (or wherever it lives) to Trash.
                let _ = select(pool, &folder, false).await;
                move_to(pool, folders, &folder, "trash", &[uid]).await?;
                moved_any = true;
                let _ = db.imap_delete_uids(account_id, &role, &[uid as i64]).await;
            } else if want_spam {
                let _ = select(pool, &folder, false).await;
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

/// Delete forever: in Trash/Junk, flag \Deleted + EXPUNGE.
pub async fn apply_delete_threads(
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
            // Prefer Trash/Junk copies; fall back to wherever it lives.
            // UID maps go stale across MOVEs (fresh UIDs), so SEARCH when empty.
            let holders = db
                .uids_for_message(account_id, &mid)
                .await
                .map_err(db_err)?;
            let (role, uid): (String, i64) = match holders
                .iter()
                .find(|(r, _)| r == "trash")
                .or_else(|| holders.iter().find(|(r, _)| r == "junk"))
                .or_else(|| holders.first())
                .cloned()
            {
                Some(v) => v,
                None => {
                    // SEARCH trash/junk/all for the current copy.
                    let Ok(dec) = u64::from_str_radix(&mid, 16) else {
                        continue;
                    };
                    let mut found: Option<(String, i64)> = None;
                    for r in ["trash", "junk", "all"] {
                        let Some(name) = folders.name_for_role(r) else {
                            continue;
                        };
                        let uids = {
                            let mut guard = pool.worker().await?;
                            let conn = guard.as_mut().expect("connected");
                            conn.select(name, true).await?;
                            conn.uid_search_gmmsgid(dec).await.unwrap_or_default()
                        };
                        if let Some(u) = uids.into_iter().next() {
                            found = Some((r.to_string(), u as i64));
                            break;
                        }
                    }
                    let Some(v) = found else { continue };
                    v
                }
            };
            let folder = folders
                .name_for_role(&role)
                .unwrap_or(&folders.trash)
                .to_string();
            let mut guard = pool.worker().await?;
            let conn = guard.as_mut().expect("connected");
            conn.select(&folder, false).await?;
            let set = super::message::uid_set(&[uid as u32], 1);
            let _ = conn
                .uid_store(&set, "+FLAGS", &["\\Deleted".to_string()])
                .await;
            match conn.uid_expunge(Some(&set)).await {
                Ok(()) => {}
                Err(SiftError::App { message, .. }) if already_applied(&message) => {}
                Err(e) => return Err(e),
            }
            let _ = db.imap_delete_uids(account_id, &role, &[uid]).await;
        }
    }
    Ok(ApplyOutcome::Done)
}

/// Draft upsert: APPEND to Drafts with (\Draft \Seen), return hex msgid,
/// expunge the previous draft UID. Returns the new remote draft id.
pub async fn draft_upsert(
    pool: &ImapPool,
    db: &Db,
    folders: &FolderMap,
    account_id: &str,
    prev_remote: Option<&str>,
    raw: &[u8],
) -> Result<String, SiftError> {
    let drafts = folders
        .name_for_role("drafts")
        .unwrap_or(&folders.all)
        .to_string();
    let (uidvalidity, uid) = {
        let mut guard = pool.worker().await?;
        let conn = guard.as_mut().expect("connected");
        conn.append(&drafts, &["\\Draft".to_string(), "\\Seen".to_string()], raw)
            .await?
    };
    let _ = uidvalidity;
    // Resolve the new UID to its Gmail msgid.
    let msgid = {
        let mut guard = pool.worker().await?;
        let conn = guard.as_mut().expect("connected");
        conn.select(&drafts, true).await?;
        let fetched = conn.uid_fetch(&uid.to_string(), "(X-GM-MSGID)").await?;
        fetched
            .into_iter()
            .find_map(|(_, attrs)| {
                attrs.into_iter().find_map(|a| match a {
                    super::proto::FetchAttr::GmailMsgId(id) => Some(id),
                    _ => None,
                })
            })
            .ok_or_else(|| SiftError::app("imap_protocol", "APPEND without X-GM-MSGID", true))?
    };
    let hex = format!("{msgid:x}");
    let _ = db
        .imap_put_uids(account_id, "drafts", &[(uid as i64, hex.clone())])
        .await;
    // Delete the previous draft revision.
    if let Some(prev) = prev_remote {
        let _ = draft_delete(pool, db, folders, account_id, prev).await;
    }
    Ok(hex)
}

/// Draft delete: flag \Deleted + EXPUNGE in Drafts (best effort).
pub async fn draft_delete(
    pool: &ImapPool,
    db: &Db,
    folders: &FolderMap,
    account_id: &str,
    remote_id: &str,
) -> Result<(), SiftError> {
    // remote_id is hex msgid; resolve to draft UID.
    let holders = db
        .uids_for_message(account_id, remote_id)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    // Prefer the drafts copy; fall back to a Gmail-id search in Drafts.
    let uid: Option<u32> = holders
        .iter()
        .find(|(r, _)| r == "drafts")
        .map(|(_, u)| *u as u32)
        .or_else(|| holders.first().map(|(_, u)| *u as u32));
    let drafts = folders
        .name_for_role("drafts")
        .unwrap_or(&folders.all)
        .to_string();
    let uid = match uid {
        Some(u) => u,
        None => {
            let Ok(dec) = u64::from_str_radix(remote_id, 16) else {
                return Ok(());
            };
            let mut guard = pool.worker().await?;
            let conn = guard.as_mut().expect("connected");
            conn.select(&drafts, false).await?;
            match conn.uid_search_gmmsgid(dec).await {
                Ok(v) => v.into_iter().next().unwrap_or(0),
                Err(_) => return Ok(()),
            }
        }
    };
    if uid == 0 {
        return Ok(());
    }
    let mut guard = pool.worker().await?;
    let conn = guard.as_mut().expect("connected");
    conn.select(&drafts, false).await?;
    let set = super::message::uid_set(&[uid], 1);
    let _ = conn
        .uid_store(&set, "+FLAGS", &["\\Deleted".to_string()])
        .await;
    let _ = conn.uid_expunge(Some(&set)).await;
    let _ = db
        .imap_delete_uids(account_id, "drafts", &[uid as i64])
        .await;
    Ok(())
}
