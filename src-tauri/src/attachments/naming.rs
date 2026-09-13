//! Attachment filenames (P2.3).
//!
//! Two distinct values come out of here:
//!
//! * the **display name** — the sender's original filename, kept verbatim in
//!   the row (`AttachmentRecord::filename`) and echoed to the UI;
//! * the **basename** — a single safe path component used for the cache file
//!   and offered as the Save As default. Path components, control characters
//!   and separators are removed, `.`/`..` are rejected, and the result is
//!   capped at [`MAX_BASENAME_BYTES`] bytes of UTF-8 without splitting a
//!   character, preserving the extension.

use std::path::Path;

/// Cache/display basename cap. Well below HFS+/APFS's 255-byte component
/// limit so appending `.part-<uuid>` keeps the temp name legal.
pub const MAX_BASENAME_BYTES: usize = 180;

/// Strip everything that must never reach a filesystem: other path components,
/// NUL and control characters, then separators. Returns `None` when nothing
/// usable is left (empty, `.`, `..`).
pub fn sanitize(raw: &str) -> Option<String> {
    let last = raw.rsplit(['/', '\\']).next().unwrap_or(raw);
    let mut cleaned = String::with_capacity(last.len());
    for ch in last.chars() {
        if ch == '\0' || ch.is_control() {
            continue;
        }
        match ch {
            '/' | '\\' => cleaned.push('_'),
            _ => cleaned.push(ch),
        }
    }
    let trimmed = cleaned.trim_end_matches(' ');
    if trimmed.is_empty() || trimmed == "." || trimmed == ".." {
        return None;
    }
    Some(trimmed.to_string())
}

/// Truncate to `max` bytes on a character boundary, preserving the extension
/// when it fits. Returns an empty string when no usable stem is left.
pub fn cap_bytes(name: &str, max: usize) -> String {
    if name.len() <= max {
        return name.to_string();
    }
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty() && !e.contains('.'))
        .map(|e| format!(".{e}"))
        .filter(|e| e.len() + 1 < max);
    let (stem, ext) = match ext {
        Some(e) => (&name[..name.len() - e.len()], e),
        None => (name, String::new()),
    };
    let budget = max.saturating_sub(ext.len());
    let mut end = stem.len().min(budget);
    while end > 0 && !stem.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{}", &stem[..end], ext)
}

/// The safe single-component name for the cache file, and the default name a
/// Save As prompt should offer. Falls back to `attachment-<short-id>.<ext>`
/// for missing or unusable names.
pub fn basename(filename: Option<&str>, mime: &str, attachment_id: &str) -> String {
    if let Some(name) = filename.and_then(sanitize) {
        let capped = cap_bytes(&name, MAX_BASENAME_BYTES);
        if !capped.is_empty() && capped != "." && capped != ".." {
            return capped;
        }
    }
    let short: String = attachment_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(8)
        .collect();
    let short = if short.is_empty() {
        "0".to_string()
    } else {
        short
    };
    format!("attachment-{short}.{}", known_extension(mime).unwrap_or("bin"))
}

/// Lowercased extension of a sanitized name, if any.
pub fn extension(name: &str) -> Option<String> {
    Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .filter(|e| !e.is_empty())
}

/// A known extension for a MIME type, used only when the sender gave no name.
pub fn known_extension(mime: &str) -> Option<&'static str> {
    let mime = mime.split(';').next().unwrap_or(mime).trim().to_ascii_lowercase();
    Some(match mime.as_str() {
        "application/pdf" => "pdf",
        "application/zip" => "zip",
        "application/gzip" | "application/x-gzip" => "gz",
        "application/x-tar" => "tar",
        "application/json" => "json",
        "application/xml" | "text/xml" => "xml",
        "application/msword" => "doc",
        "application/vnd.ms-excel" => "xls",
        "application/vnd.ms-powerpoint" => "ppt",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => "docx",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => "xlsx",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation" => "pptx",
        "message/rfc822" => "eml",
        "image/png" => "png",
        "image/jpeg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/heic" => "heic",
        "image/tiff" => "tiff",
        "image/bmp" => "bmp",
        "image/svg+xml" => "svg",
        "text/plain" => "txt",
        "text/csv" => "csv",
        "text/html" => "html",
        "text/calendar" => "ics",
        "text/vcard" => "vcf",
        "audio/mpeg" => "mp3",
        "audio/mp4" => "m4a",
        "video/mp4" => "mp4",
        "video/quicktime" => "mov",
        "application/vnd.apple.keynote" => "key",
        "application/vnd.apple.pages" => "pages",
        "application/vnd.apple.numbers" => "numbers",
        _ => return None,
    })
}

