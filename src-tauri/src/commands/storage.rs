//! Settings → Storage metering and the "clear downloaded attachment cache"
//! action (P10.4).
//!
//! Every number the panel shows is measured, never estimated: body and
//! attachment payload bytes come from `length()` over the stored compressed
//! BLOBs, on-disk cache sizes from walking the real cache directories, and the
//! metadata bucket is the SQLite file's page footprint minus the two
//! SQLite-resident buckets, floored at zero. `total_bytes` is the exact sum of
//! the four categories, so a clear action can be verified against it.
//!
//! Cache roots, read from the modules that own them:
//! * attachments — `<data_dir>/attachments/<account-id>/<attachment-id>/…`
//!   ([`crate::attachments::cache::attachment_dir`]).
//! * draft staging — `<data_dir>/compose-cache` (`attachments_add_from_paths`).

use std::path::Path;

use anyhow::{Context, Result};
use tauri::State;

use crate::app_state::AppState;
use crate::db::Db;
use crate::dto::{StorageCategory, StorageUsage};
use crate::errors::SiftError;

/// Cap used when `settings.attachment_cache_size` is unset or unparseable
/// (512 MiB, the documented default).
pub const DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES: i64 = 512 * 1024 * 1024;

const ATTACHMENT_CACHE_DIR: &str = "attachments";
const DRAFT_CACHE_DIR: &str = "compose-cache";

fn storage_error(e: anyhow::Error) -> SiftError {
    SiftError::app("storage", e.to_string(), false)
}

/// Parse a human size setting (`"512MB"`, `"2GB"`, `"500 MB"`, `"1GiB"`,
/// `"1024KB"`, `"2048B"`) into bytes. Case-insensitive; every unit is binary
/// (MB = MiB), matching how the cap is enforced elsewhere. An empty,
/// negative, non-numeric or unknown-unit value falls back to
/// [`DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES`]; an explicit `0` is honoured.
pub fn parse_cache_limit(raw: &str) -> i64 {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES;
    }
    let split = trimmed
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(trimmed.len());
    let (number, unit) = trimmed.split_at(split);
    let Ok(value) = number.parse::<f64>() else {
        return DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES;
    };
    if !value.is_finite() || value < 0.0 {
        return DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES;
    }
    let multiplier = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" | "byte" | "bytes" => 1.0,
        "k" | "kb" | "kib" => 1024.0,
        "m" | "mb" | "mib" => 1024.0 * 1024.0,
        "g" | "gb" | "gib" => 1024.0 * 1024.0 * 1024.0,
        _ => return DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES,
    };
    let bytes = value * multiplier;
    if !bytes.is_finite() || bytes > i64::MAX as f64 {
        return DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES;
    }
    bytes.round() as i64
}

/// Logical bytes and file count of every regular file under `root`,
/// recursively. A missing directory is an empty cache, not an error, and a
/// partially readable tree reports what it could read rather than failing the
/// whole panel. Allocation slack is deliberately not counted: the panel shows
/// app-owned content bytes.
pub fn dir_usage(root: &Path) -> (i64, i64) {
    fn walk(dir: &Path, bytes: &mut i64, files: &mut i64) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                walk(&entry.path(), bytes, files);
            } else if kind.is_file() {
                if let Ok(meta) = entry.metadata() {
                    *bytes = bytes.saturating_add(meta.len() as i64);
                    *files += 1;
                }
            }
        }
    }
    let (mut bytes, mut files) = (0i64, 0i64);
    if root.is_dir() {
        walk(root, &mut bytes, &mut files);
    }
    (bytes, files)
}

/// The SQLite-resident half of the panel, read in one snapshot.
struct DbUsage {
    page_bytes: i64,
    bodies: StorageCategory,
    attachment_rows: StorageCategory,
    /// Rows in the metadata tables the panel's hint names (accounts, threads,
    /// labels).
    metadata_items: i64,
}

