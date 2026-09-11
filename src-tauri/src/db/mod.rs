use anyhow::Result;
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
#[cfg(test)]
use rusqlite::params;
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub mod accounts;
pub mod attachments;
pub mod bodies;
pub mod contacts;
pub mod drafts;
pub mod fts;
pub mod imap;
pub mod labels;
pub mod messages;
pub mod outbox;
pub mod settings;
pub mod threads;

static MIGRATIONS: &[(&str, &str)] = &[
    ("0001_init", include_str!("migrations/0001_init.sql")),
    (
        "0002_contacts_backfill",
        include_str!("migrations/0002_contacts_backfill.sql"),
    ),
    ("0003_imap", include_str!("migrations/0003_imap.sql")),
    (
        "0004_mail_rendering",
        include_str!("migrations/0004_mail_rendering.sql"),
    ),
    (
        "0005_remote_images_default",
        include_str!("migrations/0005_remote_images_default.sql"),
    ),
    (
        "0006_rerender_bodies",
        include_str!("migrations/0006_rerender_bodies.sql"),
    ),
];

#[derive(Clone)]
pub struct Db {
    pool: Pool<SqliteConnectionManager>,
    write_lock: Arc<tokio::sync::Mutex<()>>,
    #[allow(dead_code)]
    pub path: PathBuf,
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn apply_pragmas(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;
     PRAGMA temp_store=MEMORY; PRAGMA mmap_size=268435456; PRAGMA cache_size=-65536;
     PRAGMA busy_timeout=5000;",
    )?;
    Ok(())
}

/// Startup repair for the outbox. An op left `inflight` means the app exited
/// mid-send, so requeue it. Ops whose account no longer exists are orphaned by
/// account removal and would otherwise count as "pending" forever.
fn recover_outbox(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "UPDATE outbox_ops SET state='pending', attempts=0, not_before=0, last_error=NULL WHERE state='inflight';
         DELETE FROM outbox_ops WHERE account_id NOT IN (SELECT id FROM accounts);",
    )?;
    Ok(())
}

fn run_migrations(conn: &Connection) -> Result<()> {
    // Ensure schema_version exists (fresh DB has no tables)
    let has_version: bool = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='schema_version'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .map(|c| c > 0)
        .unwrap_or(false);
    if !has_version {
        conn.execute_batch(MIGRATIONS[0].1)?;
        // mark 0002 applied state: check if triggers exist; run 0002 (IF NOT EXISTS so idempotent)
        let _ = conn.execute_batch(MIGRATIONS[1].1);
        conn.execute_batch(MIGRATIONS[2].1)?;
        conn.execute("UPDATE schema_version SET version=4", [])?;
        conn.execute_batch(MIGRATIONS[3].1)?;
        conn.execute("UPDATE schema_version SET version=5", [])?;
        let _ = conn.execute_batch(MIGRATIONS[4].1);
        conn.execute_batch(MIGRATIONS[5].1)?;
        conn.execute("UPDATE schema_version SET version=6", [])?;
        return Ok(());
    }
    let v: i64 = conn
        .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
        .unwrap_or(1);
    if v < 2 {
        let _ = conn.execute_batch(MIGRATIONS[1].1);
        conn.execute("UPDATE schema_version SET version=2", [])?;
    }
    if v < 3 {
        // Explicit transaction: ALTER TABLE is not idempotent, so a crash
        // mid-migration must roll back for a clean retry.
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let r = conn.execute_batch(MIGRATIONS[2].1);
        if r.is_ok() {
            conn.execute("UPDATE schema_version SET version=3", [])?;
            conn.execute_batch("COMMIT")?;
        } else {
            let _ = conn.execute_batch("ROLLBACK");
            r?;
        }
    }
    if v < 4 {
        conn.execute_batch(MIGRATIONS[3].1)?;
        conn.execute("UPDATE schema_version SET version=4", [])?;
    }
    if v < 5 {
        conn.execute_batch(MIGRATIONS[4].1)?;
        conn.execute("UPDATE schema_version SET version=5", [])?;
    }
    if v < 6 {
        conn.execute_batch(MIGRATIONS[5].1)?;
        conn.execute("UPDATE schema_version SET version=6", [])?;
    }
    Ok(())
}

