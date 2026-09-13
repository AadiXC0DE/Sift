//! macOS download-quarantine metadata (P2.3).
//!
//! Files Sift writes into its cache, and copies it hands to the user, carry the
//! same `com.apple.quarantine` attribute macOS gives any downloaded file, so
//! Gatekeeper can still evaluate them. Sift never removes that attribute to
//! make an open succeed.
//!
//! Implemented with `/usr/bin/xattr` rather than FFI: this crate forbids
//! `unsafe`, and the adapter is deliberately tiny and best-effort — a failure
//! to write the attribute must not fail a download.

use std::path::Path;
use std::process::Command;

/// Read the raw quarantine attribute, if present.
pub fn read(path: &Path) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let out = Command::new("/usr/bin/xattr")
        .arg("-p")
        .arg("com.apple.quarantine")
        .arg(path)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let value = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// Apply quarantine to `path`. An existing attribute inherited from a source
/// file (a user's saved copy of a cache file) is written back verbatim; a new
/// value is generated otherwise. Best-effort: returns whether the attribute is
/// present afterwards.
pub fn apply(path: &Path, inherited: Option<&str>) -> bool {
    if !cfg!(target_os = "macos") {
        return false;
    }
    let value = match inherited {
        Some(v) if !v.is_empty() => v.to_string(),
        _ => format!("0081;{:x};Sift;{}", now_hex(), uuid::Uuid::now_v7().simple()),
    };
    let ok = Command::new("/usr/bin/xattr")
        .arg("-w")
        .arg("com.apple.quarantine")
        .arg(&value)
        .arg(path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        log::debug!("quarantine attribute not applied to {}", path.display());
    }
    ok || read(path).is_some()
}

/// Seconds since the macOS epoch (2001-01-01) in hex, the format the
/// quarantine attribute expects.
fn now_hex() -> u64 {
    const MAC_EPOCH_OFFSET: u64 = 978_307_200;
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().saturating_sub(MAC_EPOCH_OFFSET))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarantine_roundtrip_on_macos() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("downloaded.bin");
        std::fs::write(&file, b"x").unwrap();
        assert!(read(&file).is_none());
        assert!(apply(&file, None), "attribute written");
        let value = read(&file).expect("attribute present");
        assert!(value.contains(";Sift;"), "{value}");
        // An inherited value is preserved verbatim, never replaced with a
        // fresh one and never cleared.
        let inherited = "0081;5f000000;OtherApp;ABCD-1234";
        assert!(apply(&file, Some(inherited)));
        assert_eq!(read(&file).as_deref(), Some(inherited));
    }
}
