//! Startup-time recovery tasks.
//!
//! Currently exposes [`reap_orphans`], which marks every `connecting`
//! session that survived a previous server crash as `error`. The state
//! machine has only one status that genuinely could be a zombie from a
//! killed server: `connecting` is the transient state set at session
//! creation and only the spawned streaming task transitions it out
//! (to `ready` at the start of the first turn). If a session is in
//! `connecting` when the server starts, no other process exists to
//! finish the transition, so it is by definition a zombie.
//!
//! Crucially, `ready` sessions are NOT reaped. `ready` is the
//! multi-turn idle state — the session successfully completed its last
//! turn and is waiting for the next prompt. Reaping it would silently
//! break every multi-turn conversation on every server restart. The
//! ActiveSessions in-memory map (which doesn't survive a crash) is
//! the only way to know whether a `ready` session was mid-stream when
//! the server died, and it's gone — so we conservatively leave `ready`
//! alone and surface a half-streamed assistant message to the user
//! instead of nuking a successful multi-turn history.
//!
//! Used by `run()` in `main.rs` after the database is opened and migrations
//! have run, but BEFORE the listener is bound. This guarantees that any
//! client that races to connect immediately after a successful bind sees a
//! consistent world: no zombie `connecting` sessions, no half-streamed
//! events. `ready` sessions are preserved across restarts.

use tracing::info;

use crate::db::Db;
use crate::error::AppError;

/// Statuses that are reaped on startup. Keep this list narrow — see the
/// module docs for the multi-turn `ready` invariant.
const REAP_STATUSES: &[&str] = &["connecting"];

/// Reason text recorded into the tracing log for each reaped session.
///
/// The sessions table has no `error_message` column (and adding one is
/// out of scope for feat-034), so the reason is observability-only — the
/// session's `status` flips to `error` and a structured log line names
/// the session so the operator can correlate it with client reports.
const REAP_REASON: &str = "orphan: server restarted with active session";