async fn read_db_usage(db: &Db) -> Result<DbUsage> {
    db.read(|c| {
        let (body_bytes, body_items): (i64, i64) = c.query_row(
            "SELECT COALESCE(SUM(COALESCE(length(html_z),0) + COALESCE(length(text_z),0)),0), COUNT(*)
             FROM bodies",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let (attachment_bytes, attachment_items): (i64, i64) = c.query_row(
            "SELECT COALESCE(SUM(COALESCE(length(data_z),0)),0), COUNT(*) FROM attachments",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let page_count: i64 = c.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let page_size: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        let metadata_items: i64 = c.query_row(
            "SELECT (SELECT COUNT(*) FROM accounts)
                  + (SELECT COUNT(*) FROM threads)
                  + (SELECT COUNT(*) FROM labels)",
            [],
            |r| r.get(0),
        )?;
        Ok(DbUsage {
            page_bytes: page_count.saturating_mul(page_size),
            bodies: StorageCategory {
                bytes: body_bytes,
                items: body_items,
            },
            attachment_rows: StorageCategory {
                bytes: attachment_bytes,
                items: attachment_items,
            },
            metadata_items,
        })
    })
    .await
}

/// Combine the measured buckets. The metadata bucket is what is left of the
/// database file once the two SQLite-resident buckets are subtracted; a WAL
/// that has not been checkpointed can make that negative, so it floors at 0
/// instead of reporting a nonsense number.
fn assemble(
    db_usage: DbUsage,
    attachment_files: i64,
    draft_cache: StorageCategory,
    limit: i64,
    computed_at: i64,
) -> StorageUsage {
    let metadata = StorageCategory {
        bytes: db_usage
            .page_bytes
            .saturating_sub(db_usage.bodies.bytes)
            .saturating_sub(db_usage.attachment_rows.bytes)
            .max(0),
        items: db_usage.metadata_items,
    };
    let attachments = StorageCategory {
        bytes: db_usage
            .attachment_rows
            .bytes
            .saturating_add(attachment_files),
        items: db_usage.attachment_rows.items,
    };
    let total_bytes = metadata
        .bytes
        .saturating_add(db_usage.bodies.bytes)
        .saturating_add(attachments.bytes)
        .saturating_add(draft_cache.bytes);
    StorageUsage {
        metadata,
        bodies: db_usage.bodies,
        attachments,
        draft_cache,
        attachment_cache_limit_bytes: limit,
        total_bytes,
        computed_at,
    }
}

/// Measure all four app-owned cache classes plus the configured cap.
pub async fn measure(db: &Db, data_dir: &Path) -> Result<StorageUsage> {
    let db_usage = read_db_usage(db).await?;
    let limit = parse_cache_limit(&db.settings_get().await?.attachment_cache_size);
    let draft_usage = dir_usage(&data_dir.join(DRAFT_CACHE_DIR));
    Ok(assemble(
        db_usage,
        dir_usage(&data_dir.join(ATTACHMENT_CACHE_DIR)).0,
        StorageCategory {
            bytes: draft_usage.0,
            items: draft_usage.1,
        },
        limit,
        crate::db::now_ms(),
    ))
}

/// Remove the contents of `root`, leaving the directory itself in place so a
/// concurrent writer can still find it. A missing root is a no-op.
fn clear_dir(root: &Path) -> Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e).with_context(|| format!("read {}", root.display())),
    };
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let kind = entry.file_type()?;
        let removed = if kind.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        removed.with_context(|| format!("remove {}", path.display()))?;
    }
    Ok(())
}

/// Clear the downloaded attachment cache: drop the cached payload and path
/// from every attachment row, then delete the files on disk, then report the
/// freshly measured totals.
///
/// Rows are reset first so a failed file deletion cannot leave a row claiming
/// a `ready` file that no longer exists (and a failed database write touches
/// nothing at all). Attachment identity, filenames, MIME and the mail itself
/// are preserved, so every cleared attachment is redownloadable.
pub async fn clear_attachment_cache(db: &Db, data_dir: &Path) -> Result<StorageUsage> {
    db.write_tx(|tx| {
        tx.execute(
            "UPDATE attachments
                SET data_z=NULL, local_path=NULL, decoded_size=NULL, cache_state='missing'",
            [],
        )?;
        Ok(())
    })
    .await?;
    clear_dir(&data_dir.join(ATTACHMENT_CACHE_DIR))?;
    measure(db, data_dir).await
}

