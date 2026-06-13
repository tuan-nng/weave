use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tracing::info;

use crate::error::AppError;

/// Path literal for in-memory databases. SQLite treats `:memory:` as a
/// reserved name; we compare against it in `size_bytes` to return
/// `None` (no on-disk file).
const IN_MEMORY_PATH: &str = ":memory:";

/// Result of a synchronous `PRAGMA wal_checkpoint(TRUNCATE)` call.
///
/// `busy` is `true` when another connection held the writer lock at the
/// moment of the checkpoint — the result still reports the page counts but
/// no work was done. `log_pages` and `checkpointed_pages` are the number
/// of WAL frames observed and merged; both are zero for in-memory
/// databases. Used by the graceful-shutdown sequence (feat-034).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalCheckpointResult {
    pub busy: bool,
    pub log_pages: u64,
    pub checkpointed_pages: u64,
}

/// Remap an `INSERT`-time `rusqlite::Error` to the right `AppError`
/// variant for the `*_store::create` path. The expected
/// `INSERT`-time errors are:
///
///   - `SQLITE_CONSTRAINT_UNIQUE` (extended code 2067) — a UNIQUE
///     index on the table tripped. Surfaced as `Conflict` with the
///     caller-supplied `conflict_message` so the API caller sees a
///     stable shape ("a X with the same Y already exists").
///   - `SQLITE_CONSTRAINT_FOREIGNKEY` (extended code 787) — the
///     parent row was deleted between the API check and the INSERT
///     (the narrow TOCTOU window the precheck cannot fully close).
///     Surfaced as `NotFound` with the supplied `fk_resource` so
///     the caller gets the same shape as the precheck failure.
///
/// Anything else (NOT NULL, CHECK, internal) propagates as
/// `Database` via `AppError::from`.
///
/// The third caller (the notes store, feat-030) triggered the
/// hoist from per-store private helpers. The previous
/// implementations live in `store::artifacts` and `store::codebases`
/// and remain identical in behavior.
pub fn map_insert_error(e: rusqlite::Error, conflict_message: &str, fk_resource: &str) -> AppError {
    if let rusqlite::Error::SqliteFailure(err, _msg) = &e {
        if err.code == rusqlite::ErrorCode::ConstraintViolation {
            // SQLITE_CONSTRAINT_UNIQUE = 2067
            if err.extended_code == 2067 {
                return AppError::Conflict(conflict_message.to_string());
            }
            // SQLITE_CONSTRAINT_FOREIGNKEY = 787
            if err.extended_code == 787 {
                return AppError::NotFound {
                    resource: fk_resource.to_string(),
                    id: "(deleted between verify and insert)".to_string(),
                };
            }
        }
    }
    AppError::from(e)
}

/// Database connection wrapper with interior mutability.
///
/// Encapsulates a single SQLite connection behind a mutex.
/// WAL mode allows concurrent reads; writes acquire the lock.
pub struct Db {
    conn: Mutex<Connection>,
    /// Path the database was opened from (including `":memory:"`).
    /// Used by `size_bytes` to short-circuit in-memory databases.
    path: PathBuf,
}

/// Ordered list of (version, sql) migrations embedded at compile time.
const MIGRATIONS: &[(&str, &str)] = &[
    ("001", include_str!("migrations/001_init.sql")),
    ("002", include_str!("migrations/002_kanban.sql")),
    (
        "003",
        include_str!("migrations/003_workspace_unique_name.sql"),
    ),
    ("004", include_str!("migrations/004_task_context.sql")),
    (
        "005",
        include_str!("migrations/005_task_column_cascade.sql"),
    ),
    (
        "006",
        include_str!("migrations/006_column_transition_gates.sql"),
    ),
    ("007", include_str!("migrations/007_artifacts.sql")),
    ("008", include_str!("migrations/008_notes.sql")),
    ("009", include_str!("migrations/009_a2a_context_id.sql")),
    (
        "010",
        include_str!("migrations/010_session_codebase_id.sql"),
    ),
    ("011", include_str!("migrations/011_session_runtime.sql")),
    (
        "012",
        include_str!("migrations/012_provider_runtime_kind.sql"),
    ),
    (
        "013",
        include_str!("migrations/013_column_runtime_kind.sql"),
    ),
    (
        "014",
        include_str!("migrations/014_task_extended_fields.sql"),
    ),
    ("015", include_str!("migrations/015_column_stage.sql")),
];

impl Db {
    /// Open a SQLite database at the given path and run pending migrations.
    ///
    /// Use `":memory:"` for in-memory databases in tests.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;

        // Enable WAL mode for concurrent read access
        conn.pragma_update(None, "journal_mode", "WAL")?;

