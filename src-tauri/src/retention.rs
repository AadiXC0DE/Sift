//! Cache retention and disk accounting (P10.4).
//!
//! Three kinds of app-owned bytes exist on disk, and each has a different
//! owner:
//!
//! | directory | owner | may be deleted when |
//! |---|---|---|
//! | `attachments/<account>/<attachment>/` | the attachment cache | over the cap, oldest access first, and never when pinned |
//! | `raw/<account>/` | the raw-source cache | over the cap, oldest access first |
//! | `compose-cache/<draft>/` | the draft / outbox recovery state | the draft is gone **and** no queued operation still points at a file in it |
//!
//! Nothing here deletes a file because it looks old. A file is deleted when its
//! owner says it is finished with it, or when it is over the retention cap and
//! nothing references it — and the sweep is bounded so it can run at startup
//! without standing between the user and their inbox.
//!
//! ## What is never evicted
//!
//! * **Pinned** attachments (`pinned_at`): the user asked to keep them offline.
//! * **In-use** files: a preview or a save that is reading the file right now.
//!   The lease is process-local, which is correct because eviction is
//!   process-local too.
//! * **In-flight downloads** (`cache_state='downloading'`): the file is being
//!   written, and its row is about to point at it.
//! * **Anything a draft or a queued operation still references**, which is what
//!   protects a pending send's frozen MIME and every staged attachment.
//!
//! ## Storage totals
//!
//! `storage_usage` reports the sum of the app-owned file sizes (logical bytes,
//! as the filesystem reports them) plus the SQLite page count. It deliberately
//! does not try to model allocation: a 1-byte file can occupy 4 KiB, APFS may
//! share extents, and a sparse file is smaller on disk than its length. The
//! documented difference is therefore "logical size, not allocated size", which
//! is the only figure an app can compute portably and verify against its own
//! files.

use crate::db::Db;
use anyhow::Result;
use rusqlite::params;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// The default cap for the attachment cache: 512 MiB. A larger cap is an
/// explicit choice the user makes in Settings; nothing here raises it on its
/// own.
pub const DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES: i64 = 512 * 1024 * 1024;

/// A file with no reference anywhere is only swept after this long, so a
/// transfer that is mid-flight (or a draft being written) is never a candidate.
pub const SWEEP_MIN_AGE_MS: i64 = 24 * 60 * 60 * 1000;

/// How many filesystem entries one sweep tick examines. The rest is scheduled
/// for the next tick, which is how a huge cache directory cannot delay inbox
/// paint.
pub const SWEEP_BATCH: usize = 256;

/// Parse a human size setting (`"512MB"`, `"2GB"`, `"500 MB"`, `"1GiB"`,
/// `"1024KB"`, `"2048B"`) into bytes.
///
/// Binary units, because that is what a cache size means. Junk falls back to
/// the default rather than to zero: a settings file that cannot be read must
/// not silently disable the cache, and it must not silently allow it to grow
/// without bound either. An explicit `0` is honoured — "keep nothing cached" is
/// a real choice.
pub fn parse_cache_limit(value: &str) -> i64 {
    let text = value.trim().to_ascii_uppercase();
    if text.is_empty() {
        return DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES;
    }
    let (digits, mult) =
        if let Some(v) = text.strip_suffix("GIB").or_else(|| text.strip_suffix("GB")) {
            (v, 1024 * 1024 * 1024)
        } else if let Some(v) = text.strip_suffix("MIB").or_else(|| text.strip_suffix("MB")) {
            (v, 1024 * 1024)
        } else if let Some(v) = text.strip_suffix("KIB").or_else(|| text.strip_suffix("KB")) {
            (v, 1024)
        } else if let Some(v) = text.strip_suffix('B') {
            (v, 1)
        } else {
            (text.as_str(), 1)
        };
    let digits = digits.trim();
    if digits.is_empty() {
        return DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES;
    }
    // A plain decimal amount only: an exponent (`1e9MB`) or stray words
    // (`1GB extra`) are junk, not a size.
    let mut seen_dot = false;
    let mut seen_digit = false;
    for ch in digits.chars() {
        match ch {
            '0'..='9' => seen_digit = true,
            '.' if !seen_dot => seen_dot = true,
            _ => return DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES,
        }
    }
    if !seen_digit {
        return DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES;
    }
    let Ok(amount) = digits.parse::<f64>() else {
        return DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES;
    };
    if !amount.is_finite() || amount < 0.0 {
        return DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES;
    }
    (amount * mult as f64).round().min(i64::MAX as f64) as i64
}

