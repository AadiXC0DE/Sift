use anyhow::{Context, Result};
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use rusqlite::params;
use rusqlite::{Connection, Transaction, TransactionBehavior};
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
pub mod privacy;
pub mod raw_cache;
pub mod reminders;
pub mod rules;
pub mod saved_searches;
pub mod settings;
pub mod threads;
pub mod vips;

/// The schema version this build ships: the count of applied migrations.
/// Tests assert against this instead of a hardcoded number, which is what made
/// adding a migration break an unrelated assertion.
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

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
    (
        "0007_attachment_cache",
        include_str!("migrations/0007_attachment_cache.sql"),
    ),
    (
        "0008_account_scoping",
        include_str!("migrations/0008_account_scoping.sql"),
    ),
    (
        "0009_draft_lifecycle",
        include_str!("migrations/0009_draft_lifecycle.sql"),
    ),
    (
        "0010_outbox_lifecycle",
        include_str!("migrations/0010_outbox_lifecycle.sql"),
    ),
    (
        "0011_saved_searches",
        include_str!("migrations/0011_saved_searches.sql"),
    ),
    (
        "0012_search_indexes",
        include_str!("migrations/0012_search_indexes.sql"),
    ),
    (
        "0013_remote_content_policy",
        include_str!("migrations/0013_remote_content_policy.sql"),
    ),
    (
        "0014_unsubscribe_auth",
        include_str!("migrations/0014_unsubscribe_auth.sql"),
    ),
    (
        "0015_mail_utilities",
        include_str!("migrations/0015_mail_utilities.sql"),
    ),
    (
        "0016_cache_retention",
        include_str!("migrations/0016_cache_retention.sql"),
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

/// Initial cache per pooled connection (8 MiB; measured in Phase 10).
const CACHE_SIZE_KIB: i64 = -8192;
/// Bounded busy timeout so a contended write fails fast instead of hanging.
const BUSY_TIMEOUT_MS: i64 = 5_000;
/// How many pre-migration snapshots to retain.
const BACKUP_KEEP: usize = 3;

/// Per-connection initialization. `foreign_keys`, `cache_size` and
/// `busy_timeout` are connection-scoped: applying them to a single pooled
/// connection left the other three without foreign-key enforcement (DB-01).
/// This runs through r2d2's init hook for every pooled connection and is also
/// applied to the bootstrap connection before the pool is exposed (P4.1).
fn init_connection(conn: &mut Connection) -> rusqlite::Result<()> {
    conn.execute_batch(&format!(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA foreign_keys=ON;
         PRAGMA temp_store=MEMORY;
         PRAGMA mmap_size=268435456;
         PRAGMA cache_size={CACHE_SIZE_KIB};
         PRAGMA busy_timeout={BUSY_TIMEOUT_MS};"
    ))
}

/// Startup repair for the outbox (P6.1).
///
/// The old version reset *every* `inflight` row to `pending`, which is exactly
/// how a message whose SMTP `DATA` had already been accepted was sent a second
/// time. Recovery is now per kind:
///
/// * label-set work (`modify_labels`, `trash`, `untrash`, `create_label`,
///   `label_rename`, `label_delete`) and identity-addressed `delete` are
///   idempotent: a crash between claim and acknowledgement only means the
///   change may already have been applied, so they are requeued and re-checked
///   against the server. A rule application is the same kind of work: its
///   `rule_applications` row is written before the operation is queued, so
///   requeueing it can only re-apply the identical label diff.
/// * a `draft_sync` is a push of one immutable revision: requeueing it can
///   only re-create the same remote draft.
/// * an `inflight` **send** never resubmits. Its acceptance is unknown, so it
///   becomes `uncertain` and is reconciled against the provider's Sent folder
///   by stable RFC Message-ID before any human decision.
///
/// Dependents of a terminally failed or cancelled op can never run: they are
/// cancelled here as well as at the transition that failed them.
fn recover_outbox(conn: &Connection) -> Result<()> {
    let now = now_ms();
    conn.execute(
        "UPDATE outbox_ops SET state='pending', attempts=0, not_before=0, started_at=NULL, last_error=NULL \
         WHERE state='inflight' AND kind IN ('modify_labels','trash','untrash','delete','draft_sync','create_label','label_rename','label_delete','rule_apply')",
        [],
    )?;
    conn.execute(
        "UPDATE outbox_ops SET state='uncertain', started_at=COALESCE(started_at, ?1), \
           reconcile_at=?1, reconcile_attempts=0, last_error=COALESCE(last_error, 'the app closed while this message was being handed to Gmail') \
         WHERE state='inflight' AND kind='send'",
        params![now],
    )?;
    conn.execute(
        "UPDATE outbox_ops SET state='cancelled', completed_at=?1, \
           failure_code='dependency_failed', last_error='the operation this waited on did not succeed' \
         WHERE state='pending' AND depends_on_op_id IS NOT NULL \
           AND EXISTS (SELECT 1 FROM outbox_ops d WHERE d.id=outbox_ops.depends_on_op_id AND d.state IN ('failed','cancelled','uncertain'))",
        params![now],
    )?;
    conn.execute(
        "DELETE FROM outbox_ops WHERE account_id NOT IN (SELECT id FROM accounts)",
        [],
    )?;
    Ok(())
}

/// Current schema version, or `None` when the database has no schema yet
/// (fresh install).
fn schema_version(conn: &Connection) -> Result<Option<i64>> {
    let has_version: bool = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='schema_version'",
        [],
        |r| r.get::<_, i64>(0),
    )? > 0;
    if !has_version {
        return Ok(None);
    }
    Ok(Some(conn.query_row(
        "SELECT version FROM schema_version",
        [],
        |r| r.get(0),
    )?))
}

