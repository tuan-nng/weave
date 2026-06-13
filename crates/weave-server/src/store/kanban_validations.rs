//! `kanban_validations` cache — avoid re-running validator subprocesses
//! on every move attempt.
//!
//! Keyed on `(task_id, command_key)` where `command_key` is the literal
//! shell command string. Same command on the same task returns the
//! cached result; a different command (operator changed it) misses.
//!
//! Best-effort: cache writes that fail are silently ignored (the next
//! move will re-run the command). The cache table is created by
//! migration 016.

use crate::db::Db;
use uuid::Uuid;

/// Look up a cached validator result.
/// Returns `Some(true)` for a cached pass, `Some(false)` for a cached
/// fail, `None` for a cache miss.
pub fn check_cache(db: &Db, task_id: &str, command_key: &str) -> Option<bool> {
    db.conn()
        .query_row(
            "SELECT result FROM kanban_validations WHERE task_id = ?1 AND command_key = ?2",
            rusqlite::params![task_id, command_key],
            |row| {
                let v: i64 = row.get(0)?;
                Ok(v != 0)
            },
        )
        .ok()
}

/// Store a validator result. Best-effort: failures are logged and
/// silently dropped (the next move will re-run the command).
pub fn cache_result(db: &Db, task_id: &str, command_key: &str, passed: bool) {
    let result = db.conn().execute(
        "INSERT OR REPLACE INTO kanban_validations
            (id, task_id, command_key, result, cached_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![
            Uuid::new_v4().to_string(),
            task_id,
            command_key,
            passed as i64,
            chrono::Utc::now().to_rfc3339(),
        ],
    );
    if let Err(e) = result {
        tracing::warn!(
            error = %e,
            task_id,
            "failed to cache validator result; will re-run next time"
        );
    }
}
