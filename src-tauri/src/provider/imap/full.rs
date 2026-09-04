//! IMAP full sync (Phase 11 task 6): newest-first metadata from `\All`
//! (+ `\Trash`/`\Junk` for those views), snippet pass for the newest 5,000,
//! progressive `store:threads` per 500-UID chunk. Idempotent: stored UIDs are
//! diffed, so a restart resumes without re-fetching.
use super::{
    conn::ImapPool,
    folders::FolderMap,
    message::{self, fetch_snippet_groups, section_is_html, MetaItem, SnipTarget},
};
use crate::db::imap::FolderCursor;
use crate::dto::Label;
use crate::errors::SiftError;
use crate::provider::{Cursor, SyncSink};
use std::collections::HashSet;

const CHUNK: usize = 500;
const SNIPPET_BUDGET: usize = 5_000;

pub async fn run_full_sync(
    pool: &ImapPool,
    sink: &dyn SyncSink,
    account_id: &str,
    folders: &FolderMap,
    cancel: tokio_util::sync::CancellationToken,
) -> Result<Cursor, SiftError> {
    // System labels keep their REST ids: every sidebar query works unchanged.
    let mut labels: Vec<Label> = super::provider::sys_labels(account_id);
    for (i, (name, id)) in folders.user.iter().enumerate() {
        labels.push(Label {
            account_id: account_id.into(),
            id: id.clone(),
            name: name.clone(),
            kind: "user".into(),
            color_bg: None,
            color_fg: None,
            visible: true,
            unread_count: 0,
            total_count: 0,
            sort_order: 200 + i as i64,
        });
    }
    sink.upsert_labels(&labels)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    sink.progress(crate::dto::SyncStatus {
        account_id: account_id.into(),
        phase: "listing".into(),
        done: 0,
        total: 0,
        last_error: None,
    });

    let mut cursors = vec![];
    // \All first (snippets), then Trash/Junk without snippets.
    for (role, extra) in [
        ("all", vec![]),
        ("trash", vec!["TRASH"]),
        ("junk", vec!["SPAM"]),
    ] {
        if cancel.is_cancelled() {
            return Err(SiftError::app("cancelled", "full sync cancelled", false));
        }
        let name = folders.name_for_role(role).ok_or_else(|| {
            SiftError::app("imap_protocol", format!("no folder for {role}"), true)
        })?;
        let cur = sync_folder(pool, sink, account_id, role, name, &extra, true, &cancel).await?;
        cursors.push(cur);
    }
    sink.set_sync_state(account_id, "partial")
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    sink.log_sync(account_id, "full", "done")
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    let cursor = Cursor::Imap { folders: cursors };
    sink.set_history_id(account_id, &cursor.render())
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    Ok(cursor)
}