impl Db {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("sift.db");
        let manager = SqliteConnectionManager::file(&path);
        let pool = Pool::builder().max_size(4).build(manager)?;
        {
            let conn = pool.get()?;
            apply_pragmas(&conn)?;
            run_migrations(&conn)?;
            recover_outbox(&conn)?;
            apply_pragmas(&conn)?;
        }
        Ok(Self {
            pool,
            write_lock: Arc::new(tokio::sync::Mutex::new(())),
            path,
        })
    }

    pub fn open_in_memory() -> Result<Self> {
        // For tests that want isolation without tempdir plumbing we still use tempdir-backed file
        // because FTS5 + WAL behave differently on :memory:. Caller passes a temp dir.
        Err(anyhow::anyhow!("use Db::open with a temp dir"))
    }

    pub async fn read<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| anyhow::anyhow!(e.to_string()))?;
            f(&conn)
        })
        .await?
    }

    pub async fn write<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let _guard = self.write_lock.lock().await;
        let pool = self.pool.clone();
        // Hold the async guard across spawn_blocking via an owned permit: instead hold a
        // synchronous mutex around the actual write. We keep the async lock held by
        // blocking the thread briefly - acceptable and guarantees serialization.
        let res = tokio::task::spawn_blocking(move || {
            let conn = pool.get().map_err(|e| anyhow::anyhow!(e.to_string()))?;
            f(&conn)
        })
        .await?;
        drop(_guard);
        res
    }

    pub fn pool(&self) -> &Pool<SqliteConnectionManager> {
        &self.pool
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn p1_t01_creates_schema() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let conn = db.pool.get().unwrap();
        let v: i64 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, 6);
        for t in [
            "accounts",
            "labels",
            "threads",
            "messages",
            "message_labels",
            "bodies",
            "attachments",
            "contacts",
            "outbox_ops",
            "drafts",
            "snoozes",
            "sender_prefs",
            "settings",
            "sync_log",
        ] {
            let c: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?",
                    params![t],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(c, 1, "missing table {t}");
        }
    }
    #[test]
    fn p1_t02_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let _ = Db::open(dir.path()).unwrap();
        let _ = Db::open(dir.path()).unwrap();
    }

    #[tokio::test]
    async fn outbox_recovery_requeues_inflight_and_purges_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let acc = db
            .new_account("outbox@example.com", None, None)
            .await
            .unwrap();
        let aid = acc.id.clone();
        db.outbox_enqueue(
            &aid,
            "modify_labels",
            "{\"add\":[],\"remove\":[\"UNREAD\"]}",
            None,
            0,
        )
        .await
        .unwrap();
        db.outbox_enqueue(
            &aid,
            "modify_labels",
            "{\"add\":[\"STARRED\"],\"remove\":[]}",
            None,
            0,
        )
        .await
        .unwrap();
        // Simulate a send interrupted by app exit.
        let op = db.outbox_next(&aid).await.unwrap().unwrap();
        db.outbox_set(op.id, "inflight", 0, 0, None).await.unwrap();
        // Orphaned op from an account that no longer exists.
        db.outbox_enqueue("gone", "trash", "{}", None, 0)
            .await
            .unwrap();
        drop(db);

        let reopened = Db::open(dir.path()).unwrap();
        assert_eq!(reopened.outbox_pending_count(&aid).await.unwrap(), 2);
        assert_eq!(reopened.outbox_pending_count("gone").await.unwrap(), 0);
        let summary = reopened.outbox_summary(&aid).await.unwrap();
        assert!(
            summary.iter().any(|(l, _)| l == "Marking as read"),
            "{summary:?}"
        );
        assert!(summary.iter().any(|(l, _)| l == "Starring"), "{summary:?}");
    }

    #[tokio::test]
    async fn rendering_upgrade_invalidates_cached_bodies() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let account = db
            .new_account("mail@example.com", None, None)
            .await
            .unwrap();
        let account_id = account.id;
        db.write(move |connection| {
            connection.execute(
                "INSERT INTO messages (id,account_id,thread_id,internal_date,body_state) VALUES ('m1',?,'t1',1,'fetched')",
                params![account_id],
            )?;
            Ok(())
        })
        .await
        .unwrap();
        db.bodies_put(crate::db::bodies::BodyPut {
            message_id: "m1".into(),
            html: Some("<p>cached mail</p>".into()),
            text: Some("cached mail".into()),
            remote_images: 0,
            trackers: 0,
            dark_safe: true,
            quoted_from: None,
        })
        .await
        .unwrap();
        db.write(|connection| {
            connection.execute("UPDATE schema_version SET version=5", [])?;
            Ok(())
        })
        .await
        .unwrap();
        drop(db);

        // 0006 drops bodies rendered by older builds and lets them refetch, so
        // the leaked <title> and stale layout in cached mail are re-rendered.
        let reopened = Db::open(dir.path()).unwrap();
        assert!(reopened.bodies_get("m1").await.unwrap().is_none());
        let conn = reopened.pool.get().unwrap();
        let state: String = conn
            .query_row("SELECT body_state FROM messages WHERE id='m1'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(state, "none");
    }
    #[test]
    fn p1_t03_pragmas() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let conn = db.pool.get().unwrap();
        let jm: String = conn
            .query_row("PRAGMA journal_mode", [], |r| r.get(0))
            .unwrap();
        assert_eq!(jm.to_lowercase(), "wal");
        let fk: i64 = conn
            .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fk, 1);
    }
    #[tokio::test]
    async fn p1_t04_write_lane_serializes() {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        db.write(|c| {
            c.execute(
                "CREATE TABLE IF NOT EXISTS t_lane (id INTEGER PRIMARY KEY, v INTEGER)",
                [],
            )?;
            c.execute("DELETE FROM t_lane", [])?;
            Ok(())
        })
        .await
        .unwrap();
        let mut hs = vec![];
        for i in 0..100 {
            let d = db.clone();
            hs.push(tokio::spawn(async move {
                d.write(move |c| {
                    c.execute("INSERT INTO t_lane (v) VALUES (?)", params![i])?;
                    Ok(())
                })
                .await
                .unwrap()
            }));
        }
        for h in hs {
            h.await.unwrap();
        }
        let n: i64 = db
            .read(|c| Ok(c.query_row("SELECT count(*) FROM t_lane", [], |r| r.get(0))?))
            .await
            .unwrap();
        assert_eq!(n, 100);
    }
}
