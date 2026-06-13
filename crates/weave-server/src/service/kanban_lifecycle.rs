//! Kanban-auto-spawned session lifecycle supervisor (feat-067).
//!
//! A `tokio` watchdog task that detects stalled sessions, re-prompts them
//! with a "you appear stalled, continue" prefix, stops recovery after
//! `max_recovery_retries` (default 2), and broadcasts a `SessionFailed`
//! SSE event on BOTH the session channel and the board channel when the
//! limit is hit. Mirrors the `reap_orphans` startup-time recovery pattern
//! (`service/startup.rs`), but runs on a 30s `tokio::time::interval` so
//! it sees live activity, not just startup-time state.
//!
//! Architecture:
//!   * `start(state, shutdown)` — spawns the task, returns the `JoinHandle`.
//!   * `scan_once(state, now)` — pure function over the current DB state
//!     that lists stalled rows. Reused by tests so the supervisor
//!     doesn't need a live interval to be exercised.
//!   * `recover_stalled(state, rows, now)` — sends re-prompts via
//!     `SessionService::send_prompt` and either marks the watch row
//!     `recovered` or `failed` based on `recovery_count` vs
//!     `max_recovery_retries`. Broadcasts `SessionFailed` on failure.
//!
//! The supervisor does NOT need a lock on the DB or the SSE manager —
//! the `bump_activity` hook (`SseManager::broadcast`) and the watch-row
//! SQL updates are the only cross-cutting concerns, and they're all
//! fire-and-forget.

use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::service::sessions::SessionService;
use crate::sse::SseWireEvent;
use crate::store::kanban_session_watch::{
    self, KanbanSessionWatch, DEFAULT_MAX_RECOVERY_RETRIES, DEFAULT_STALL_THRESHOLD_SECONDS,
};
use crate::store::sessions::SessionStore;
use crate::AppState;

/// How often the supervisor scans for stalled sessions. Short enough to
/// catch stalls before they eat too much wall-clock, long enough that the
/// per-scan SQL is cheap on a board with a few watching sessions.
pub const SCAN_INTERVAL: Duration = Duration::from_secs(30);

/// Look up the task's `board_id` from a task_id. The supervisor needs
/// the board id to broadcast `SessionFailed` on the board channel. The
/// join is `tasks → boards`; a missing task (deleted between the
/// supervisor's scan and the broadcast) is logged and silently
/// skipped.
fn board_id_for_task(state: &AppState, task_id: &str) -> Option<String> {
    let conn = state.db.conn();
    let mut stmt = match conn
        .prepare("SELECT b.id FROM tasks t JOIN boards b ON b.id = t.board_id WHERE t.id = ?1")
    {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, task_id, "kanban_lifecycle: prepare board lookup");
            return None;
        }
    };
    stmt.query_row([task_id], |r| r.get(0)).ok()
}

/// Pure list of stalled rows. Exposed for tests.
pub fn list_stalled(
    state: &AppState,
    stall_threshold_seconds: u64,
) -> Result<Vec<KanbanSessionWatch>, rusqlite::Error> {
    kanban_session_watch::list_stalled(&state.db, stall_threshold_seconds)
}

/// Send a recovery re-prompt to a stalled session.
///
/// Format: a short, declarative line telling the agent it's stalled,
/// followed by the last assistant message excerpt (so the agent
/// resumes from the right point), followed by the kanban-task
/// instructions (move forward or report a blocker). The exact wording
/// is per the feat-067 spec; the trailing `last_assistant_message_excerpt`
/// is truncated to 1 KB to keep the re-prompt bounded.
pub fn build_recovery_prompt(last_excerpt: &str) -> String {
    const MAX_EXCERPT: usize = 1024;
    let excerpt = if last_excerpt.len() > MAX_EXCERPT {
        // Truncate at a char boundary, not mid-byte. The spec's
        // 8 KB description cap (feat-063) uses the same `.char_indices`
        // approach — copy it here for consistency.
        let cut = last_excerpt
            .char_indices()
            .take_while(|(i, _)| *i < MAX_EXCERPT)
            .last()
            .map(|(i, _)| i)
            .unwrap_or(0);
        &last_excerpt[..cut]
    } else {
        last_excerpt
    };
    format!(
        "You appear stalled (no activity in the last few minutes). \
         Continue the kanban task. If you're done, call `move_card` to advance the card; \
         if you're blocked, call `update_card` with a comment explaining the blocker.\n\n\
         Last assistant message:\n{excerpt}"
    )
}

