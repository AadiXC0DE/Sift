//! Cached raw MIME for View Source and Save as `.eml` (P9.3).
//!
//! The bytes live on disk, never in SQLite: a raw message can be many
//! megabytes and the database is the app's hot path. The row records the path,
//! the size and the digest, so an export can prove it wrote exactly the bytes
//! that were cached, and a cache file that changed underneath Sift is detected
//! rather than exported silently.
//!
//! Nothing here ever converts bytes to text. That conversion happens once, at
//! the UI, where a lossy decode can be *labelled* as lossy; the export path is
//! byte-oriented end to end.

use super::Db;
use anyhow::Result;
use rusqlite::{params, OptionalExtension};
use std::path::{Path, PathBuf};

/// One cached raw message.
#[derive(Debug, Clone)]
pub struct RawCacheRow {
    pub account_id: String,
    pub message_id: String,
    pub path: String,
    pub size: i64,
    pub sha256: String,
}

/// Where the raw cache for an account lives. Account-scoped like the
/// attachment cache, so removing an account can remove exactly its files.
pub fn raw_dir(data_dir: &Path, account_id: &str) -> PathBuf {
    data_dir.join("raw").join(safe_component(account_id))
}

/// A message id is a provider string; it is never allowed to steer the path.
pub fn raw_file_name(message_id: &str) -> String {
    let safe: String = message_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let capped = if safe.len() > 120 {
        &safe[..120]
    } else {
        &safe[..]
    };
    format!("{capped}.eml")
}

fn safe_component(value: &str) -> String {
    let safe: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.is_empty() {
        "_".into()
    } else if safe.len() > 80 {
        safe[..80].to_string()
    } else {
        safe
    }
}

/// Lowercase hex SHA-256, implemented here because the export path must be
/// able to state the digest of what it wrote without pulling in a dependency.
pub fn sha256_hex(bytes: &[u8]) -> String {
    // FIPS 180-4 constants.
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut message = bytes.to_vec();
    let bit_len = (bytes.len() as u64).wrapping_mul(8);
    message.push(0x80);
    while message.len() % 64 != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());
    for chunk in message.chunks(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    let mut out = String::with_capacity(64);
    for word in h {
        out.push_str(&format!("{word:08x}"));
    }
    out
}

impl Db {
    pub async fn raw_cache_get(
        &self,
        account_id: &str,
        message_id: &str,
    ) -> Result<Option<RawCacheRow>> {
        let (a, m) = (account_id.to_string(), message_id.to_string());
        self.read(move |c| {
            Ok(c.query_row(
                "SELECT account_id,message_id,path,size,sha256 FROM message_raw \
                 WHERE account_id=? AND message_id=?",
                params![a, m],
                |r| {
                    Ok(RawCacheRow {
                        account_id: r.get(0)?,
                        message_id: r.get(1)?,
                        path: r.get(2)?,
                        size: r.get(3)?,
                        sha256: r.get(4)?,
                    })
                },
            )
            .optional()?)
        })
        .await
    }

    pub async fn raw_cache_put(&self, row: &RawCacheRow) -> Result<()> {
        let row = row.clone();
        let now = super::now_ms();
        self.write(move |c| {
            c.execute(
                "INSERT INTO message_raw (account_id,message_id,path,size,sha256,cached_at,last_accessed_at) \
                 VALUES (?1,?2,?3,?4,?5,?6,?6) \
                 ON CONFLICT(account_id,message_id) DO UPDATE SET path=?3, size=?4, sha256=?5, cached_at=?6",
                params![row.account_id, row.message_id, row.path, row.size, row.sha256, now],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn raw_cache_touch(&self, account_id: &str, message_id: &str) -> Result<()> {
        let (a, m) = (account_id.to_string(), message_id.to_string());
        let now = super::now_ms();
        self.write(move |c| {
            c.execute(
                "UPDATE message_raw SET last_accessed_at=?3 WHERE account_id=?1 AND message_id=?2",
                params![a, m, now],
            )?;
            Ok(())
        })
        .await
    }

    pub async fn raw_cache_remove(&self, account_id: &str, message_id: &str) -> Result<()> {
        let (a, m) = (account_id.to_string(), message_id.to_string());
        self.write(move |c| {
            c.execute(
                "DELETE FROM message_raw WHERE account_id=? AND message_id=?",
                params![a, m],
            )?;
            Ok(())
        })
        .await
    }

    /// Bytes and rows held by the raw cache, for the storage report.
    pub async fn raw_cache_usage(&self) -> Result<(i64, i64)> {
        self.read(move |c| {
            let (bytes, items): (i64, i64) = c.query_row(
                "SELECT COALESCE(SUM(size),0), COUNT(*) FROM message_raw",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            Ok((bytes, items))
        })
        .await
    }

    /// Oldest-accessed raw rows first, for the retention pass.
    pub async fn raw_cache_lru(&self, limit: i64) -> Result<Vec<RawCacheRow>> {
        self.read(move |c| {
            let mut s = c.prepare(
                "SELECT account_id,message_id,path,size,sha256 FROM message_raw \
                 ORDER BY last_accessed_at LIMIT ?",
            )?;
            let rows = s
                .query_map(params![limit], |r| {
                    Ok(RawCacheRow {
                        account_id: r.get(0)?,
                        message_id: r.get(1)?,
                        path: r.get(2)?,
                        size: r.get(3)?,
                        sha256: r.get(4)?,
                    })
                })?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_matches_the_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"The quick brown fox jumps over the lazy dog"),
            "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
        // Binary bytes are not special-cased: the raw export hashes them.
        assert_eq!(sha256_hex(&[0x00, 0xff, 0x10]).len(), 64);
    }

    #[test]
    fn raw_paths_cannot_escape_the_cache_root() {
        assert_eq!(raw_file_name("18f2a/../../etc"), "18f2a_______etc.eml");
        assert_eq!(raw_file_name(""), ".eml");
        assert_eq!(safe_component(".."), "__");
    }
}
