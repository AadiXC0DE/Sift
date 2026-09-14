//! Upgrade acceptance against a real, populated mailbox.
//!
//! The offline fixture tests build a database from the historical migration
//! SQL. That proves the SQL runs; it does not prove that a mailbox someone has
//! actually been using — tens of thousands of messages, thousands of attachment
//! rows, a live FTS index — survives the same path. This test migrates a real
//! database and checks the things that would be catastrophic to lose.
//!
//! It is ignored by default and takes the database path from the environment,
//! so it can never run against the user's live file by accident:
//!
//! ```text
//! SIFT_UPGRADE_DB="/path/to/copy-of-sift.db" \
//! cargo test --manifest-path src-tauri/Cargo.toml \
//!   --test upgrade_real_database -- --ignored --nocapture
//! ```
//!
//! Always point it at a COPY. The test refuses to run when the path is inside
//! the live Application Support directory.

use sift::db::Db;

#[tokio::test]
#[ignore = "migrates a real database: run explicitly with SIFT_UPGRADE_DB pointing at a copy"]
async fn p10_t05_a_real_mailbox_survives_the_upgrade() {
    let Some(path) = std::env::var("SIFT_UPGRADE_DB").ok() else {
        eprintln!("SKIPPED: set SIFT_UPGRADE_DB to a COPY of a real sift.db");
        return;
    };
    let path = std::path::PathBuf::from(&path);
    assert!(path.exists(), "SIFT_UPGRADE_DB must exist: {path:?}");

    let live_marker = format!(
        "{}/Library/Application Support",
        std::env::var("HOME").unwrap_or_default()
    );
    assert!(
        !path.to_string_lossy().contains(&live_marker),
        "refusing to migrate a database inside Application Support: point SIFT_UPGRADE_DB at a copy"
    );

    // Count what must survive, before the migration touches anything.
    let before = count(&path);
    println!("\n=== real-database upgrade acceptance ===");
    for (k, v) in &before {
        println!("{k:<26} {v}");
    }

    let dir = path.parent().expect("parent dir");
    let db = Db::open(dir).expect("Db::open must migrate the real database");
    drop(db);

    let after = count(&path);
    for (k, v) in &before {
        println!("{k:<26} {v} -> {}", after[k]);
        assert_eq!(
            after[k], *v,
            "{k} changed across the migration: the upgrade must not lose mail"
        );
    }

    // The schema must now be current, and referential integrity must hold: the
    // composite-key rebuild is exactly where an orphan would appear.
    let version: i64 = query(&path, "SELECT version FROM schema_version", |r| r.get(0));
    assert_eq!(version, sift::db::SCHEMA_VERSION, "version must be current");
    let violations: i64 = query(
        &path,
        "SELECT count(*) FROM pragma_foreign_key_check",
        |r| r.get(0),
    );
    assert_eq!(
        violations, 0,
        "foreign_key_check must be empty after upgrade"
    );

    // The FTS index is rebuilt by hand during the account-scoping migration; a
    // silent partial rebuild would make search quietly wrong.
    let hits: i64 = query(
        &path,
        "SELECT count(*) FROM messages_fts WHERE messages_fts MATCH 'a*'",
        |r| r.get(0),
    );
    assert!(hits > 0, "the rebuilt FTS index must still return matches");
    println!("fts sample matches         {hits}");
    println!("foreign_key_check          clean");
    println!("schema version             {version}");
    println!("=== upgrade acceptance finished: no rows lost ===");
}

fn query<T: rusqlite::types::FromSql>(
    path: &std::path::Path,
    sql: &str,
    f: impl Fn(&rusqlite::Row) -> rusqlite::Result<T>,
) -> T {
    let conn = rusqlite::Connection::open(path).expect("open for verification");
    conn.query_row(sql, [], |r| f(r)).expect("query")
}

fn count(path: &std::path::Path) -> std::collections::BTreeMap<String, i64> {
    let conn = rusqlite::Connection::open(path).expect("open for counting");
    let mut out = std::collections::BTreeMap::new();
    for (label, sql) in [
        ("accounts", "SELECT count(*) FROM accounts"),
        ("threads", "SELECT count(*) FROM threads"),
        ("messages", "SELECT count(*) FROM messages"),
        ("message_labels", "SELECT count(*) FROM message_labels"),
        ("bodies", "SELECT count(*) FROM bodies"),
        ("attachments", "SELECT count(*) FROM attachments"),
        ("labels", "SELECT count(*) FROM labels"),
        ("imap_uids", "SELECT count(*) FROM imap_uids"),
        ("drafts", "SELECT count(*) FROM drafts"),
        ("outbox_ops", "SELECT count(*) FROM outbox_ops"),
    ] {
        out.insert(
            label.to_string(),
            conn.query_row(sql, [], |r| r.get(0)).unwrap_or(-1),
        );
    }
    out
}