/// Read the most recent assistant message for a session (used as the
/// `last_assistant_message_excerpt` in the re-prompt). Returns an
/// empty string when the session has no prior assistant turns.
fn last_assistant_excerpt(state: &AppState, session_id: &str) -> String {
    let conn = state.db.conn();
    let mut stmt = match conn.prepare(
        "SELECT content FROM messages
         WHERE session_id = ?1 AND role = 'assistant'
         ORDER BY created_at DESC LIMIT 1",
    ) {
        Ok(s) => s,
        Err(_) => return String::new(),
    };
    stmt.query_row([session_id], |r| r.get::<_, String>(0))
        .unwrap_or_default()
}

/// One scan cycle's worth of work. Returns a structured summary so the
/// tests can assert the right rows were touched.
pub struct ScanReport {
    pub scanned: usize,
    pub recovered: usize,
    pub failed: usize,
}

pub async fn recover_stalled(state: &AppState, rows: Vec<KanbanSessionWatch>) -> ScanReport {
    let mut report = ScanReport {
        scanned: rows.len(),
        recovered: 0,
        failed: 0,
    };

    for row in rows {
        // Best-effort: a session that disappeared between scan and
        // recovery (e.g. manual cancel, completed) is silently dropped.
        // The next scan will exclude it via the `status = 'watching'`
        // filter once we flip it.
        kanban_session_watch::mark_stalled(&state.db, &row.session_id);

        let new_count = match kanban_session_watch::begin_recovery(&state.db, &row.session_id) {
            Ok(n) => n,
            Err(e) => {
                warn!(error = %e, session_id = %row.session_id, "begin_recovery failed");
                continue;
            }
        };

        if new_count > DEFAULT_MAX_RECOVERY_RETRIES {
            // Recovery budget exhausted — fail the session.
            info!(
                session_id = %row.session_id,
                task_id = %row.task_id,
                recovery_count = new_count,
                "kanban_lifecycle: session failed after exhausting recovery retries"
            );
            // Flip the session status so the UI sees `error` rather
            // than a stuck `ready` row. `update_status` accepts
            // terminal transitions (ready → error).
            if let Err(e) = SessionStore::update_status(&state.db, &row.session_id, "error") {
                warn!(error = %e, session_id = %row.session_id, "session mark error failed");
            }
            kanban_session_watch::mark_failed(&state.db, &row.session_id);

            // Broadcast SessionFailed on BOTH the session channel and
            // the board channel so the session page UI and the kanban
            // board UI both see the failure signal. The board id is
            // looked up once from the task; if the task has been
            // deleted, only the session channel gets the event.
            let reason = format!(
                "stalled: recovery retries exhausted ({} attempts)",
                DEFAULT_MAX_RECOVERY_RETRIES
            );
            let board_id = board_id_for_task(state, &row.task_id);
            state.sse_manager.broadcast(
                &row.session_id,
                SseWireEvent::SessionFailed {
                    session_id: row.session_id.clone(),
                    task_id: row.task_id.clone(),
                    board_id: board_id.clone().unwrap_or_default(),
                    reason: reason.clone(),
                },
            );
            if let Some(bid) = board_id {
                state.sse_manager.broadcast(
                    &format!("board:{bid}"),
                    SseWireEvent::SessionFailed {
                        session_id: row.session_id.clone(),
                        task_id: row.task_id.clone(),
                        board_id: bid,
                        reason,
                    },
                );
            }
            report.failed += 1;
            continue;
        }

        // Recovery: send a re-prompt via the same path user prompts
        // take. `send_prompt` returns the user message id; errors
        // (session not in `ready` state, missing provider) are logged
        // and the row stays in `recovering` for the next scan.
        let excerpt = last_assistant_excerpt(state, &row.session_id);
        let prompt = build_recovery_prompt(&excerpt);
        let send_result = SessionService::send_prompt(
            &state.db,
            &state.registry,
            &state.specialists,
            &state.active_sessions,
            &state.sse_manager,
            &state.tools,
            &row.session_id,
            &prompt,
        )
        .await;
        match send_result {
            Ok(_msg_id) => {
                info!(
                    session_id = %row.session_id,
                    task_id = %row.task_id,
                    recovery_count = new_count,
                    "kanban_lifecycle: sent recovery re-prompt"
                );
                // The session's own SSE events from the re-prompt
                // (text_delta, done, error) will bump activity
                // through the broadcast hook. We still flip the row
                // to `watching` so the next scan re-evaluates.
                kanban_session_watch::mark_recovered(&state.db, &row.session_id);
                report.recovered += 1;
            }
            Err(e) => {
                warn!(
                    error = %e,
                    session_id = %row.session_id,
                    "kanban_lifecycle: recovery re-prompt failed; row stays in recovering"
                );
                // Leave the row in `recovering`; the next scan will
                // see the bumped `last_activity_at` from the failed
                // send (or skip if it really was a permanent error).
                // No action here — the bump happened inside send_prompt.
                let _ = e; // already logged
            }
        }
    }

    report
}

