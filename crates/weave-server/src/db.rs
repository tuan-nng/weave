use rusqlite::Connection;
use std::path::Path;
use std::sync::Mutex;
use tracing::info;

use crate::error::AppError;

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
        assert_eq!(v1, 9, "user_version should be 9 after all migrations");
    }
}