/// Measured usage for the Settings → Storage panel.
#[tauri::command]
pub async fn storage_usage(state: State<'_, AppState>) -> Result<StorageUsage, SiftError> {
    measure(&state.db, &state.data_dir)
        .await
        .map_err(storage_error)
}

/// Delete the downloaded attachment cache and return the post-clear usage.
#[tauri::command]
pub async fn storage_clear_attachment_cache(
    state: State<'_, AppState>,
) -> Result<StorageUsage, SiftError> {
    clear_attachment_cache(&state.db, &state.data_dir)
        .await
        .map_err(storage_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attachments::cache;
    use crate::db::attachments::AttPut;
    use crate::dto::{AttachmentRefKey, CacheState, MessageRef};
    use rusqlite::params;

    /// Seed one account plus one message, returning the account id.
    async fn seed_message(db: &Db, email: &str, mid: &str) -> String {
        let account = db.new_account(email, None, None).await.unwrap();
        let aid = account.id.clone();
        let mid = mid.to_string();
        db.write(move |c| {
            c.execute(
                "INSERT INTO messages (id,account_id,thread_id,internal_date) VALUES (?1,?2,'t1',1)",
                params![mid, aid],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        account.id
    }

    /// One attachment row carrying `data` as its cached payload.
    async fn put_cached(db: &Db, aid: &str, mid: &str, data: Vec<u8>) -> AttachmentRefKey {
        let size = data.len() as i64;
        db.attachments_put(AttPut {
            id: "att-2".into(),
            account_id: aid.into(),
            message_id: mid.into(),
            gmail_att_id: None,
            part_id: "2".into(),
            filename: Some("invoice.pdf".into()),
            mime: "application/pdf".into(),
            size,
            content_id: None,
            is_inline: false,
            data: Some(data),
        })
        .await
        .unwrap();
        AttachmentRefKey {
            account_id: aid.into(),
            attachment_id: "att-2".into(),
        }
    }

    fn total_of(u: &StorageUsage) -> i64 {
        u.metadata.bytes + u.bodies.bytes + u.attachments.bytes + u.draft_cache.bytes
    }

    /// (a) A fresh database: empty content caches, the documented default cap,
    ///     and a total that is exactly the sum of the four categories.
    #[tokio::test]
    async fn fresh_db_reports_empty_categories_and_default_limit() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();

        let usage = measure(&db, dir.path()).await.unwrap();

        for (name, cat) in [
            ("bodies", &usage.bodies),
            ("attachments", &usage.attachments),
            ("draftCache", &usage.draft_cache),
        ] {
            assert_eq!(cat.bytes, 0, "{name} bytes on a fresh database");
            assert_eq!(cat.items, 0, "{name} items on a fresh database");
        }
        // The schema itself is real metadata, not zero, and fully accounted for.
        assert!(usage.metadata.bytes > 0, "schema pages are metadata");
        assert_eq!(usage.metadata.items, 0);
        assert_eq!(usage.attachment_cache_limit_bytes, 512 * 1024 * 1024);
        assert_eq!(usage.total_bytes, total_of(&usage));
        assert!(usage.computed_at > 0);
    }

    /// (b) Row-cached attachment bytes are measured, not estimated.
    #[tokio::test]
    async fn stored_attachment_bytes_are_measured() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let aid = seed_message(&db, "a@x.com", "m1").await;

        let before = measure(&db, dir.path()).await.unwrap();
        assert_eq!(before.attachments.bytes, 0);

        put_cached(&db, &aid, "m1", vec![7u8; 4096]).await;

        let after = measure(&db, dir.path()).await.unwrap();
        assert!(
            after.attachments.bytes > 0,
            "stored payload bytes are counted"
        );
        assert_eq!(after.attachments.items, 1);
        assert_eq!(after.bodies.bytes, 0);
        assert_eq!(after.total_bytes, total_of(&after));

        // The residual bucket is exact: the SQLite-resident categories plus
        // metadata add up to the database file's whole page footprint, so
        // nothing is double counted or lost.
        let page_bytes: i64 = db
            .read(|c| {
                let pages: i64 = c.query_row("PRAGMA page_count", [], |r| r.get(0))?;
                let size: i64 = c.query_row("PRAGMA page_size", [], |r| r.get(0))?;
                Ok(pages * size)
            })
            .await
            .unwrap();
        assert_eq!(
            after.metadata.bytes + after.bodies.bytes + after.attachments.bytes,
            page_bytes
        );
        assert!(before.metadata.bytes > 0);
    }

    /// (c) Clearing removes the cache file and the row's cached payload while
    ///     keeping the attachment metadata row and the mail.
    #[tokio::test]
    async fn clear_removes_cache_files_and_resets_rows() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let aid = seed_message(&db, "a@x.com", "m1").await;
        let key = put_cached(&db, &aid, "m1", vec![1, 2, 3]).await;

        // Publish a real cache file where the attachment service would.
        let adir = cache::attachment_dir(dir.path(), &aid, "att-2").unwrap();
        let file = cache::write_atomic(&adir, "invoice.pdf", &vec![9u8; 8192])
            .await
            .unwrap();
        db.attachment_set_ready(&key, &file.to_string_lossy(), 8192)
            .await
            .unwrap();

        let before = measure(&db, dir.path()).await.unwrap();
        assert!(
            before.attachments.bytes >= 8192,
            "on-disk cache file is measured, got {}",
            before.attachments.bytes
        );
        assert_eq!(before.attachments.items, 1);

        let after = clear_attachment_cache(&db, dir.path()).await.unwrap();

        assert!(!file.exists(), "cache file removed from disk");
        assert_eq!(after.attachments.bytes, 0, "no residual attachment bytes");
        assert_eq!(after.attachments.items, 1, "metadata row preserved");
        assert_eq!(after.total_bytes, total_of(&after));

        let rec = db.attachment_get(&key).await.unwrap().unwrap();
        assert_eq!(rec.cache_state, CacheState::Missing);
        assert!(rec.local_path.is_none());
        assert!(rec.data.is_none());
        let metas = db
            .attachments_for_message(&MessageRef::new(&aid, "m1"))
            .await
            .unwrap();
        assert_eq!(metas.len(), 1, "attachment metadata survives the clear");
    }

    /// (d) The cap setting parses common units; junk falls back to the default.
    #[test]
    fn cache_limit_parses_common_units() {
        assert_eq!(parse_cache_limit("2GB"), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_cache_limit("2gb"), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_cache_limit("2GiB"), 2 * 1024 * 1024 * 1024);
        assert_eq!(parse_cache_limit("512MB"), 512 * 1024 * 1024);
        assert_eq!(parse_cache_limit(" 500 MB "), 500 * 1024 * 1024);
        assert_eq!(parse_cache_limit("1024KB"), 1024 * 1024);
        assert_eq!(parse_cache_limit("2048B"), 2048);
        assert_eq!(parse_cache_limit("1.5GB"), 1_610_612_736);
        assert_eq!(parse_cache_limit("0MB"), 0);
        for junk in ["", "   ", "big", "1GB extra", "-1GB", "12QB", "1e9MB"] {
            assert_eq!(
                parse_cache_limit(junk),
                DEFAULT_ATTACHMENT_CACHE_LIMIT_BYTES,
                "{junk:?} should fall back"
            );
        }
    }

    /// Draft staging files count towards `draftCache`, and the configured
    /// setting (not a constant) drives the reported cap.
    #[tokio::test]
    async fn draft_staging_files_and_configured_limit_are_measured() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let staging = dir.path().join(DRAFT_CACHE_DIR);
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("draft-1-invoice.pdf"), vec![4u8; 7777]).unwrap();

        let default = measure(&db, dir.path()).await.unwrap();
        assert_eq!(default.draft_cache.bytes, 7777);
        assert_eq!(default.draft_cache.items, 1);
        assert_eq!(default.attachment_cache_limit_bytes, 512 * 1024 * 1024);

        db.settings_set(serde_json::json!({ "attachmentCacheSize": "2GB" }))
            .await
            .unwrap();
        let configured = measure(&db, dir.path()).await.unwrap();
        assert_eq!(
            configured.attachment_cache_limit_bytes,
            2 * 1024 * 1024 * 1024
        );
        assert_eq!(configured.total_bytes, total_of(&configured));
    }
}