#[allow(clippy::too_many_arguments)]
async fn sync_folder(
    pool: &ImapPool,
    sink: &dyn SyncSink,
    account_id: &str,
    role: &str,
    folder: &str,
    extra_labels: &[&str],
    with_snippets: bool,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<FolderCursor, SiftError> {
    // EXAMINE is read-only; label/flag writes use their own SELECT in ops.
    let info = {
        let mut guard = pool.worker().await?;
        let conn = guard.as_mut().expect("connected");
        conn.select(folder, true).await?
    };
    let mut cur = FolderCursor {
        role: role.into(),
        name: folder.into(),
        uidvalidity: info.uidvalidity as i64,
        uidnext: info.uidnext as i64,
        highestmodseq: info.highestmodseq.map(|v| v as i64),
        exists_count: info.exists as i64,
        last_full_scan: Some(crate::db::now_ms()),
    };
    sink.imap_set_folder(account_id, &cur)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;

    // Full UID list, newest first.
    let all_uids: Vec<u32> = {
        let mut guard = pool.worker().await?;
        let conn = guard.as_mut().expect("connected");
        conn.uid_search_all().await?
    };
    sink.progress(crate::dto::SyncStatus {
        account_id: account_id.into(),
        phase: "listing".into(),
        done: 0,
        total: all_uids.len() as i64,
        last_error: None,
    });
    // Resume: skip UIDs already stored.
    let stored: HashSet<i64> = sink
        .imap_uid_map(account_id, role)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?
        .into_iter()
        .map(|(u, _)| u)
        .collect();
    let mut missing: Vec<u32> = all_uids
        .into_iter()
        .filter(|u| !stored.contains(&(*u as i64)))
        .collect();
    missing.sort_unstable_by(|a, b| b.cmp(a));
    let total = missing.len() as i64;
    let mut done = 0i64;
    let mut snippeted = 0usize;

    for chunk in missing.chunks(CHUNK) {
        if cancel.is_cancelled() {
            return Err(SiftError::app("cancelled", "full sync cancelled", false));
        }
        let items: Vec<MetaItem> = {
            let mut guard = pool.worker().await?;
            let conn = guard.as_mut().expect("connected");
            message::fetch_meta(conn, chunk).await?
        };
        let mut thread_ids = vec![];
        let mut uid_rows = vec![];
        let mut snip_targets: Vec<SnipTarget> = vec![];
        for item in &items {
            let extra: Vec<String> = extra_labels.iter().map(|s| s.to_string()).collect();
            if let Some(mut up) =
                message::meta_to_upsert(account_id, item, String::new(), &extra, false)
            {
                // Attachment meta rows (content fetched lazily on open).
                // Stored AFTER the message row: attachments.message_id is a
                // foreign key and the write would fail otherwise.
                let mut att_rows = vec![];
                if let Some(bs) = &item.structure {
                    att_rows = message::attachment_rows(&up.id, bs);
                    if !att_rows.is_empty() {
                        up.has_attachments = true;
                    }
                }
                if with_snippets && snippeted < SNIPPET_BUDGET {
                    if let Some(bs) = &item.structure {
                        if let Some((section, enc, charset)) = message::snippet_section(bs) {
                            snip_targets.push(SnipTarget {
                                uid: item.uid,
                                message_hex: up.id.clone(),
                                section: section.clone(),
                                enc,
                                charset,
                                is_html: section_is_html(bs, &section),
                            });
                            snippeted += 1;
                        }
                    }
                }
                let (tid, mid) = (up.thread_id.clone(), up.id.clone());
                if let Err(e) = sink.upsert_message(up).await {
                    log::warn!(target: "sift::imap", "upsert failed: {e}");
                    continue;
                }
                for att in att_rows {
                    if let Err(e) = sink.store_attachment(att).await {
                        log::warn!(target: "sift::imap", "attachment row failed: {e}");
                    }
                }
                thread_ids.push(tid);
                uid_rows.push((item.uid as i64, mid));
            }
        }
        sink.imap_put_uids(account_id, role, &uid_rows)
            .await
            .map_err(|e| SiftError::app("db", e.to_string(), false))?;
        fetch_snippet_groups(pool, sink, &snip_targets, cancel).await;
        done += chunk.len() as i64;
        sink.progress(crate::dto::SyncStatus {
            account_id: account_id.into(),
            phase: "metadata".into(),
            done,
            total,
            last_error: None,
        });
        sink.threads_changed(account_id, &thread_ids);
    }
    // Refresh cursor post-scan (UIDNEXT may have moved).
    let info = {
        let mut guard = pool.worker().await?;
        let conn = guard.as_mut().expect("connected");
        conn.select(folder, true).await?
    };
    cur.uidnext = info.uidnext as i64;
    cur.highestmodseq = info.highestmodseq.map(|v| v as i64);
    cur.exists_count = info.exists as i64;
    cur.last_full_scan = Some(crate::db::now_ms());
    sink.imap_set_folder(account_id, &cur)
        .await
        .map_err(|e| SiftError::app("db", e.to_string(), false))?;
    Ok(cur)
}