/// Mark every `connecting` session as `error`.
///
/// Returns the number of sessions reaped. Idempotent — calling this on a
/// fresh database (no survivors) is a no-op and returns 0. Uses a single
/// transaction so the recovery either lands atomically or rolls back, and
/// the caller's view of "active sessions" never sees a partial transition.
///
/// `ready` sessions are deliberately left alone: see the module docs for
/// why this is the only correct behavior for a multi-turn session model.
///
/// Safe to call from the synchronous startup path: it holds the DB mutex
/// only for the duration of one transaction.
pub(crate) fn reap_orphans(db: &Db) -> Result<u64, AppError> {
    db.with_transaction(|conn| {
        // 1. Collect the survivor IDs in this transaction. We can't bind
        //    a subquery directly to the UPDATE in SQLite without a CTE,
        //    and the read+write split keeps the two statements readable.
        let placeholders = REAP_STATUSES
            .iter()
            .map(|_| "?")
            .collect::<Vec<_>>()
            .join(",");
        let select_sql = format!("SELECT id FROM sessions WHERE status IN ({placeholders})");
        let mut stmt = conn.prepare(&select_sql)?;
        let survivors: Vec<String> = stmt
            .query_map(
                rusqlite::params_from_iter(REAP_STATUSES.iter().copied()),
                |r| r.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;

        if survivors.is_empty() {
            return Ok(0);
        }

        // 2. Flip each survivor to `error`. The WHERE clause mirrors the
        //    `SessionStore::update_status` state-machine check, so a row
        //    that became terminal between (1) and (2) is left alone
        //    (rows_affected = 0). We can't call `update_status` here
        //    because it acquires `db.conn()` — re-entrant from inside a
        //    `with_transaction` closure that already holds the lock.
        let now = chrono::Utc::now().to_rfc3339();
        for id in &survivors {
            let rows = conn.execute(
                "UPDATE sessions SET status = 'error', updated_at = ?1
                 WHERE id = ?2 AND status IN ('connecting')",
                rusqlite::params![now, id],
            )?;
            if rows > 0 {
                info!(session_id = %id, "Reaped orphan session");
            }
        }

        info!(
            count = survivors.len(),
            reason = REAP_REASON,
            "Orphan reaper finished"
        );
        Ok(survivors.len() as u64)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_test_helpers::{
        make_test_db, seed_provider, seed_workspace_with_board,
    };
    use crate::store::sessions::{Session, SessionStore};

    /// Insert a session with the given status. Returns the session id.
    fn insert_session(db: &Db, workspace_id: &str, provider_id: &str, status: &str) -> String {
        let session = SessionStore::create(
            db,
            workspace_id,
            provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
        )
        .expect("create session");
        // status is initially 'connecting'; transition to the requested
        // terminal/non-terminal state.
        if status != "connecting" {
            SessionStore::update_status(db, &session.id, status)
                .expect("transition to requested status");
        }
        session.id
    }

    fn get_status(db: &Db, id: &str) -> String {
        Session::get_status_via_db(db, id)
    }

    #[test]
    fn test_reap_orphans_marks_only_connecting_sessions_as_error() {
        let db = make_test_db();
        let (workspace_id, _, _) = seed_workspace_with_board(&db);
        let provider_id = seed_provider(&db);

        // One genuine orphan (connecting — the only status the spawned
        // streaming task can be in when the server dies), one idle
        // multi-turn session (ready — must be preserved), one already
        // terminal session (completed — must be untouched), and one
        // already-error session (must also be untouched; it was never
        // going to be reaped again).
        let id_connecting = insert_session(&db, &workspace_id, &provider_id, "connecting");
        let id_ready = insert_session(&db, &workspace_id, &provider_id, "ready");
        let _id_error = insert_session(&db, &workspace_id, &provider_id, "error");
        let id_completed = insert_session(&db, &workspace_id, &provider_id, "completed");

        let reaped = reap_orphans(&db).expect("reap_orphans");
        assert_eq!(
            reaped, 1,
            "only the `connecting` session should be reaped; \
             `ready` is the multi-turn idle state and must survive restart"
        );

        assert_eq!(get_status(&db, &id_connecting), "error");
        // Regression guard: previously this asserted `error`, which broke
        // every multi-turn session across server restarts.
        assert_eq!(
            get_status(&db, &id_ready),
            "ready",
            "`ready` sessions must survive `reap_orphans` (multi-turn invariant)"
        );
        assert_eq!(get_status(&db, &id_completed), "completed");
    }

    #[test]
    fn test_reap_orphans_empty_database_is_noop() {
        let db = make_test_db();
        let _ = seed_workspace_with_board(&db);
        let reaped = reap_orphans(&db).expect("reap_orphans");
        assert_eq!(reaped, 0);
    }

    #[test]
    fn test_reap_orphans_idempotent() {
        let db = make_test_db();
        let (workspace_id, _, _) = seed_workspace_with_board(&db);
        let provider_id = seed_provider(&db);
        // Use `connecting` (not `ready` — see multi-turn invariant).
        insert_session(&db, &workspace_id, &provider_id, "connecting");

        // First call reaps 1, second call sees only terminal sessions
        // and reaps 0.
        assert_eq!(reap_orphans(&db).expect("first reap"), 1);
        assert_eq!(reap_orphans(&db).expect("second reap"), 0);
    }

    /// Regression test for the multi-turn invariant: a `ready` session
    /// (idle, waiting for next prompt) must survive every startup of
    /// `reap_orphans`. Previously `reap_orphans` treated `ready` as an
    /// orphan state, silently breaking every multi-turn conversation on
    /// every server restart and forcing users to start a new session.
    #[test]
    fn test_reap_orphans_preserves_ready_sessions_across_restarts() {
        let db = make_test_db();
        let (workspace_id, _, _) = seed_workspace_with_board(&db);
        let provider_id = seed_provider(&db);
        let id_ready = insert_session(&db, &workspace_id, &provider_id, "ready");

        // Simulate 5 server restarts.
        for _ in 0..5 {
            assert_eq!(reap_orphans(&db).expect("reap"), 0);
            assert_eq!(
                get_status(&db, &id_ready),
                "ready",
                "`ready` session must survive repeated `reap_orphans` calls"
            );
        }
    }

    impl Session {
        /// Test-only helper: read the current `status` of a session row
        /// by primary key. The public `get_by_id` returns a full struct;
        /// this avoids constructing one when we only want a single field.
        fn get_status_via_db(db: &Db, id: &str) -> String {
            db.conn()
                .query_row("SELECT status FROM sessions WHERE id = ?1", [id], |r| {
                    r.get(0)
                })
                .expect("session exists")
        }
    }
}