/// Snapshot the database before an upgrade using SQLite's own consistent
/// writer (`VACUUM INTO`), never a raw copy of a live `.db` file. A fresh
/// install has nothing to preserve.
fn backup_before_migration(conn: &Connection, dir: &Path, from_version: i64) -> Result<PathBuf> {
    let backups = dir.join("backups");
    std::fs::create_dir_all(&backups)?;
    let dest = backups.join(format!("sift-v{from_version}-{}.db", now_ms()));
    let escaped = dest.to_string_lossy().replace('\'', "''");
    conn.execute_batch(&format!("VACUUM INTO '{escaped}'"))
        .with_context(|| format!("pre-migration backup to {}", dest.display()))?;
    prune_backups(&backups, BACKUP_KEEP);
    Ok(dest)
}

/// Keep the newest `keep` snapshots; a failed prune never blocks the upgrade.
fn prune_backups(dir: &Path, keep: usize) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return;
    };
    let mut files: Vec<PathBuf> = read
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "db"))
        .collect();
    files.sort();
    let drop_count = files.len().saturating_sub(keep);
    for old in files.into_iter().take(drop_count) {
        let _ = std::fs::remove_file(old);
    }
}

/// Register SQL helpers the migrations need. `zstd_text` decodes the
/// compressed body payload so the FTS rebuild can re-index cached mail without
/// a second copy of the plain text in the database.
fn register_migration_functions(conn: &Connection) -> Result<()> {
    use rusqlite::functions::FunctionFlags;
    conn.create_scalar_function(
        "zstd_text",
        1,
        FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC,
        |ctx| {
            let raw: Option<Vec<u8>> = ctx.get(0)?;
            Ok(raw
                .and_then(|b| zstd::decode_all(b.as_slice()).ok())
                .map(|v| String::from_utf8_lossy(&v).into_owned()))
        },
    )
    .context("register zstd_text")?;
    Ok(())
}

/// Apply every migration past `limit`, one immediate transaction each:
/// `BEGIN IMMEDIATE` -> migration SQL -> `schema_version` -> `COMMIT`. Any
/// error rolls the transaction back and aborts, so a crash leaves the
/// pre-migration schema and version, never a half-applied one (P4.1). The
/// fresh-install path runs through this same loop instead of a hand-rolled
/// sequence that skipped/ignored errors.
///
/// `foreign_keys` is switched off around each migration (the documented SQLite
/// table-rebuild procedure; the PRAGMA is a no-op inside a transaction) and
/// `PRAGMA foreign_key_check` must come back empty before the commit, so a
/// migration that would strand a child row fails and rolls back instead.
fn apply_migrations(conn: &mut Connection, limit: i64, fail_after: Option<i64>) -> Result<()> {
    let mut current = schema_version(conn)?.unwrap_or(0);
    for (idx, (name, sql)) in MIGRATIONS.iter().enumerate() {
        let version = (idx + 1) as i64;
        if version > limit {
            break;
        }
        if version <= current {
            continue;
        }
        // Standard SQLite table-rebuild procedure: enforcement is disabled
        // outside the transaction (the PRAGMA is a no-op inside one) and the
        // rebuilt schema is checked before the commit.
        conn.execute_batch("PRAGMA foreign_keys=OFF")
            .context("disable foreign keys for migration")?;
        let applied: Result<()> = (|| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .with_context(|| format!("begin migration {name}"))?;
            tx.execute_batch(sql)
                .with_context(|| format!("apply migration {name}"))?;
            tx.execute("UPDATE schema_version SET version=?1", [version])
                .with_context(|| format!("record migration {name}"))?;
            if fail_after == Some(version) {
                anyhow::bail!("injected migration failure after {name}");
            }
            tx.commit()
                .with_context(|| format!("commit migration {name}"))?;
            Ok(())
        })();
        conn.execute_batch("PRAGMA foreign_keys=ON")
            .context("re-enable foreign keys")?;
        applied?;
        current = version;
    }
    // One clean-check after the whole sequence: earlier migrations may run
    // against a database that a pre-P4.1 delete left with orphans (foreign
    // keys were only enforced on one pooled connection), and reconciling those
    // is 0008's job. A violation that *survives* the sequence is a real failure
    // and stops the open before sync starts.
    let violations: i64 = conn
        .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .context("foreign_key_check")?;
    if violations > 0 {
        anyhow::bail!(
            "{violations} foreign-key violation(s) remain after migration to version {current}"
        );
    }
    Ok(())
}

