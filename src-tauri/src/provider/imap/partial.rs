//! IMAP incremental sync (Phase 11 task 7): per folder — new messages by
//! UIDNEXT, flag/label deltas by CHANGEDSINCE (newest-2000 diff without
//! CONDSTORE, which Gmail always advertises), deletions by UID-set diff,
//! UIDVALIDITY mismatch → NeedsFull. A quiet poll is one SELECT plus one
//! CHANGEDSINCE fetch.
use super::{
    conn::ImapPool,
    message::{self, MetaItem},
};
use crate::db::imap::FolderCursor;
use crate::errors::SiftError;
use crate::provider::{Cursor, PartialOutcome, SyncSink};
use std::collections::{HashMap, HashSet};

use super::proto::{FetchAttr, GmailMeta};

const ROLLING_SCAN_UIDS: usize = 2_000;
const FULL_SCAN_INTERVAL_MS: i64 = 6 * 3_600_000;

/// Run one partial tick over all/trash/junk. `prev` holds the stored folder
/// cursors; `force_uid_diff` forces the UID-set reconciliation (every 10th
/// poll per spec — the caller owns the counter).
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
    let mut new_inbox: Vec<(String, String, String, String)> = vec![];
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
            for mid in missing {
                let holders: HashSet<String> = sink
                    .uids_for_message(account_id, &mid)
                    .await
                    .map_err(|e| SiftError::app("db", e.to_string(), false))?
                    .into_iter()
                    .map(|(r, _)| r)
                    .collect();
                if holders.contains("all") {
                    let add = vec!["INBOX".to_string()];
                    let remove = if role == "trash" {
                        vec!["TRASH".to_string()]
                    } else {
                        vec!["SPAM".to_string()]
                    };
                    let (a, t) = sink
                        .apply_label_change(&mid, &add, &remove)
                        .await
                        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
                    changed.push((a, t));
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
                .apply_label_change(&mid, &["TRASH".to_string()], &["INBOX".to_string()])
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            changed.push((a, t));
        } else if holders.contains("junk") {
            let (a, t) = sink
                .apply_label_change(&mid, &["SPAM".to_string()], &["INBOX".to_string()])
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            changed.push((a, t));
        } else if let Some((a, t)) = sink
            .message_thread(&mid)
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?
        {
            // Deleted forever (or moved out by another client): drop the row.
            sink.delete_message(&mid, &a, &t)
                .await
                .map_err(|e| SiftError::app("db", e.to_string(), false))?;
            changed.push((a, t));
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
    Vec<(String, String, String, String)>,
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

    let info = {
        let mut guard = pool.worker().await?;
        guard
            .as_mut()
            .expect("connected")
            .select(folder, true)
            .await?
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
    if info.uidnext as i64 > prev_uidnext {
        let from = prev_uidnext.max(1) as u32;
        let new_uids: Vec<u32> = (from..info.uidnext).collect();
        for chunk in new_uids.chunks(500) {
            if cancel.is_cancelled() {
                return Err(SiftError::app("cancelled", "partial sync cancelled", false));
            }
            let items: Vec<MetaItem> = {
                let mut guard = pool.worker().await?;
                let conn = guard.as_mut().expect("connected");
                message::fetch_meta(conn, chunk).await?
            };
            let mut uid_rows = vec![];
            let mut snip_targets = vec![];
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
                    if sink.upsert_message(up).await.is_err() {
                        continue;
                    }
                    uid_rows.push((item.uid as i64, mid.clone()));
                    if let Some(bs) = &item.structure {
                        for att in message::attachment_rows(&mid, bs) {
                            let _ = sink.store_attachment(att).await;
                        }
                        if snip_targets.len() < 500 {
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
                    changed.push((account_id.to_string(), tid.clone()));
                    if is_new_inbox {
                        new_inbox.push((
                            account_id.to_string(),
                            tid,
                            from,
                            item.headers.get("subject").cloned().unwrap_or_default(),
                        ));
                    }
                }
            }
            sink.imap_put_uids(account_id, role, &uid_rows)
                .await
                .map_err(db_err)?;
            message::fetch_snippet_groups(pool, sink, &snip_targets, cancel).await;
            sink.threads_changed(
                account_id,
                &changed.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>(),
            );
        }
    }

    // --- flag/label deltas ---------------------------------------------------
    let mut max_modseq = cur.highestmodseq;
    if caps.condstore {
        if let Some(since) = prev_modseq {
            if since >= 0 {
                let deltas = {
                    let mut guard = pool.worker().await?;
                    let conn = guard.as_mut().expect("connected");
                    conn.uid_fetch_changed(since as u64).await?
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
                        if let Some(tid) = apply_remote_state(sink, &mid, &meta, &flags).await? {
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
            let mut guard = pool.worker().await?;
            let conn = guard.as_mut().expect("connected");
            conn.uid_search_all().await?
        };
        let mut newest: Vec<u32> = all;
        newest.sort_unstable_by(|a, b| b.cmp(a));
        newest.truncate(ROLLING_SCAN_UIDS);
        newest.sort_unstable();
        for chunk in newest.chunks(500) {
            let items = {
                let mut guard = pool.worker().await?;
                let conn = guard.as_mut().expect("connected");
                message::fetch_meta(conn, chunk).await?
            };
            for item in &items {
                if let Some(mid) = stored.get(&(item.uid as i64)).cloned() {
                    let (mapped, _, _, _, _) = message::map_labels(&item.meta.labels, &item.flags);
                    if let Some(tid) = apply_mapped_state(sink, &mid, &mapped).await? {
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
            let mut guard = pool.worker().await?;
            let conn = guard.as_mut().expect("connected");
            conn.uid_search_all()
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
    // End-of-pass SELECT for exact trailing values.
    let end = {
        let mut guard = pool.worker().await?;
        guard
            .as_mut()
            .expect("connected")
            .select(folder, true)
            .await?
    };
    cur.uidnext = end.uidnext as i64;
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

/// Apply one remote flag/label state to a message row. Returns the thread id
/// when anything changed.
async fn apply_remote_state(
    sink: &dyn SyncSink,
    mid: &str,
    meta: &GmailMeta,
    flags: &[String],
) -> Result<Option<String>, SiftError> {
    let (mapped, _, _, _, _) = message::map_labels(&meta.labels, flags);
    apply_mapped_state(sink, mid, &mapped).await
}

/// Diff helper: returns the thread id when the row changed.
async fn apply_mapped_state(
    sink: &dyn SyncSink,
    mid: &str,
    mapped: &[String],
) -> Result<Option<String>, SiftError> {
    let db_err = |e: anyhow::Error| SiftError::app("db", e.to_string(), false);
    let cur: HashSet<String> = sink
        .message_labels(mid)
        .await
        .map_err(db_err)?
        .into_iter()
        .collect();
    let want: HashSet<String> = mapped.iter().cloned().collect();
    if cur == want {
        return Ok(None);
    }
    let add: Vec<String> = want.difference(&cur).cloned().collect();
    let remove: Vec<String> = cur.difference(&want).cloned().collect();
    let (_, tid) = sink
        .apply_label_change(mid, &add, &remove)
        .await
        .map_err(db_err)?;
    Ok(Some(tid))
}