/// Extensions whose contents macOS will hand to an interpreter or installer.
/// These require an explicit confirmation result before a system open.
const EXECUTABLE_EXTENSIONS: &[&str] = &[
    "app", "action", "bash", "bat", "bin", "command", "com", "csh", "deb", "dmg", "dylib", "exe",
    "fish", "hta", "jar", "js", "jsc", "ksh", "lnk", "mpkg", "msi", "osa", "php", "pif", "pkg",
    "pl", "ps1", "py", "rb", "reg", "rpm", "run", "scpt", "scptd", "scr", "sh", "so", "terminal",
    "tool", "vbs", "wdgt", "workflow", "wsf", "xip", "zsh",
];

/// True when opening this name would execute code or run an installer.
pub fn is_executable_like(name: &str) -> bool {
    extension(name).is_some_and(|e| EXECUTABLE_EXTENSIONS.contains(&e.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_ordinary_names() {
        assert_eq!(basename(Some("invoice.pdf"), "application/pdf", "x"), "invoice.pdf");
    }

    #[test]
    fn strips_path_components() {
        assert_eq!(basename(Some("../../x"), "", "x"), "x");
        assert_eq!(basename(Some("/etc/passwd"), "", "x"), "passwd");
        assert_eq!(basename(Some("..\\..\\evil.exe"), "", "x"), "evil.exe");
    }

    #[test]
    fn rejects_dot_names_and_controls() {
        assert_eq!(basename(Some(".."), "image/png", "abcd1234"), "attachment-abcd1234.png");
        assert_eq!(basename(Some("."), "image/png", "abcd1234"), "attachment-abcd1234.png");
        assert_eq!(basename(Some("re\u{1}port\u{0}.pdf"), "", "x"), "report.pdf");
        assert_eq!(basename(None, "application/pdf", "abcd1234"), "attachment-abcd1234.pdf");
        assert_eq!(basename(Some("   "), "application/zip", "abcd1234"), "attachment-abcd1234.zip");
        assert_eq!(basename(None, "application/x-weird", "abcd1234"), "attachment-abcd1234.bin");
    }

    #[test]
    fn keeps_unicode_and_quotes() {
        assert_eq!(basename(Some("Résumé \"final\".pdf"), "", "x"), "Résumé \"final\".pdf");
    }

    #[test]
    fn trims_unusable_trailing_spaces() {
        assert_eq!(basename(Some("trailing.pdf   "), "", "x"), "trailing.pdf");
    }

    #[test]
    fn caps_without_splitting_a_character_and_keeps_extension() {
        let long = format!("{}.pdf", "é".repeat(200));
        let out = basename(Some(&long), "", "x");
        assert!(out.len() <= MAX_BASENAME_BYTES, "{}", out.len());
        assert!(out.ends_with(".pdf"));
        assert!(out.is_char_boundary(out.len()));
        // no extension to preserve: still a valid capped name
        let long = "a".repeat(400);
        let out = basename(Some(&long), "", "x");
        assert_eq!(out.len(), MAX_BASENAME_BYTES);
    }

    #[test]
    fn detects_executables() {
        assert!(is_executable_like("Installer.DMG"));
        assert!(is_executable_like("run.command"));
        assert!(is_executable_like("script.sh"));
        assert!(!is_executable_like("invoice.pdf"));
        assert!(!is_executable_like("photo.jpeg"));
    }
}
