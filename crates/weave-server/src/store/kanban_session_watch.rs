//! `kanban_session_watch` table — supervisor state for kanban-auto-spawned
//! sessions (feat-067).
//!
//! One row per kanban-auto-spawned session, with `last_activity_at` bumped on
//! every SSE event for the session and on every `send_prompt` call. The
//! `KanbanLifecycleSupervisor` reads this table to detect stalled sessions
//! and either re-prompts them or fails them after `recovery_count` hits
//! `max_recovery_retries`.
//!
//! The `last_activity_at` column is RFC3339 text (not a numeric epoch) so
//! the rows are human-readable in `sqlite3` and consistent with the
//! timestamps used elsewhere in the schema (sessions.updated_at,
//! kanban_validations.cached_at, etc.).

use crate::db::Db;
use chrono::Utc;

/// Possible `status` values. The supervisor moves rows through
/// `watching` → `stalled` → `recovering` → either `watching` (recovery
/// succeeded) or `failed` (recovery exhausted).
pub const STATUS_WATCHING: &str = "watching";
pub const STATUS_STALLED: &str = "stalled";
pub const STATUS_RECOVERING: &str = "recovering";
pub const STATUS_FAILED: &str = "failed";

/// Maximum recovery attempts before a session is failed. Spec default.
pub const DEFAULT_MAX_RECOVERY_RETRIES: u32 = 2;

/// Default stall threshold. A session with no activity for this many
/// seconds is considered stalled.
pub const DEFAULT_STALL_THRESHOLD_SECONDS: u64 = 300;

/// One watch row, as the supervisor reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KanbanSessionWatch {
    pub session_id: String,
    pub task_id: String,
    pub last_activity_at: String,
    pub recovery_count: u32,
    pub status: String,
}

/// Insert a new watch row. Called by `try_automate_lane` after the
/// session is created. `INSERT OR IGNORE` makes the call safe to repeat
/// (e.g. on a partial retry), and the subsequent `bump_activity` call
/// will catch the row up.
pub fn create_watch(db: &Db, session_id: &str, task_id: &str) -> Result<(), rusqlite::Error> {
    db.conn().execute(
        "INSERT OR IGNORE INTO kanban_session_watch
            (session_id, task_id, last_activity_at, recovery_count, status)
         VALUES (?1, ?2, ?3, 0, ?4)",
        rusqlite::params![
            session_id,
            task_id,
            Utc::now().to_rfc3339(),
            STATUS_WATCHING
        ],
    )?;
    Ok(())
}

/// Bump `last_activity_at` for a session. Called by the `SseManager::broadcast`
/// hook and by `SessionService::send_prompt`. Silently ignores unknown
/// sessions (the session may not be a kanban-auto-spawned one — no row,
/// no bump, no log spam).
///
/// The bump is a no-op when the row is NOT in `status = 'watching'`:
/// `recovering` rows are mid-flight and the supervisor owns the next
/// state transition; `stalled` / `failed` rows have already left the
/// active set and must not be silently re-armed by a stray broadcast.
/// (Without this guard, broadcasting `SessionFailed` from the
/// supervisor would un-fail the row on the spot — the same broadcast
/// that just marked it failed.)
pub fn bump_activity(db: &Db, session_id: &str) {
    let res = db.conn().execute(
        "UPDATE kanban_session_watch
         SET last_activity_at = ?1
         WHERE session_id = ?2 AND status = ?3",
        rusqlite::params![Utc::now().to_rfc3339(), session_id, STATUS_WATCHING],
    );
    if let Err(e) = res {
        tracing::warn!(
            error = %e,
            session_id,
            "kanban_session_watch: failed to bump activity"
        );
    }
}

