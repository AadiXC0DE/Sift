//! In-use leases for the attachment cache (P10.4).
//!
//! Eviction must not delete a file somebody is reading. The reader is always in
//! this process — a preview, a save, an inline image — so a process-local lease
//! is the honest mechanism: it is released exactly when the reader stops, even
//! if it stops by panicking or being dropped mid-flight.
//!
//! A lease is deliberately *not* persisted. After a crash nothing is reading
//! anything, so there is nothing to protect, and a persisted lease would only
//! be a lock that outlives its owner.

use std::collections::HashSet;
use std::sync::{LazyLock, Mutex};

/// Leases are held for as long as a reader is reading; the set is process-wide
/// because the cache is.
static LEASES: LazyLock<Mutex<HashSet<(String, String)>>> =
    LazyLock::new(|| Mutex::new(HashSet::new()));

fn leases() -> &'static Mutex<HashSet<(String, String)>> {
    &LEASES
}

fn key(account_id: &str, attachment_id: &str) -> (String, String) {
    (account_id.to_string(), attachment_id.to_string())
}

/// Hold a file open for the duration of a read.
pub struct Lease {
    key: (String, String),
}

impl Drop for Lease {
    fn drop(&mut self) {
        if let Ok(mut map) = leases().lock() {
            map.remove(&self.key);
        }
    }
}

/// Mark an attachment as being read right now.
pub fn acquire(account_id: &str, attachment_id: &str) -> Lease {
    let key = key(account_id, attachment_id);
    if let Ok(mut map) = leases().lock() {
        map.insert(key.clone());
    }
    Lease { key }
}

pub fn is_in_use(account_id: &str, attachment_id: &str) -> bool {
    leases()
        .lock()
        .map(|map| map.contains(&key(account_id, attachment_id)))
        .unwrap_or(false)
}

/// How many leases are outstanding, for diagnostics and tests.
pub fn in_use_count() -> usize {
    leases().lock().map(|map| map.len()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn p10_4_leases_track_readers_and_release_on_drop() {
        assert!(!is_in_use("a", "one"));
        {
            let _lease = acquire("a", "one");
            assert!(is_in_use("a", "one"));
            // Account-scoped: another account's identical id is untouched.
            assert!(!is_in_use("b", "one"));
        }
        assert!(!is_in_use("a", "one"));
        assert_eq!(in_use_count(), 0);
    }
}