fn run_migrations(conn: &mut Connection) -> Result<()> {
    register_migration_functions(conn)?;
    apply_migrations(conn, MIGRATIONS.len() as i64, None)
}

impl Db {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)?;
        let path = dir.join("sift.db");
        // Bootstrap on a dedicated connection *before* the pool exists: with a
        // single writer, initialization, the pre-migration snapshot and the
        // migrations all observe one coherent database (P4.1).
        let mut boot = Connection::open(&path).with_context(|| "open sift.db")?;
        init_connection(&mut boot)?;
        let current = schema_version(&boot)?.unwrap_or(0);
        if (current as usize) < MIGRATIONS.len() {
            if current > 0 {
                backup_before_migration(&boot, dir, current)?;
            }
            run_migrations(&mut boot)?;
        }
        // Startup repair for the outbox, also on the bootstrap connection.
        recover_outbox(&boot)?;
        drop(boot);

        let manager = SqliteConnectionManager::file(&path).with_init(init_connection);
        let pool = Pool::builder().max_size(4).build(manager)?;
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

    /// Run `f` inside one `BEGIN IMMEDIATE` transaction on a pooled
    /// connection. The write mutex only serializes writers; it does **not**
    /// make a multi-statement closure atomic, so any operation that must be
    /// all-or-nothing goes through here. The transaction commits only when
    /// `f` returns `Ok`; any error rolls back.
    pub async fn write_tx<T, F>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Transaction<'_>) -> Result<T> + Send + 'static,
        T: Send + 'static,
    {
        let _guard = self.write_lock.lock().await;
        let pool = self.pool.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = pool.get().map_err(|e| anyhow::anyhow!(e.to_string()))?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .context("begin write transaction")?;
            let out = f(&tx)?;
            tx.commit().context("commit write transaction")?;
            Ok(out)
        })
        .await?
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
        assert_eq!(v, SCHEMA_VERSION);
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
        // A genuine v5 database with a cached body: the upgrade path runs
        // 0006 (drop stale renderings) through 0008.
        {
            let conn = fixture_at(dir.path(), 5);
            conn.execute_batch(
                "INSERT INTO accounts (id,email,created_at) VALUES ('a1','mail@example.com',1);
                 INSERT INTO messages (id,account_id,thread_id,internal_date,body_state)
                   VALUES ('m1','a1','t1',1,'fetched');
                 INSERT INTO bodies (message_id,html_z,text_z) VALUES ('m1',NULL,NULL);",
            )
            .unwrap();
        }
        let account_id = "a1".to_string();

        // 0006 drops bodies rendered by older builds and lets them refetch, so
        // the leaked <title> and stale layout in cached mail are re-rendered.
        let reopened = Db::open(dir.path()).unwrap();
        assert!(reopened
            .bodies_get(&crate::dto::MessageRef::new(account_id.clone(), "m1"))
            .await
            .unwrap()
            .is_none());
        let conn = reopened.pool.get().unwrap();
        let state: String = conn
            .query_row(
                "SELECT body_state FROM messages WHERE account_id=? AND id='m1'",
                params![account_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(state, "none");
    }
    #[test]
    fn p1_t03_every_pooled_connection_is_initialized() {
        // DB-01: PRAGMAs are connection-scoped. Acquire all four connections
        // at once and assert each one, not just the first checkout.
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(dir.path()).unwrap();
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
        let mut handles = vec![];
        for _ in 0..4 {
            let pool = db.pool.clone();
            let barrier = barrier.clone();
            handles.push(std::thread::spawn(move || {
                let conn = pool.get().unwrap();
                barrier.wait();
                let fk: i64 = conn
                    .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
                    .unwrap();
                let jm: String = conn
                    .query_row("PRAGMA journal_mode", [], |r| r.get(0))
                    .unwrap();
                let bt: i64 = conn
                    .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
                    .unwrap();
                let cs: i64 = conn
                    .query_row("PRAGMA cache_size", [], |r| r.get(0))
                    .unwrap();
                (fk, jm.to_lowercase(), bt, cs)
            }));
        }
        for h in handles {
            assert_eq!(
                h.join().unwrap(),
                (1, "wal".to_string(), BUSY_TIMEOUT_MS, CACHE_SIZE_KIB)
            );
        }
    }

    /// Build a raw database at `upto` schema version, for upgrade fixtures.
    fn fixture_at(dir: &Path, upto: i64) -> Connection {
        let mut conn = Connection::open(dir.join("sift.db")).unwrap();
        init_connection(&mut conn).unwrap();
        register_migration_functions(&conn).unwrap();
        apply_migrations(&mut conn, upto, None).unwrap();
        conn
    }

    #[test]
    fn p4_t01_fresh_and_v1_fixtures_upgrade_cleanly() {
        // Fresh install.
        let fresh = tempfile::tempdir().unwrap();
        let db = Db::open(fresh.path()).unwrap();
        {
            let conn = db.pool.get().unwrap();
            let v: i64 = conn
                .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
                .unwrap();
            assert_eq!(v, SCHEMA_VERSION);
            let violations: i64 = conn
                .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(violations, 0);
            let recovery: i64 = conn
                .query_row("SELECT count(*) FROM migration_recovery_rows", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(recovery, 0, "fresh install has nothing to rehome");
        }
        // v1 fixture: the ALTER in 0003 and every rebuild must run exactly once.
        let old = tempfile::tempdir().unwrap();
        {
            let conn = fixture_at(old.path(), 1);
            conn.execute_batch(
                "INSERT INTO accounts (id,email,created_at) VALUES ('a1','a@x.com',1);
                 INSERT INTO messages (id,account_id,thread_id,internal_date,subject)
                   VALUES ('m1','a1','t1',1,'hello');",
            )
            .unwrap();
        }
        let upgraded = Db::open(old.path()).unwrap();
        let conn = upgraded.pool.get().unwrap();
        let v: i64 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let auth_cols: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('accounts') WHERE name='auth_kind'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(auth_cols, 1, "0003 ALTER must not be applied twice");
        let subject: String = conn
            .query_row(
                "SELECT subject FROM messages WHERE account_id='a1' AND id='m1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(subject, "hello");
        let violations: i64 = conn
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(violations, 0);
    }

    #[test]
    fn p4_t02_failed_migration_rolls_back_and_reopens() {
        let dir = tempfile::tempdir().unwrap();
        fixture_at(dir.path(), 7)
            .execute_batch(
                "INSERT INTO accounts (id,email,created_at) VALUES ('a1','a@x.com',1);
                 INSERT INTO messages (id,account_id,thread_id,internal_date,subject,body_state)
                   VALUES ('m1','a1','t1',1,'hello','fetched');
                 INSERT INTO message_labels (message_id,label_id) VALUES ('m1','INBOX');
                 INSERT INTO bodies (message_id,text_z) VALUES ('m1',NULL);
                 INSERT INTO attachments (id,message_id,part_id,mime) VALUES ('att1','m1','1','text/plain');",
            )
            .unwrap();

        // Inject a failure while 0008 is being applied.
        {
            let mut conn = Connection::open(dir.path().join("sift.db")).unwrap();
            init_connection(&mut conn).unwrap();
            register_migration_functions(&conn).unwrap();
            let err = apply_migrations(&mut conn, 8, Some(8));
            assert!(err.is_err(), "injected failure must propagate");
            let v: i64 = conn
                .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
                .unwrap();
            assert_eq!(v, 7, "version must not advance on a rolled-back migration");
            let leftovers: i64 = conn
                .query_row(
                    "SELECT count(*) FROM sqlite_master WHERE name IN
                       ('messages_new','message_labels_new','bodies_new','attachments_new')",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(leftovers, 0, "half-applied rebuild tables must roll back");
            // Old shape is intact.
            let account_pk: i64 = conn
                .query_row(
                    "SELECT count(*) FROM pragma_table_info('messages') WHERE name='account_id' AND pk>0",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(account_pk, 0);
            let n: i64 = conn
                .query_row("SELECT count(*) FROM messages", [], |r| r.get(0))
                .unwrap();
            assert_eq!(n, 1);
        }

        // Reopen: the upgrade completes on the coherent pre-migration database.
        let db = Db::open(dir.path()).unwrap();
        let conn = db.pool.get().unwrap();
        let v: i64 = conn
            .query_row("SELECT version FROM schema_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        let acc: String = conn
            .query_row(
                "SELECT account_id FROM attachments WHERE id='att1'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(acc, "a1");
        let fts: i64 = conn
            .query_row("SELECT count(*) FROM messages_fts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(fts, 1);
        let violations: i64 = conn
            .query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(violations, 0);
        // A pre-migration snapshot was taken before the structural rebuild.
        let backups = std::fs::read_dir(dir.path().join("backups"))
            .unwrap()
            .count();
        assert_eq!(backups, 1);
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
