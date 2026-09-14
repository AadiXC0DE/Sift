//! IMAP incremental sync (Phase 11 task 7): per folder - new messages by
//! UIDNEXT, flag/label deltas by CHANGEDSINCE (newest-2000 diff without
//! CONDSTORE, which Gmail always advertises), deletions by UID-set diff,
//! UIDVALIDITY mismatch → NeedsFull. A quiet poll is one SELECT plus one
//! CHANGEDSINCE fetch.
//!
//! P4.5 hardens the cursor: the new-message interval is resolved with one
//! `UID SEARCH UID <from>:*` (a UIDNEXT jump of a billion with two new
//! messages stays two rows), each batch of at most 500 real UIDs commits its
//! messages, attachment metadata, UID map and folder checkpoint in a single
//! transaction, and Trash/Junk departures take their labels from the server's
//! current \All state instead of inferring INBOX from All Mail membership.
use super::{
    conn::ImapPool,
    message::{self, MetaItem},
};
use crate::db::imap::FolderCursor;
use crate::dto::SyncStatus;
use crate::errors::SiftError;
use crate::provider::{Cursor, MetadataBatch, PartialOutcome, SyncSink};
use std::collections::{HashMap, HashSet};

use super::proto::{FetchAttr, GmailMeta};

const ROLLING_SCAN_UIDS: usize = 2_000;
const FULL_SCAN_INTERVAL_MS: i64 = 6 * 3_600_000;
/// Metadata FETCH width per committed batch (P4.5). Bounded so one batch's
/// transaction stays small and a failure retries a bounded amount of work.
const META_BATCH: usize = 500;