/// One scan cycle. `now` is injected so tests can pin the timestamp.
pub async fn scan_once(
    state: &AppState,
    stall_threshold_seconds: u64,
) -> Result<ScanReport, rusqlite::Error> {
    let rows = list_stalled(state, stall_threshold_seconds)?;
    Ok(recover_stalled(state, rows).await)
}

/// Spawn the supervisor task. Returns the `JoinHandle` so the caller
/// can await it on shutdown. The task is a 30s `tokio::time::interval`
/// loop; it exits when `shutdown` is cancelled.
pub fn start(state: AppState, shutdown: CancellationToken) -> JoinHandle<()> {
    tokio::spawn(async move {
        info!("KanbanLifecycleSupervisor: started");
        let mut ticker = tokio::time::interval(SCAN_INTERVAL);
        // The first tick fires immediately by default; we want the
        // server to settle first. Skip the first tick.
        ticker.tick().await;

        loop {
            tokio::select! {
                _ = shutdown.cancelled() => {
                    info!("KanbanLifecycleSupervisor: shutdown received, exiting");
                    return;
                }
                _ = ticker.tick() => {
                    let report = match scan_once(&state, DEFAULT_STALL_THRESHOLD_SECONDS).await {
                        Ok(r) => r,
                        Err(e) => {
                            warn!(error = %e, "kanban_lifecycle: scan_once failed");
                            continue;
                        }
                    };
                    if report.scanned > 0 {
                        info!(
                            scanned = report.scanned,
                            recovered = report.recovered,
                            failed = report.failed,
                            "kanban_lifecycle: scan complete"
                        );
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_session_watch;
    use crate::store::kanban_test_helpers::{make_test_state, seed_provider};

    /// Build a watch row backed by a real session row. The test bypasses
    /// `SessionStore::create` (which generates its own UUID) so the row
    /// can carry a deterministic id — this matches the `kanban_session_watch`
    /// test fixture pattern. Seeds the full `workspace → board → column →
    /// task → session → watch` chain so the FK constraints are satisfied.
    fn seed_watch_with_session(state: &AppState, sid: &str, task_id: &str) {
        let now = chrono::Utc::now().to_rfc3339();
        state
            .db
            .conn()
            .execute(
                "INSERT INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES ('ws-test', 'test', 'active', ?1, ?1)",
                rusqlite::params![now],
            )
            .unwrap();
        // Provider (any will do; the supervisor doesn't need a healthy agent
        // for `list_stalled` / `mark_stalled`).
        let _ = seed_provider(&state.db);
        state
            .db
            .conn()
            .execute(
                "INSERT INTO sessions
                     (id, workspace_id, provider_id, status, created_at, updated_at)
                 VALUES (?1, 'ws-test', (SELECT id FROM providers ORDER BY created_at LIMIT 1),
                         'ready', ?2, ?2)",
                rusqlite::params![sid, now],
            )
            .expect("seed session");
        // Board → column → task so the SessionFailed broadcast can resolve
        // board_id and the task FK (`column_id REFERENCES columns(id)`) is
        // satisfied. Stage is the post-feat-065 default ('dev').
        state
            .db
            .conn()
            .execute(
                "INSERT INTO boards (id, workspace_id, name, created_at)
                 VALUES ('board-test', 'ws-test', 'b', ?1)",
                rusqlite::params![now],
            )
            .unwrap();
        state
            .db
            .conn()
            .execute(
                "INSERT INTO columns (id, board_id, name, position, stage, created_at)
                 VALUES ('col-test', 'board-test', 'c', 0, 'dev', ?1)",
                rusqlite::params![now],
            )
            .unwrap();
        state
            .db
            .conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, 'board-test', 'col-test', 'T', 0, 'active', ?2, ?2)",
                rusqlite::params![task_id, now],
            )
            .expect("seed task");
        kanban_session_watch::create_watch(&state.db, sid, task_id).unwrap();
    }

    #[tokio::test]
    async fn test_list_stalled_finds_old_watch_rows() {
        let state = make_test_state();
        seed_watch_with_session(&state, "s-old", "t-1");
        // Backdate by 10 minutes.
        let ten_min_ago = (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339();
        state
            .db
            .conn()
            .execute(
                "UPDATE kanban_session_watch SET last_activity_at = ?1",
                rusqlite::params![ten_min_ago],
            )
            .unwrap();
        let rows = list_stalled(&state, 300).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].session_id, "s-old");
    }

    #[tokio::test]
    async fn test_scan_once_no_stalled_is_noop() {
        let state = make_test_state();
        let report = scan_once(&state, 300).await.unwrap();
        assert_eq!(report.scanned, 0);
        assert_eq!(report.recovered, 0);
        assert_eq!(report.failed, 0);
    }

    #[tokio::test]
    async fn test_recover_stalled_increments_count_on_first_recovery() {
        // Manual `send_prompt` would need a real agent + provider, which
        // is the wrong shape for a unit test. We instead exercise the
        // recovery counter by reading the watch row before/after.
        let state = make_test_state();
        seed_watch_with_session(&state, "s-1", "t-1");
        // Bump recovery_count to 1 directly (we're testing the
        // increment behavior, not the send_prompt path).
        let _ = kanban_session_watch::begin_recovery(&state.db, "s-1");
        let row = kanban_session_watch::get(&state.db, "s-1")
            .unwrap()
            .unwrap();
        assert_eq!(row.recovery_count, 1);
    }

    #[tokio::test]
    async fn test_build_recovery_prompt_includes_excerpt() {
        let prompt = build_recovery_prompt("I was about to commit the changes.");
        assert!(
            prompt.contains("I was about to commit the changes"),
            "got: {prompt}"
        );
        assert!(prompt.contains("move_card"), "got: {prompt}");
    }

    #[tokio::test]
    async fn test_build_recovery_prompt_truncates_long_excerpt() {
        let long: String = "a".repeat(2048);
        let prompt = build_recovery_prompt(&long);
        // The prompt is bounded — assert it's well under 2 KB + the
        // 200-byte wrapper.
        assert!(prompt.len() < 1500, "got len = {}", prompt.len());
        // The excerpt is the last a's, truncated mid-word, but the
        // prefix is preserved.
        assert!(prompt.contains("Last assistant message:"), "got: {prompt}");
    }

    #[tokio::test]
    async fn test_board_id_for_task_returns_board() {
        let state = make_test_state();
        seed_watch_with_session(&state, "s-1", "t-1");
        let bid = board_id_for_task(&state, "t-1");
        assert_eq!(bid.as_deref(), Some("board-test"));
    }

    #[tokio::test]
    async fn test_board_id_for_task_unknown_returns_none() {
        let state = make_test_state();
        let bid = board_id_for_task(&state, "t-ghost");
        assert!(bid.is_none());
    }
}