        // Enforce foreign key constraints
        conn.pragma_update(None, "foreign_keys", "ON")?;

        // Wait up to 5s when the database is locked
        conn.pragma_update(None, "busy_timeout", 5000)?;

        let db = Self {
            conn: Mutex::new(conn),
            path: path.to_path_buf(),
        };
        db.run_migrations()?;

        info!(path = %path.display(), "Database opened");
        Ok(db)
    }

    /// Get a guard to the underlying connection.
    ///
    /// Holds the mutex for the lifetime of the returned guard.
    /// Use this for all queries: `db.conn().execute(...)`.
    pub fn conn(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.conn.lock().expect("database lock poisoned")
    }

    /// Execute a closure within a database transaction.
    ///
    /// Acquires the connection lock, begins a transaction, and passes
    /// `&Connection` to the closure. Commits on `Ok`, auto-rollbacks on `Err`.
    pub fn with_transaction<T, F>(&self, f: F) -> Result<T, AppError>
    where
        F: FnOnce(&Connection) -> Result<T, AppError>,
    {
        let mut conn = self.conn.lock().expect("database lock poisoned");
        let tx = conn.transaction()?;
        match f(&tx) {
            Ok(val) => {
                tx.commit()?;
                Ok(val)
            }
            Err(e) => Err(e),
            // tx drops here on Err, auto-rollback via RAII
        }
    }

    /// File size of the underlying SQLite database in bytes.
    ///
    /// Returns `None` for in-memory databases (`:memory:`) — the path
    /// has no on-disk file. Returns `None` if the file metadata cannot
    /// be read (e.g. the file was deleted between open and probe).
    pub fn size_bytes(&self) -> Option<u64> {
        if self.path == Path::new(IN_MEMORY_PATH) {
            return None;
        }
        std::fs::metadata(&self.path).ok().map(|m| m.len())
    }

    /// Whether the WAL has frames that have not yet been merged into
    /// the main database file.
    ///
    /// Runs `PRAGMA wal_checkpoint(PASSIVE)` and inspects the result
    /// tuple `(busy, log, checkpointed)`. `PASSIVE` does not block
    /// writers and never returns an error in normal operation; any
    /// unexpected error is reported as `false` (best-effort).
    pub fn wal_checkpoint_pending(&self) -> bool {
        let conn = self.conn.lock().expect("database lock poisoned");
        let result: rusqlite::Result<(i64, i64, i64)> =
            conn.query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            });
        matches!(result, Ok((_, log, ckpt)) if log > ckpt)
    }

    /// Synchronous WAL checkpoint with `TRUNCATE` mode.
    ///
    /// Returns a structured [`WalCheckpointResult`]. `TRUNCATE` blocks
    /// writers for the duration of the checkpoint and truncates the WAL
    /// file to zero length on success — the strongest mode, intended for
    /// the graceful-shutdown sequence (feat-034) rather than the hot path.
    ///
    /// - `result.busy == true` means another connection held the lock; the
    ///   page counts are still reported but no work was done.
    /// - For in-memory databases (`:memory:`) the WAL is a no-op and the
    ///   result is `(false, 0, 0)`.
    /// - Returns `AppError::Database` if the pragma itself fails.
    pub fn checkpoint(&self) -> Result<WalCheckpointResult, AppError> {
        let conn = self.conn.lock().expect("database lock poisoned");
        let (busy, log_pages, checkpointed_pages): (i64, i64, i64) =
            conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
        Ok(WalCheckpointResult {
            busy: busy != 0,
            log_pages: log_pages.max(0) as u64,
            checkpointed_pages: checkpointed_pages.max(0) as u64,
        })
    }

    /// Run all pending migrations in order.
    ///
    /// Uses `user_version` pragma to track which migrations have been applied.
    /// Safe to call multiple times — already-applied migrations are skipped.
    fn run_migrations(&self) -> anyhow::Result<()> {
        let conn = self.conn();
        let current: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;

        for (version, sql) in MIGRATIONS {
            let v: i32 = version.parse()?;
            if v > current {
                conn.execute_batch(sql)?;
                conn.pragma_update(None, "user_version", v)?;
                info!(version = v, "Migration applied");
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Verify database opens with correct pragmas and all 13 tables exist.
    ///
    /// Uses a temp file because `:memory:` databases don't support WAL mode.
    #[test]
    fn test_db_init() {
        let path = std::env::temp_dir().join("weave-test-db-init.db");
        // Clean up before test in case of leftover from previous run
        let _ = std::fs::remove_file(&path);

        let db = Db::open(&path).expect("failed to open db");

        // Verify WAL mode
        let journal: String = db
            .conn()
            .pragma_query_value(None, "journal_mode", |r| r.get(0))
            .expect("failed to query journal_mode");
        assert_eq!(journal, "wal", "journal_mode should be WAL");

        // Verify foreign keys are ON
        let fk: i32 = db
            .conn()
            .pragma_query_value(None, "foreign_keys", |r| r.get(0))
            .expect("failed to query foreign_keys");
        assert_eq!(fk, 1, "foreign_keys should be ON");

        // Verify busy timeout is 5000ms
        let timeout: i32 = db
            .conn()
            .pragma_query_value(None, "busy_timeout", |r| r.get(0))
            .expect("failed to query busy_timeout");
        assert_eq!(timeout, 5000, "busy_timeout should be 5000");

        // Verify all 13 tables exist
        let expected_tables = [
            "workspaces",
            "sessions",
            "messages",
            "providers",
            "codebases",
            "worktrees",
            "boards",
            "columns",
            "tasks",
            "traces",
            "file_changes",
            "artifacts",
            "notes",
        ];

        {
            let conn = db.conn();
            for table in &expected_tables {
                let count: i32 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                        [table],
                        |r| r.get(0),
                    )
                    .unwrap_or(0);
                assert_eq!(count, 1, "table '{table}' should exist");
            }
        }

        // Clean up
        drop(db);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    /// Verify running migrations twice is idempotent.
    #[test]
    fn test_migrations_idempotent() {
        let db = Db::open(Path::new(":memory:")).expect("failed to open db");

        // Record version after first run
        let v1: i32 = db
            .conn()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .expect("failed to query user_version");

        // Run migrations again
        db.run_migrations().expect("second migration run failed");

        let v2: i32 = db
            .conn()
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .expect("failed to query user_version");

        assert_eq!(v1, v2, "user_version should not change on second run");
        assert_eq!(v1, 15, "user_version should be 15 after all migrations");
    }

    /// `size_bytes` returns `None` for `:memory:` databases — there is
    /// no on-disk file to measure.
    #[test]
    fn test_db_size_bytes_returns_none_for_memory() {
        let db = Db::open(Path::new(":memory:")).expect("open :memory:");
        assert_eq!(db.size_bytes(), None);
    }

    /// `size_bytes` returns `Some(n)` for a file-backed DB after
    /// migrations have created tables. The exact size depends on
    /// SQLite's page size and the migration footprint; we just check
    /// it's non-zero.
    #[test]
    fn test_db_size_bytes_returns_some_for_file() {
        let path = std::env::temp_dir().join("weave-test-db-size-bytes.db");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));

        let db = Db::open(&path).expect("open file");
        let size = db.size_bytes();
        assert!(
            size.is_some(),
            "size_bytes should be Some for a file-backed db"
        );
        assert!(
            size.unwrap() > 0,
            "size_bytes should be > 0 after migrations"
        );

        drop(db);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }

    /// `wal_checkpoint_pending` is best-effort and returns `bool`.
    /// PASSIVE may auto-merge frames, so the value can be either true
    /// or false after writes; we only assert the call succeeds.
    #[test]
    fn test_db_wal_checkpoint_pending_returns_bool() {
        let db = Db::open(Path::new(":memory:")).expect("open :memory:");
        // In-memory DBs have no WAL; the pragma returns (0,0,0).
        assert!(!db.wal_checkpoint_pending());
    }

    /// `Db::checkpoint` runs `PRAGMA wal_checkpoint(TRUNCATE)` and returns
    /// a structured result. On an in-memory database the WAL is a no-op
    /// and the result is `(false, 0, 0)`. On a file-backed database with
    /// pending writes, `checkpointed_pages` may be > 0.
    #[test]
    fn test_db_checkpoint_runs_and_returns_structured_result() {
        // :memory: case — WAL is a no-op, all counts are zero.
        let mem = Db::open(Path::new(":memory:")).expect("open :memory:");
        let r = mem.checkpoint().expect("checkpoint on :memory:");
        assert!(!r.busy, "no other connection should hold the lock");
        assert_eq!(r.log_pages, 0, ":memory: has no WAL");
        assert_eq!(r.checkpointed_pages, 0, ":memory: checkpoints nothing");

        // File-backed case — write something so the WAL has a frame, then
        // checkpoint. We don't assert exact counts (SQLite may have already
        // merged the frame during the INSERT) — just that the call returns
        // a structured result without error and is not busy.
        let path = std::env::temp_dir().join("weave-test-db-checkpoint.db");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));

        let db = Db::open(&path).expect("open file");
        let r = db.checkpoint().expect("checkpoint on file");
        assert!(!r.busy, "we hold the only connection — busy must be false");

        drop(db);
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(format!("{}-wal", path.display()));
        let _ = std::fs::remove_file(format!("{}-shm", path.display()));
    }
}