/// Run one partial tick over all/trash/junk. `prev` holds the stored folder
/// cursors; `force_uid_diff` forces the UID-set reconciliation (every 10th
/// poll per spec - the caller owns the counter).
pub async fn run_partial_sync(
    pool: &ImapPool,
    sink: &dyn SyncSink,
    account_id: &str,
    folders: &super::folders::FolderMap,
    prev: &HashMap<String, FolderCursor>,
    force_uid_diff: bool,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<PartialOutcome, SiftError> {
    let mut changed: Vec<(String, String)> = vec![];
    let mut new_inbox: Vec<crate::provider::NewMail> = vec![];
    // Deferred \All disappearances: resolved after trash/junk passes tell us
    // whether the message moved there or vanished entirely.
    let mut missing_from_all: Vec<String> = vec![];
    let mut cursors: HashMap<String, FolderCursor> = HashMap::new();

    for (role, extra) in [
        ("all", vec![]),
        ("trash", vec!["TRASH"]),
        ("junk", vec!["SPAM"]),
    ] {
        if cancel.is_cancelled() {
            return Err(SiftError::app("cancelled", "partial sync cancelled", false));
        }
        let Some(name) = folders.name_for_role(role) else {
            continue;
        };
        let Some((cur, mut fc, mut fn_, mut missing)) = sync_one_folder(
            pool,
            sink,
            account_id,
            role,
            name,
            &extra,
            prev.get(role),
            force_uid_diff,
            cancel,
        )
        .await?
        else {
            // UIDVALIDITY changed: stored UIDs are garbage, rebuild everything.
            return Ok(PartialOutcome::NeedsFull);
        };
        if role == "all" {
            missing_from_all.append(&mut missing);
        } else {
            // Untrash/unspam: gone from trash/junk but present in \All.
            let all_folder = folders.name_for_role("all").map(|s| s.to_string());
            for mid in missing {
                let holders: HashSet<String> = sink
                    .uids_for_message(account_id, &mid)
                    .await
                    .map_err(|e| SiftError::app("db", e.to_string(), false))?
                    .into_iter()
                    .map(|(r, _)| r)
                    .collect();
                if !holders.contains("all") {
                    // Left both: the \All pass already resolved (or will
                    // resolve) where it went.
                    continue;
                }
                // P4.5: the departure says nothing about where the message
                // landed. All Mail holds archived mail too, so INBOX must come
                // from the server's authoritative labels, never from
                // membership in \All. Only the old membership is dropped here.
                let old = if role == "trash" { "TRASH" } else { "SPAM" };
                let desired = match all_folder.as_deref() {
                    Some(folder) => {
                        authoritative_labels(pool, sink, account_id, folder, &mid, cancel).await?
                    }
                    None => None,
                };
                match desired {
                    Some(mut labels) => {
                        labels.retain(|l| l != "TRASH" && l != "SPAM");
                        if let Some(tid) =
                            apply_mapped_state(sink, account_id, &mid, &labels).await?
                        {
                            changed.push((account_id.to_string(), tid));
                        }
                    }
                    // No longer locatable in \All: drop only the old
                    // membership instead of guessing a destination.
                    None => {
                        let (a, t) = sink
                            .apply_label_change(
                                &crate::dto::MessageRef::new(account_id, &mid),
                                &[],
                                &[old.to_string()],
                            )
                            .await
                            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
                        changed.push((a, t));
                    }
                }
            }
        }
        cursors.insert(role.into(), cur);
        changed.append(&mut fc);
        new_inbox.append(&mut fn_);
    }

    // Resolve deferred \All disappearances now that trash/junk are current.
    for mid in missing_from_all {
        let holders: HashSet<String> = sink
            .uids_for_message(account_id, &mid)
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?
            .into_iter()
            .map(|(r, _)| r)
            .collect();
        if holders.contains("trash") {
            let (a, t) = sink
                .apply_label_change(&crate::dto::MessageRef::new(account_id, &mid), &["TRASH".to_string()], &["INBOX".to_string()])
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            changed.push((a, t));
        } else if holders.contains("junk") {
            let (a, t) = sink
                .apply_label_change(&crate::dto::MessageRef::new(account_id, &mid), &["SPAM".to_string()], &["INBOX".to_string()])
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            changed.push((a, t));
        } else if let Some(t) = sink
            .message_thread(&crate::dto::MessageRef::new(account_id, &mid))
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?
        {
            // Deleted forever (or moved out by another client): drop the row.
            sink.delete_message(&crate::dto::MessageRef::new(account_id, &mid), &t)
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            changed.push((account_id.to_string(), t));
        }
    }

    // Refresh the portable cursor snapshot (also stamps last_sync_at).
    let ordered = ["all", "trash", "junk"]
        .into_iter()
        .filter_map(|r| cursors.remove(r))
        .collect();
    let cursor = Cursor::Imap { folders: ordered };
    sink.set_history_id(account_id, &cursor.render())
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    Ok(PartialOutcome::Synced {
        changed_threads: changed,
        new_inbox,
    })
}

type FolderPass = (
    FolderCursor,
    Vec<(String, String)>,
    Vec<crate::provider::NewMail>,
    Vec<String>,
);

/// Sync one folder. Returns None on UIDVALIDITY mismatch (caller → NeedsFull).
/// Missing UIDs are removed from the uid map here; message-row disposal
/// (trash/spam/delete) happens in the caller, which sees all folders.
#[allow(clippy::too_many_arguments)]
async fn sync_one_folder(
    pool: &ImapPool,
    sink: &dyn SyncSink,
    account_id: &str,
    role: &str,
    folder: &str,
    extra_labels: &[&str],
    prev: Option<&FolderCursor>,
    force_uid_diff: bool,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Option<FolderPass>, SiftError> {
    let mut changed = vec![];
    let mut new_inbox = vec![];
    let mut missing = vec![];
    let db_err = |e: anyhow::Error| SiftError::app("db", e.to_string(), false);
    let caps = pool.caps().await;

    // SELECT inside the lease; the lease is released only after the values it
    // reported have been used (P4.3).
    let info = {
        let w = pool.with_selected_worker(folder, true, cancel).await?;
        w.info().clone()
    };
    if let Some(p) = prev {
        if p.uidvalidity != info.uidvalidity as i64 {
            return Ok(None);
        }
    }
    let mut cur = FolderCursor {
        role: role.into(),
        name: folder.into(),
        uidvalidity: info.uidvalidity as i64,
        uidnext: info.uidnext as i64,
        highestmodseq: info.highestmodseq.map(|v| v as i64),
        exists_count: info.exists as i64,
        last_full_scan: prev.and_then(|p| p.last_full_scan),
    };
    let prev_uidnext = prev.map(|p| p.uidnext).unwrap_or(1);
    let prev_modseq = prev.and_then(|p| p.highestmodseq);

    // Stored map: uid -> message hex.
    let stored: HashMap<i64, String> = sink
        .imap_uid_map(account_id, role)
        .await
        .map_err(db_err)?
        .into_iter()
        .collect();

    // --- new messages ------------------------------------------------------
    // `committed_uidnext` only advances after a batch is durably committed, so
    // a failed batch keeps the checkpoint before it and is retried (P4.5).
    let mut committed_uidnext = prev_uidnext;
    let mut batch_error: Option<String> = None;
    if info.uidnext as i64 > committed_uidnext {
        let from = committed_uidnext.max(1) as u32;
        // The ACTUAL UIDs at or above the checkpoint: one SEARCH, bounded by
        // the number of new messages, never by the gap in UID space.
        let new_uids: Vec<u32> = {
            let mut w = pool.with_selected_worker(folder, true, cancel).await?;
            w.conn().uid_search_range(from, None).await?
        }
        .into_iter()
        // A UID already in the map was committed (message row and UID row
        // share one transaction); only unknown UIDs are new work.
        .filter(|u| !stored.contains_key(&(*u as i64)))
        .collect();
        for chunk in new_uids.chunks(META_BATCH) {
            if cancel.is_cancelled() {
                return Err(SiftError::app("cancelled", "partial sync cancelled", false));
            }
            let items: Vec<MetaItem> = {
                let mut w = pool.with_selected_worker(folder, true, cancel).await?;
                message::fetch_meta(w.conn(), chunk).await?
            };
            let mut messages: Vec<crate::db::messages::MsgUpsert> = vec![];
            let mut attachments: Vec<crate::db::attachments::AttPut> = vec![];
            let mut uid_rows: Vec<(i64, String)> = vec![];
            let mut snip_targets = vec![];
            let mut thread_ids: Vec<String> = vec![];
            let mut batch_new_inbox = vec![];
            for item in &items {
                let extra: Vec<String> = extra_labels.iter().map(|s| s.to_string()).collect();
                if let Some(up) =
                    message::meta_to_upsert(account_id, item, String::new(), &extra, false)
                {
                    let is_new_inbox = role == "all"
                        && up.label_ids.contains(&"INBOX".to_string())
                        && up.is_unread;
                    let from = up
                        .from_name
                        .clone()
                        .unwrap_or_else(|| up.from_email.clone().unwrap_or_default());
                    let (tid, mid) = (up.thread_id.clone(), up.id.clone());
                    if let Some(bs) = &item.structure {
                        for att in message::attachment_rows(account_id, &mid, bs) {
                            attachments.push(att);
                        }
                        if snip_targets.len() < META_BATCH {
                            if let Some((section, enc, charset)) = message::snippet_section(bs) {
                                snip_targets.push(message::SnipTarget {
                                    uid: item.uid,
                                    message_hex: mid.clone(),
                                    section: section.clone(),
                                    enc,
                                    charset,
                                    is_html: message::section_is_html(bs, &section),
                                });
                            }
                        }
                    }
                    messages.push(up);
                    thread_ids.push(tid.clone());
                    if is_new_inbox {
                        batch_new_inbox.push(crate::provider::NewMail {
                            account_id: account_id.to_string(),
                            thread_id: tid,
                            message_id: mid.clone(),
                            from,
                            subject: item.headers.get("subject").cloned().unwrap_or_default(),
                        });
                    }
                    uid_rows.push((item.uid as i64, mid));
                }
            }
            let last_uid = chunk.last().copied().unwrap_or(from);
            let mut checkpoint = cur.clone();
            checkpoint.uidnext = last_uid as i64 + 1;
            let batch = MetadataBatch {
                account_id: account_id.to_string(),
                cursor: checkpoint,
                messages,
                attachments,
                uid_pairs: uid_rows,
            };
            match sink.commit_metadata_batch(&batch).await {
                Ok(()) => {
                    committed_uidnext = last_uid as i64 + 1;
                    for tid in &thread_ids {
                        changed.push((account_id.to_string(), tid.clone()));
                    }
                    new_inbox.append(&mut batch_new_inbox);
                    // Snippets after the commit: the network read may never
                    // sit inside the batch transaction.
                    message::fetch_snippet_groups(
                        pool,
                        folder,
                        account_id,
                        sink,
                        &snip_targets,
                        cancel,
                    )
                    .await;
                    sink.threads_changed(account_id, &thread_ids);
                }
                Err(e) => {
                    // Nothing from this batch was written: keep the checkpoint
                    // before it, record a recoverable error and stop scanning
                    // further (later batches must not be committed ahead of it).
                    batch_error = Some(e.to_string());
                    break;
                }
            }
        }
    }
    if let Some(err) = &batch_error {
        log::warn!(target: "sift::imap", "partial batch failed for {role}: {err}");
        let detail = format!("{role} batch at uid {committed_uidnext}: {err}");
        let _ = sink.log_sync(account_id, "partial-error", &detail).await;
        sink.progress(SyncStatus {
            account_id: account_id.to_string(),
            phase: "error".into(),
            done: 0,
            total: 0,
            total_known: true,
            last_error: Some(err.clone()),
        });
    }

    // --- flag/label deltas ---------------------------------------------------
    let mut max_modseq = cur.highestmodseq;
    if caps.condstore {
        if let Some(since) = prev_modseq {
            if since >= 0 {
                let deltas = {
                    let mut w = pool.with_selected_worker(folder, true, cancel).await?;
                    w.conn().uid_fetch_changed(since as u64).await?
                };
                for (_seq, attrs) in &deltas {
                    let meta = super::proto::gmail_meta(attrs);
                    let flags: Vec<String> = attrs
                        .iter()
                        .filter_map(|a| match a {
                            FetchAttr::Flags(f) => Some(f.clone()),
                            _ => None,
                        })
                        .flatten()
                        .collect();
                    let uid = attrs.iter().find_map(|a| match a {
                        FetchAttr::Uid(u) => Some(*u as i64),
                        _ => None,
                    });
                    if let Some(mid) = uid.and_then(|u| stored.get(&u).cloned()) {
                        if let Some(tid) = apply_remote_state(sink, account_id, &mid, &meta, &flags).await? {
                            changed.push((account_id.to_string(), tid));
                        }
                    }
                    if let Some(m) = meta.modseq {
                        max_modseq = Some(max_modseq.map_or(m as i64, |c: i64| c.max(m as i64)));
                    }
                }
            }
        }
    } else {
        // Fallback (never expected on Gmail): newest-2000 full flag compare.
        let all: Vec<u32> = {
            let mut w = pool.with_selected_worker(folder, true, cancel).await?;
            w.conn().uid_search_all().await?
        };
        let mut newest: Vec<u32> = all;
        newest.sort_unstable_by(|a, b| b.cmp(a));
        newest.truncate(ROLLING_SCAN_UIDS);
        newest.sort_unstable();
        for chunk in newest.chunks(META_BATCH) {
            let items = {
                let mut w = pool.with_selected_worker(folder, true, cancel).await?;
                message::fetch_meta(w.conn(), chunk).await?
            };
            for item in &items {
                if let Some(mid) = stored.get(&(item.uid as i64)).cloned() {
                    let (mapped, _, _, _, _) = message::map_labels(&item.meta.labels, &item.flags);
                    if let Some(tid) = apply_mapped_state(sink, account_id, &mid, &mapped).await? {
                        changed.push((account_id.to_string(), tid));
                    }
                }
            }
        }
    }

    // --- deletions / moves out ----------------------------------------------
    let stored_now: HashSet<i64> = sink
        .imap_uid_map(account_id, role)
        .await
        .map_err(db_err)?
        .into_iter()
        .map(|(u, _)| u)
        .collect();
    let need_diff = force_uid_diff
        || info.exists as i64 != stored_now.len() as i64
        || cur
            .last_full_scan
            .map(|t| crate::db::now_ms() - t > FULL_SCAN_INTERVAL_MS)
            .unwrap_or(true);
    if need_diff {
        let server: HashSet<i64> = {
            let mut w = pool.with_selected_worker(folder, true, cancel).await?;
            w.conn()
                .uid_search_all()
                .await?
                .into_iter()
                .map(|u| u as i64)
                .collect()
        };
        let uid2mid: HashMap<i64, String> = sink
            .imap_uid_map(account_id, role)
            .await
            .map_err(db_err)?
            .into_iter()
            .collect();
        let gone: Vec<i64> = stored_now.difference(&server).copied().collect();
        for uid in &gone {
            if let Some(mid) = uid2mid.get(uid) {
                missing.push(mid.clone());
            }
        }
        sink.imap_delete_uids(account_id, role, &gone)
            .await
            .map_err(db_err)?;
        cur.last_full_scan = Some(crate::db::now_ms());
    }

    // --- persist cursor --------------------------------------------------------
    // End-of-pass SELECT for exact trailing values; a failed batch keeps the
    // checkpoint before it so nothing is skipped (P4.5).
    let end = {
        let w = pool.with_selected_worker(folder, true, cancel).await?;
        w.info().clone()
    };
    cur.uidnext = match batch_error {
        Some(_) => committed_uidnext,
        None => end.uidnext as i64,
    };
    cur.highestmodseq = end
        .highestmodseq
        .map(|v| v as i64)
        .or(max_modseq)
        .or(cur.highestmodseq);
    cur.exists_count = end.exists as i64;
    sink.imap_set_folder(account_id, &cur)
        .await
        .map_err(db_err)?;
    Ok(Some((cur, changed, new_inbox, missing)))
}

/// Authoritative label set for a message, read from its current \All locator
/// (P4.5).
///
/// Returns `None` when the message is no longer in \All (deleted, or still
/// moving): the caller then changes only the stale membership instead of
/// guessing a destination.
async fn authoritative_labels(
    pool: &ImapPool,
    sink: &dyn SyncSink,
    account_id: &str,
    all_folder: &str,
    mid: &str,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Option<Vec<String>>, SiftError> {
    let db_err = |e: anyhow::Error| SiftError::app("db", e.to_string(), false);
    let uid: Option<i64> = sink
        .uids_for_message(account_id, mid)
        .await
        .map_err(db_err)?
        .into_iter()
        .find(|(r, _)| r == "all")
        .map(|(_, u)| u);
    let Some(uid) = uid else { return Ok(None) };
    let items = super::conn::FetchItems::new().uid().gmail_labels().flags();
    let rows = {
        let mut w = pool.with_selected_worker(all_folder, true, cancel).await?;
        w.conn().uid_fetch_items(&uid.to_string(), &items).await?
    };
    for (_seq, attrs) in rows {
        let row_uid = attrs.iter().find_map(|a| match a {
            FetchAttr::Uid(u) => Some(*u as i64),
            _ => None,
        });
        if row_uid != Some(uid) {
            continue;
        }
        let meta = super::proto::gmail_meta(&attrs);
        let flags: Vec<String> = attrs
            .iter()
            .filter_map(|a| match a {
                FetchAttr::Flags(f) => Some(f.clone()),
                _ => None,
            })
            .flatten()
            .collect();
        let (mapped, _, _, _, _) = message::map_labels(&meta.labels, &flags);
        return Ok(Some(mapped));
    }
    Ok(None)
}

/// Apply one remote flag/label state to a message row. Returns the thread id
/// when anything changed.
async fn apply_remote_state(
    sink: &dyn SyncSink,
    account_id: &str,
    mid: &str,
    meta: &GmailMeta,
    flags: &[String],
) -> Result<Option<String>, SiftError> {
    let (mapped, _, _, _, _) = message::map_labels(&meta.labels, flags);
    apply_mapped_state(sink, account_id, mid, &mapped).await
}

/// Diff helper: returns the thread id when the row changed.
///
/// The server snapshot is a *base*: local label intents the outbox still owes
/// the server are re-applied over it in queue order (P4.5), so an old snapshot
/// can never undo a change that is queued but not yet acknowledged.
async fn apply_mapped_state(
    sink: &dyn SyncSink,
    account_id: &str,
    mid: &str,
    mapped: &[String],
) -> Result<Option<String>, SiftError> {
    let db_err = |e: anyhow::Error| SiftError::app("db", e.to_string(), false);
    let cur: HashSet<String> = sink
        .message_labels(&crate::dto::MessageRef::new(account_id, mid))
        .await
        .map_err(db_err)?
        .into_iter()
        .collect();
    let mut want: HashSet<String> = mapped.iter().cloned().collect();
    for (add, remove) in sink.label_intents(account_id, mid).await.map_err(db_err)? {
        for a in add {
            want.insert(a);
        }
        for r in remove {
            want.remove(&r);
        }
    }
    if cur == want {
        return Ok(None);
    }
    let add: Vec<String> = want.difference(&cur).cloned().collect();
    let remove: Vec<String> = cur.difference(&want).cloned().collect();
    let (_, tid) = sink
        .apply_label_change(&crate::dto::MessageRef::new(account_id, mid), &add, &remove)
        .await
        .map_err(db_err)?;
    Ok(Some(tid))
}
