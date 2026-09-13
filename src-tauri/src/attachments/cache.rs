//! Attachment cache layout and atomic publication (P2.3).
//!
//! Layout: `<data_dir>/attachments/<account-id>/<attachment-id>/<basename>`.
//! Bytes are written to `<basename>.part-<uuid>` inside that directory with
//! `create_new`, flushed and synced, closed, and only then renamed into place;
//! the caller updates the database after the rename. A partially written or
//! abandoned temp file is removed by [`TempFile`]'s drop, so a failed write
//! never leaves a half-file at the final path.

use std::io;
use std::path::{Path, PathBuf};

use tokio::io::AsyncWriteExt;

fn unsafe_component(id: &str) -> bool {
    id.is_empty()
        || id == "."
        || id == ".."
        || id.contains('/')
        || id.contains('\\')
        || id.contains('\0')
}

/// Per-attachment cache directory. The ids are opaque provider/row keys, but
/// they are still checked so nothing can escape the cache root.
pub fn attachment_dir(data_dir: &Path, account_id: &str, attachment_id: &str) -> io::Result<PathBuf> {
    if unsafe_component(account_id) || unsafe_component(attachment_id) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "unsafe attachment cache id",
        ));
    }
    Ok(data_dir
        .join("attachments")
        .join(account_id)
        .join(attachment_id))
}

/// A temp file that is renamed into the cache, or removed on drop.
pub struct TempFile {
    path: PathBuf,
    file: Option<tokio::fs::File>,
    published: bool,
}

impl TempFile {
    pub async fn create(dir: &Path, basename: &str) -> io::Result<Self> {
        tokio::fs::create_dir_all(dir).await?;
        let path = dir.join(format!(
            "{basename}.part-{}",
            uuid::Uuid::now_v7().simple()
        ));
        let file = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await?;
        Ok(Self {
            path,
            file: Some(file),
            published: false,
        })
    }

    pub async fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        match self.file.as_mut() {
            Some(f) => f.write_all(bytes).await,
            None => Err(io::Error::other("temp file closed")),
        }
    }

    /// Flush, sync, close, then rename. Only after this returns may the row be
    /// marked `ready`.
    pub async fn publish(mut self, final_path: &Path) -> io::Result<()> {
        let mut file = self
            .file
            .take()
            .ok_or_else(|| io::Error::other("temp file closed"))?;
        file.flush().await?;
        file.sync_all().await?;
        drop(file);
        tokio::fs::rename(&self.path, final_path).await?;
        self.published = true;
        Ok(())
    }
}

impl Drop for TempFile {
    fn drop(&mut self) {
        if !self.published {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Write a complete payload atomically (row-cached bytes, small inline parts).
pub async fn write_atomic(dir: &Path, basename: &str, bytes: &[u8]) -> io::Result<PathBuf> {
    let mut temp = TempFile::create(dir, basename).await?;
    temp.write_all(bytes).await?;
    let final_path = dir.join(basename);
    temp.publish(&final_path).await?;
    Ok(final_path)
}

/// Copy a verified cache file to a user destination without ever exposing a
/// half-written file there: the copy lands in a sibling temp path and is
/// renamed over the destination only after it is complete.
pub async fn copy_atomic(src: &Path, dest: &Path) -> io::Result<()> {
    if let Some(parent) = dest.parent() {
        if !parent.as_os_str().is_empty() {
            tokio::fs::create_dir_all(parent).await?;
        }
    }
    let temp = PathBuf::from(format!("{}.part-{}", dest.display(), uuid::Uuid::now_v7().simple()));
    let result = async {
        tokio::fs::copy(src, &temp).await?;
        if let Ok(f) = tokio::fs::OpenOptions::new().write(true).open(&temp).await {
            f.sync_all().await?;
        }
        tokio::fs::rename(&temp, dest).await
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&temp).await;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_ids_cannot_escape_the_root() {
        let root = Path::new("/tmp/data");
        assert!(attachment_dir(root, "acct", "att").is_ok());
        for bad in ["..", "../x", "a/b", "a\\b", ""] {
            assert!(attachment_dir(root, bad, "att").is_err(), "{bad} accepted");
            assert!(attachment_dir(root, "acct", bad).is_err(), "{bad} accepted");
        }
    }

    #[tokio::test]
    async fn abandoned_temp_is_removed() {
        let dir = tempfile::tempdir().unwrap();
        let temp = TempFile::create(dir.path(), "invoice.pdf").await.unwrap();
        let path = temp.path.clone();
        assert!(path.exists());
        drop(temp);
        assert!(!path.exists(), "temp removed on drop");
    }

    #[tokio::test]
    async fn publish_replaces_existing_target() {
        let dir = tempfile::tempdir().unwrap();
        let first = write_atomic(dir.path(), "a.bin", b"one").await.unwrap();
        let second = write_atomic(dir.path(), "a.bin", b"two-two").await.unwrap();
        assert_eq!(first, second);
        assert_eq!(tokio::fs::read(&second).await.unwrap(), b"two-two");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".part-"))
            .collect();
        assert!(leftovers.is_empty(), "no temp files left behind");
    }

    #[tokio::test]
    async fn failed_copy_leaves_no_destination() {
        let dir = tempfile::tempdir().unwrap();
        let dest_dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.bin");
        let dest = dest_dir.path().join("out.bin");
        assert!(copy_atomic(&missing, &dest).await.is_err());
        assert!(!dest.exists());
        let leftovers: Vec<_> = std::fs::read_dir(dest_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .collect();
        assert!(leftovers.is_empty(), "no temp left at the destination");
    }
}