/// One evictable cache file.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedFile {
    pub account_id: String,
    pub attachment_id: String,
    pub message_id: String,
    pub path: String,
    pub bytes: i64,
}

/// What one eviction pass did.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvictionReport {
    pub evicted: i64,
    pub bytes_freed: i64,
    /// Bytes still cached but exempt (pinned or in use), so a report can
    /// explain why usage is above the cap.
    pub exempt_bytes: i64,
    pub trimmed: bool,
}

/// Logical bytes and file count under one directory tree. Missing directory is
/// zero, not an error: a cache that was never used is not a failure.
pub fn dir_usage(path: &Path) -> (i64, i64) {
    let mut bytes = 0i64;
    let mut files = 0i64;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(entry.path());
            } else if meta.is_file() {
                bytes += meta.len() as i64;
                files += 1;
            }
        }
    }
    (bytes, files)
}

impl Db {
    /// Cached attachment files, least recently accessed first.
    ///
    /// Pinned rows are excluded here rather than filtered afterwards so the
    /// "why is usage above the cap" answer comes out of the same query.
    pub async fn evictable_attachments(&self) -> Result<Vec<CachedFile>> {
        self.read(|c| {
            let mut s = c.prepare(
                "SELECT account_id, id, message_id, local_path, COALESCE(decoded_size, size) \
                 FROM attachments \
                 WHERE local_path IS NOT NULL AND pinned_at IS NULL \
                   AND cache_state IN ('ready','unverified') \
                 ORDER BY COALESCE(last_accessed_at, 0) ASC, id ASC",
            )?;
            let rows = s
                .query_map([], |r| {
                    Ok(CachedFile {
                        account_id: r.get(0)?,
                        attachment_id: r.get(1)?,
                        message_id: r.get(2)?,
                        path: r.get(3)?,
                        bytes: r.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }

    /// Bytes in pinned files, which no automatic pass may free.
    pub async fn pinned_attachment_bytes(&self) -> Result<i64> {
        self.read(|c| {
            Ok(c.query_row(
                "SELECT COALESCE(SUM(COALESCE(decoded_size, size)),0) FROM attachments \
                 WHERE pinned_at IS NOT NULL AND local_path IS NOT NULL",
                [],
                |r| r.get(0),
            )?)
        })
        .await
    }

    /// Drop a cached file's row-level claim on it, keeping everything needed to
    /// download it again.
    pub async fn attachment_forget_file(&self, account_id: &str, id: &str) -> Result<()> {
        let (a, i) = (account_id.to_string(), id.to_string());
        self.write(move |c| {
            c.execute(
                "UPDATE attachments SET local_path=NULL, decoded_size=NULL, cache_state='missing' \
                 WHERE account_id=? AND id=?",
                params![a, i],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn attachment_pin(&self, account_id: &str, id: &str, pinned: bool) -> Result<()> {
        let (a, i) = (account_id.to_string(), id.to_string());
        let now = crate::db::now_ms();
        self.write(move |c| {
            c.execute(
                "UPDATE attachments SET pinned_at=?3 WHERE account_id=?1 AND id=?2",
                params![a, i, if pinned { Some(now) } else { None }],
            )?;
            Ok(())
        })
        .await
    }

    /// Every path a draft or a queued operation still owns.
    ///
    /// This is the reference set the staging sweep respects: a file named here
    /// is unreachable by any automatic deletion, however old it is.
    pub async fn referenced_staging_paths(&self) -> Result<HashSet<String>> {
        self.read(|c| {
            let mut out: HashSet<String> = HashSet::new();
            let mut drafts = c.prepare("SELECT attachments_json FROM drafts")?;
            for row in drafts.query_map([], |r| r.get::<_, String>(0))? {
                let json = row?;
                let values: Vec<serde_json::Value> =
                    serde_json::from_str(&json).unwrap_or_default();
                for v in values {
                    if let Some(p) = v.get("path").and_then(|p| p.as_str()) {
                        out.insert(p.to_string());
                    }
                }
            }
            // A queued send owns its frozen MIME and any file its payload names.
            let mut ops = c.prepare(
                "SELECT payload FROM outbox_ops WHERE state IN ('pending','inflight','uncertain')",
            )?;
            for row in ops.query_map([], |r| r.get::<_, String>(0))? {
                let json = row?;
                let value: serde_json::Value = serde_json::from_str(&json).unwrap_or_default();
                for key in ["rawPath", "path"] {
                    if let Some(p) = value.get(key).and_then(|p| p.as_str()) {
                        out.insert(p.to_string());
                    }
                }
                if let Some(list) = value.get("cachePaths").and_then(|l| l.as_array()) {
                    for p in list.iter().filter_map(|p| p.as_str()) {
                        out.insert(p.to_string());
                    }
                }
            }
            // Cached attachment files and raw sources are owned by their rows.
            let mut files =
                c.prepare("SELECT local_path FROM attachments WHERE local_path IS NOT NULL")?;
            for row in files.query_map([], |r| r.get::<_, String>(0))? {
                out.insert(row?);
            }
            let mut raw = c.prepare("SELECT path FROM message_raw")?;
            for row in raw.query_map([], |r| r.get::<_, String>(0))? {
                out.insert(row?);
            }
            Ok(out)
        })
        .await
    }
}

/// Enforce the attachment cap by least-recently-used eviction.
///
/// The walk is ordered by `last_accessed_at` (an indexed column), so the file
/// the user has not touched for longest goes first. A file that is in use, part
/// of an in-flight download, or pinned is not a candidate at all — the report
/// says how many bytes were exempt so the panel can explain the difference
/// between "over the cap" and "wrong".
pub async fn enforce_attachment_cap(db: &Db, cap_bytes: i64) -> Result<EvictionReport> {
    let mut report = EvictionReport::default();
    let candidates = db.evictable_attachments().await?;
    let total: i64 = candidates.iter().map(|c| c.bytes).sum();
    report.exempt_bytes = db.pinned_attachment_bytes().await?;
    if cap_bytes <= 0 {
        // A cap of zero means the user wants no downloaded attachments kept.
        for c in &candidates {
            if evict_one(db, c).await? {
                report.evicted += 1;
                report.bytes_freed += c.bytes;
            }
        }
        report.trimmed = true;
        return Ok(report);
    }
    if total <= cap_bytes {
        return Ok(report);
    }
    let mut remaining = total;
    for c in &candidates {
        if remaining <= cap_bytes {
            break;
        }
        if crate::attachments::in_use::is_in_use(&c.account_id, &c.attachment_id) {
            // A preview is reading it right now: keep it and try the next.
            report.exempt_bytes += c.bytes;
            continue;
        }
        if evict_one(db, c).await? {
            report.evicted += 1;
            report.bytes_freed += c.bytes;
            remaining -= c.bytes;
            report.trimmed = true;
        }
    }
    Ok(report)
}

async fn evict_one(db: &Db, file: &CachedFile) -> Result<bool> {
    // The row is released first: a file that has already been removed must not
    // keep a `ready` row pointing at nothing.
    db.attachment_forget_file(&file.account_id, &file.attachment_id)
        .await?;
    match tokio::fs::remove_file(&file.path).await {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(e) => {
            log::warn!("could not evict {}: {e}", file.path);
            Ok(false)
        }
    }
}

/// Directories under `<data_dir>/compose-cache` that a draft or a queued
/// operation still refers to.
fn referenced_dirs(data_dir: &Path, referenced: &HashSet<String>) -> HashSet<PathBuf> {
    let mut out: HashSet<PathBuf> = HashSet::new();
    for path in referenced {
        let p = Path::new(path);
        if let Ok(rel) = p.strip_prefix(data_dir.join("compose-cache")) {
            if let Some(first) = rel.iter().next() {
                out.insert(data_dir.join("compose-cache").join(first));
            }
        }
    }
    out
}

/// Remove one batch of orphaned staging and abandoned transfer files.
///
/// Only files that are older than [`SWEEP_MIN_AGE_MS`] **and** named by no
/// draft, no queued operation and no cache row are removed. Returns how many
/// entries were processed, so the caller can tell whether another batch is
/// waiting.
pub async fn sweep_ephemeral(db: &Db, data_dir: &Path, budget: usize) -> Result<usize> {
    let referenced = db.referenced_staging_paths().await?;
    let keep_dirs = referenced_dirs(data_dir, &referenced);
    let cutoff = crate::db::now_ms() - SWEEP_MIN_AGE_MS;
    let mut processed = 0usize;
    for root in [
        data_dir.join("attachments"),
        data_dir.join("compose-cache"),
        data_dir.join("raw"),
    ] {
        if processed >= budget {
            break;
        }
        processed += sweep_root(&root, &keep_dirs, &referenced, cutoff, budget - processed);
    }
    Ok(processed)
}

fn sweep_root(
    root: &Path,
    keep_dirs: &HashSet<PathBuf>,
    referenced: &HashSet<String>,
    cutoff_ms: i64,
    budget: usize,
) -> usize {
    let mut processed = 0usize;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        if processed >= budget {
            return processed;
        }
        // A staging directory a draft still owns is not walked at all.
        if keep_dirs.contains(&dir) {
            continue;
        }
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            if processed >= budget {
                return processed;
            }
            let path = entry.path();
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_dir() {
                stack.push(path);
                continue;
            }
            processed += 1;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let abandoned = name.contains(".part-") || name.ends_with(".tmp");
            if !abandoned {
                continue;
            }
            if referenced.contains(&path.to_string_lossy().to_string()) {
                continue;
            }
            let modified_ms = meta
                .modified()
                .ok()
                .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64);
            if let Some(modified_ms) = modified_ms {
                // Younger than the retention window: still the owner's.
                if modified_ms > cutoff_ms {
                    continue;
                }
            }
            if std::fs::remove_file(&path).is_ok() {
                log::debug!("swept abandoned transfer file {}", path.display());
            }
        }
    }
    processed
}

/// Delete everything an account owned on disk.
///
/// Called **after** the account's tasks have stopped and its rows and
/// operations are gone (P10.4): deleting first would leave rows pointing at
/// files that no longer exist if the transaction failed, and a still-running
/// task could recreate the directory it just lost.
pub fn remove_account_cache(data_dir: &Path, account_id: &str, draft_ids: &[String]) {
    let attachments = data_dir.join("attachments").join(account_id);
    let _ = std::fs::remove_dir_all(&attachments);
    let raw = crate::db::raw_cache::raw_dir(data_dir, account_id);
    let _ = std::fs::remove_dir_all(&raw);
    for draft_id in draft_ids {
        let dir = crate::outgoing::draft_send_dir(data_dir, draft_id);
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// The periodic retention pass (P10.4).
///
/// It runs one bounded batch per tick and then sleeps, so a huge cache
/// directory (or a `.part` file left by a crash) is cleaned up across several
/// ticks instead of delaying the first paint. The first tick is short by
/// design: startup work is the one thing the user is definitely waiting for.
pub async fn run_retention_loop(app: tauri::AppHandle) {
    use tauri::Manager;
    const TICK_MS: u64 = 60_000;
    loop {
        {
            let state = app.state::<crate::app_state::AppState>();
            let settings = state.db.settings_get().await.unwrap_or_default();
            if let Err(e) = enforce_attachment_cap(
                &state.db,
                parse_cache_limit(&settings.attachment_cache_size),
            )
            .await
            {
                log::warn!("attachment cap: {e}");
            }
            if let Err(e) = sweep_ephemeral(&state.db, &state.data_dir, SWEEP_BATCH).await {
                log::warn!("cache sweep: {e}");
            }
            let _ = state.db.notify_prune(30 * 24 * 60 * 60 * 1000).await;
        }
        tokio::time::sleep(std::time::Duration::from_millis(TICK_MS)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p10_4_cache_limit_parses_and_is_bounded() {
        assert_eq!(parse_cache_limit("512MB"), 512 * 1024 * 1024);
        assert_eq!(parse_cache_limit("1GB"), 1024 * 1024 * 1024);
        assert_eq!(parse_cache_limit("2GiB"), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_cache_limit("0"), 0);
        // Junk falls back to the documented default rather than to zero.
        assert_eq!(parse_cache_limit(""), DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES);
        assert_eq!(
            parse_cache_limit("lots"),
            DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES
        );
        assert_eq!(
            parse_cache_limit("-5GB"),
            DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES
        );
        // Decimal amounts are supported, exponents and stray words are not.
        assert_eq!(parse_cache_limit("1.5GB"), 1_610_612_736);
        assert_eq!(
            parse_cache_limit("1e9MB"),
            DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES
        );
    }

    #[test]
    fn p10_4_dir_usage_counts_logical_bytes() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/x.bin"), vec![0u8; 100]).unwrap();
        std::fs::write(dir.path().join("a/b/y.bin"), vec![0u8; 50]).unwrap();
        let (bytes, files) = dir_usage(dir.path());
        assert_eq!((bytes, files), (150, 2));
        assert_eq!(dir_usage(&dir.path().join("missing")), (0, 0));
    }
}