/// Find sessions that have been inactive for at least `stall_threshold_seconds`
/// and are still being watched (status = 'watching'). Returns the
/// `(session_id, task_id, recovery_count, last_activity_at)` rows so the
/// supervisor can act on them. Pure function over the current DB state.
pub fn list_stalled(
    db: &Db,
    stall_threshold_seconds: u64,
) -> Result<Vec<KanbanSessionWatch>, rusqlite::Error> {
    // Compute the cutoff in SQL with a single datetime expression. SQLite
    // stores the column as RFC3339 text; `datetime(last_activity_at, ?)`
    // accepts a `-N seconds` modifier and yields a comparable RFC3339
    // string. This avoids loading every row and filtering in memory.
    let cutoff_expr = format!("-{stall_threshold_seconds} seconds");
    let conn = db.conn();
    let mut stmt = conn.prepare(
        "SELECT session_id, task_id, last_activity_at, recovery_count, status
         FROM kanban_session_watch
         WHERE status = ?1
           AND datetime(last_activity_at) <= datetime('now', ?2)
         ORDER BY last_activity_at ASC",
    )?;
    let rows = stmt
        .query_map(rusqlite::params![STATUS_WATCHING, cutoff_expr], |r| {
            Ok(KanbanSessionWatch {
                session_id: r.get(0)?,
                task_id: r.get(1)?,
                last_activity_at: r.get(2)?,
                recovery_count: r.get::<_, i64>(3)? as u32,
                status: r.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// Mark a watch row as 'stalled' (called when the supervisor first
/// detects it). Best-effort: errors are logged and swallowed.
pub fn mark_stalled(db: &Db, session_id: &str) {
    if let Err(e) = db.conn().execute(
        "UPDATE kanban_session_watch SET status = ?1 WHERE session_id = ?2",
        rusqlite::params![STATUS_STALLED, session_id],
    ) {
        tracing::warn!(error = %e, session_id, "kanban_session_watch: mark_stalled failed");
    }
}

/// Increment `recovery_count` and flip the row to 'recovering'.
/// Returns the new recovery count.
pub fn begin_recovery(db: &Db, session_id: &str) -> Result<u32, rusqlite::Error> {
    db.conn().execute(
        "UPDATE kanban_session_watch
         SET recovery_count = recovery_count + 1,
             status = ?1,
             last_activity_at = ?2
         WHERE session_id = ?3",
        rusqlite::params![STATUS_RECOVERING, Utc::now().to_rfc3339(), session_id],
    )?;
    let new_count: i64 = db.conn().query_row(
        "SELECT recovery_count FROM kanban_session_watch WHERE session_id = ?1",
        [session_id],
        |r| r.get(0),
    )?;
    Ok(new_count as u32)
}

/// Move a row back to 'watching' (recovery was sent; the activity
/// bump will keep it fresh).
pub fn mark_recovered(db: &Db, session_id: &str) {
    if let Err(e) = db.conn().execute(
        "UPDATE kanban_session_watch SET status = ?1 WHERE session_id = ?2",
        rusqlite::params![STATUS_WATCHING, session_id],
    ) {
        tracing::warn!(error = %e, session_id, "kanban_session_watch: mark_recovered failed");
    }
}

/// Mark a row as 'failed'. The session status transition to 'error'
/// is the caller's responsibility (the supervisor calls
/// `SessionStore::update_status` separately).
pub fn mark_failed(db: &Db, session_id: &str) {
    if let Err(e) = db.conn().execute(
        "UPDATE kanban_session_watch SET status = ?1 WHERE session_id = ?2",
        rusqlite::params![STATUS_FAILED, session_id],
    ) {
        tracing::warn!(error = %e, session_id, "kanban_session_watch: mark_failed failed");
    }
}

/// Remove a watch row. Used when a session is closed / completed
/// outside the supervisor's control (manual cancel, server restart, etc.).
pub fn delete_watch(db: &Db, session_id: &str) {
    if let Err(e) = db.conn().execute(
        "DELETE FROM kanban_session_watch WHERE session_id = ?1",
        [session_id],
    ) {
        tracing::warn!(error = %e, session_id, "kanban_session_watch: delete failed");
    }
}

/// Look up a single watch row (for tests / observability).
pub fn get(db: &Db, session_id: &str) -> Result<Option<KanbanSessionWatch>, rusqlite::Error> {
    let conn = db.conn();
    let mut stmt = conn.prepare(
        "SELECT session_id, task_id, last_activity_at, recovery_count, status
         FROM kanban_session_watch WHERE session_id = ?1",
    )?;
    let mut rows = stmt.query([session_id])?;
    if let Some(r) = rows.next()? {
        Ok(Some(KanbanSessionWatch {
            session_id: r.get(0)?,
            task_id: r.get(1)?,
            last_activity_at: r.get(2)?,
            recovery_count: r.get::<_, i64>(3)? as u32,
            status: r.get(4)?,
        }))
    } else {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_test_helpers::make_test_db;

    /// Insert a minimal session row with a deterministic id. Bypasses
    /// `SessionStore::create` (which assigns its own UUID) so the test
    /// can pick the FK target for the watch row. The provider row uses
    /// the post-feat-039 shape (`type` + `kind` + `default_model`); the
    /// extra columns are left at their DEFAULT.
    fn seed_session(db: &Db, sid: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES ('ws-test', 'test', 'active', ?1, ?1)",
                rusqlite::params![now],
            )
            .expect("seed workspace");
        db.conn()
            .execute(
                "INSERT INTO providers (id, type, kind, name, config_json, default_model, created_at)
                 VALUES ('provider-test', 'anthropic', 'http', 'p',
                         '{\"base_url\":\"x\",\"api_key\":\"y\"}',
                         'm', ?1)",
                rusqlite::params![now],
            )
            .expect("seed provider");
        db.conn()
            .execute(
                "INSERT INTO sessions
                     (id, workspace_id, provider_id, status, created_at, updated_at)
                 VALUES (?1, 'ws-test', 'provider-test', 'ready', ?2, ?2)",
                rusqlite::params![sid, now],
            )
            .expect("seed session");
    }

    #[test]
    fn test_create_watch_inserts_row() {
        let db = make_test_db();
        seed_session(&db, "s-1");
        create_watch(&db, "s-1", "t-1").expect("create_watch");
        let row = get(&db, "s-1").expect("get").expect("row exists");
        assert_eq!(row.session_id, "s-1");
        assert_eq!(row.task_id, "t-1");
        assert_eq!(row.recovery_count, 0);
        assert_eq!(row.status, STATUS_WATCHING);
    }

    #[test]
    fn test_create_watch_is_idempotent() {
        let db = make_test_db();
        seed_session(&db, "s-1");
        create_watch(&db, "s-1", "t-1").unwrap();
        create_watch(&db, "s-1", "t-1").unwrap();
        // Still one row, recovery_count still zero.
        let row = get(&db, "s-1").expect("get").expect("row exists");
        assert_eq!(row.recovery_count, 0);
    }

    #[test]
    fn test_bump_activity_updates_timestamp() {
        let db = make_test_db();
        seed_session(&db, "s-1");
        create_watch(&db, "s-1", "t-1").unwrap();
        let before = get(&db, "s-1").unwrap().unwrap().last_activity_at;
        std::thread::sleep(std::time::Duration::from_millis(10));
        bump_activity(&db, "s-1");
        let after = get(&db, "s-1").unwrap().unwrap().last_activity_at;
        assert!(
            after >= before,
            "after should be >= before, got {after} vs {before}"
        );
    }

    #[test]
    fn test_bump_activity_unknown_session_is_silent() {
        let db = make_test_db();
        // No row for "s-ghost" — bump should be a silent no-op.
        bump_activity(&db, "s-ghost");
        assert!(get(&db, "s-ghost").unwrap().is_none());
    }

    #[test]
    fn test_bump_activity_skips_non_watching_rows() {
        // Regression guard for the supervisor race: a row in
        // 'recovering' / 'stalled' / 'failed' must NOT be un-armed by
        // a stray broadcast (the `SessionFailed` broadcast from the
        // supervisor itself would otherwise flip the row back to
        // 'watching' and silently re-arm the session).
        let db = make_test_db();
        seed_session(&db, "s-1");
        create_watch(&db, "s-1", "t-1").unwrap();
        begin_recovery(&db, "s-1").unwrap();
        let before = get(&db, "s-1").unwrap().unwrap();
        assert_eq!(before.status, STATUS_RECOVERING);
        let before_ts = before.last_activity_at;

        bump_activity(&db, "s-1");
        let after = get(&db, "s-1").unwrap().unwrap();
        assert_eq!(
            after.status, STATUS_RECOVERING,
            "bump must not flip a recovering row to watching"
        );
        assert_eq!(
            after.last_activity_at, before_ts,
            "bump must not advance a non-watching row's timestamp"
        );
    }

    #[test]
    fn test_list_stalled_finds_old_rows() {
        let db = make_test_db();
        seed_session(&db, "s-old");
        create_watch(&db, "s-old", "t-1").unwrap();
        // Backdate the row to 10 minutes ago.
        let ten_min_ago = (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339();
        db.conn()
            .execute(
                "UPDATE kanban_session_watch SET last_activity_at = ?1",
                [ten_min_ago],
            )
            .unwrap();
        // 300s threshold: 10-minute-old row is stalled.
        let stalled = list_stalled(&db, 300).unwrap();
        assert_eq!(stalled.len(), 1);
        assert_eq!(stalled[0].session_id, "s-old");
    }

    #[test]
    fn test_list_stalled_excludes_fresh_rows() {
        let db = make_test_db();
        seed_session(&db, "s-fresh");
        create_watch(&db, "s-fresh", "t-1").unwrap();
        // No backdate — the row is brand new, NOT stalled.
        let stalled = list_stalled(&db, 300).unwrap();
        assert!(stalled.is_empty(), "fresh rows must not be stalled");
    }

    #[test]
    fn test_list_stalled_excludes_non_watching_status() {
        let db = make_test_db();
        seed_session(&db, "s-stalled");
        create_watch(&db, "s-stalled", "t-1").unwrap();
        let ten_min_ago = (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339();
        db.conn()
            .execute(
                "UPDATE kanban_session_watch SET last_activity_at = ?1",
                [ten_min_ago],
            )
            .unwrap();
        // Flip status to 'failed' — even though the timestamp is old,
        // the supervisor only cares about 'watching' rows.
        mark_failed(&db, "s-stalled");
        let stalled = list_stalled(&db, 300).unwrap();
        assert!(stalled.is_empty(), "non-watching rows must be excluded");
    }

    #[test]
    fn test_begin_recovery_increments_count() {
        let db = make_test_db();
        seed_session(&db, "s-1");
        create_watch(&db, "s-1", "t-1").unwrap();
        let c1 = begin_recovery(&db, "s-1").unwrap();
        let c2 = begin_recovery(&db, "s-1").unwrap();
        assert_eq!(c1, 1);
        assert_eq!(c2, 2);
        let row = get(&db, "s-1").unwrap().unwrap();
        assert_eq!(row.status, STATUS_RECOVERING);
    }

    #[test]
    fn test_mark_recovered_resets_to_watching() {
        let db = make_test_db();
        seed_session(&db, "s-1");
        create_watch(&db, "s-1", "t-1").unwrap();
        begin_recovery(&db, "s-1").unwrap();
        mark_recovered(&db, "s-1");
        let row = get(&db, "s-1").unwrap().unwrap();
        assert_eq!(row.status, STATUS_WATCHING);
    }

    #[test]
    fn test_delete_watch_removes_row() {
        let db = make_test_db();
        seed_session(&db, "s-1");
        create_watch(&db, "s-1", "t-1").unwrap();
        delete_watch(&db, "s-1");
        assert!(get(&db, "s-1").unwrap().is_none());
    }
}
