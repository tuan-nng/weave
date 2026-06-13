//! Store-IO assembler for [`KanbanPromptContext`] (feat-063).
//!
//! Walks four stores in a fixed order to build the input struct that
//! [`super::kanban_prompt::build_kanban_prompt`] consumes. Returns
//! an owned `KanbanPromptContext` — the borrowed `Task`/`Column`/`Board`
//! from the spec wording forced a `'static` return from this async
//! function, which is an anti-pattern.
//!
//! ## Store walk order
//!
//! 1. [`BoardStore::get_in_workspace`] — workspace-scoped
//!    defense-in-depth (the task row already guarantees the
//!    workspace match via its FK, but the explicit check matches
//!    the cross-workspace defense at `api/kanban.rs:179-186`).
//! 2. [`TaskStore::list_by_board`] — filter to same column, exclude
//!    self, take [`LANE_HISTORY_LIMIT`] with `session_id.is_some()`.
//!    For each, [`SessionStore::get_by_id`] → [`LaneSession`] row.
//! 3. [`ArtifactStore::list_types_for_task`] — pre-load artifact
//!    set. Mirrors the pre-load pattern in
//!    `service::kanban::check_transition_gates`.
//!
//! `is_first_active_run` is the result of step 2: true when the
//! filtered lane-sessions list is empty.

use crate::error::AppError;
use crate::store::artifacts::ArtifactStore;
use crate::store::boards::BoardStore;
use crate::store::columns::Column;
use crate::store::sessions::SessionStore;
use crate::store::tasks::{Task, TaskStore};
use crate::AppState;

use super::kanban_prompt::{KanbanPromptContext, LaneSession};

/// Maximum number of peer-task rows carried into Section 9 (Lane
/// History). The 5-cap is fixed by the spec — the assembler picks
/// the 5 most-recently-positioned peers (the cheapest correct order
/// given `TaskStore::list_by_board` returns `position ASC`).
const LANE_HISTORY_LIMIT: usize = 5;

/// Build a fully-loaded [`KanbanPromptContext`] ready for
/// [`super::kanban_prompt::build_kanban_prompt`].
///
/// `task` and `column` are caller-supplied (the auto-spawn path has
/// both). The function looks up `board` (from `task.board_id`),
/// lane peers (other tasks in `column` with sessions), and
/// `present_artifact_types` (from the artifact store). The returned
/// context is fully owned; the caller passes it straight to
/// `build_kanban_prompt` which consumes it by value and returns a
/// `String`.
///
/// # Errors
///
/// Returns `AppError::NotFound` when the board row is missing or
/// lives in a different workspace. Returns the same error from
/// `SessionStore::get_by_id` if a peer task's `session_id` points at
/// a deleted session — the assembler surfaces this rather than
/// silently dropping the row, because "session bound to a task that
/// is now gone" is a data-integrity condition that should not pass
/// unnoticed.
pub async fn assemble_kanban_prompt_context(
    state: &AppState,
    workspace_id: &str,
    task: &Task,
    column: &Column,
) -> Result<KanbanPromptContext, AppError> {
    let board = BoardStore::get_in_workspace(&state.db, &task.board_id, workspace_id)?;

    let mut lane_sessions: Vec<LaneSession> = Vec::new();
    let peer_tasks = TaskStore::list_by_board(&state.db, workspace_id, &task.board_id)?;
    for peer in peer_tasks
        .into_iter()
        .filter(|t| t.column_id == column.id && t.id != task.id && t.session_id.is_some())
        .take(LANE_HISTORY_LIMIT)
    {
        let sid = peer.session_id.as_deref().unwrap();
        // Defensive workspace check: `SessionStore::get_by_id` is
        // not workspace-scoped (Hard Constraint #5). The `task`
        // row already FKs to a workspace-scoped board, so a
        // sibling task's `session_id` pointing at a different
        // workspace would be a data-integrity break; skip the row
        // with a log instead of surfacing the cross-workspace
        // session to the prompt. A future
        // `SessionStore::get_in_workspace` would be the clean
        // fix; the inline check is the cheapest correct answer
        // for v1.
        let session = SessionStore::get_by_id(&state.db, sid)?;
        if session.workspace_id != workspace_id {
            tracing::warn!(
                peer_task_id = %peer.id,
                peer_session_id = %sid,
                task_workspace = %workspace_id,
                session_workspace = %session.workspace_id,
                "skipping lane-history peer: session lives in a different workspace"
            );
            continue;
        }
        lane_sessions.push(LaneSession {
            task_title: peer.title,
            session_id: sid.to_string(),
            session_role: session.specialist_id.unwrap_or_else(|| "(none)".into()),
            session_status: session.status,
            started_at: session.created_at,
        });
    }

    let is_first_active_run = lane_sessions.is_empty();
    let present_artifact_types =
        ArtifactStore::list_types_for_task(&state.db, &task.id, workspace_id)?;

    Ok(KanbanPromptContext {
        task: task.clone(),
        column: column.clone(),
        board,
        lane_sessions,
        present_artifact_types,
        is_first_active_run,
    })
}

// ---------------------------------------------------------------------------
// Tests — store IO via `make_test_db` / `make_test_state`.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::store::artifacts::seed_artifact_row;
    use crate::store::columns::ColumnStage;
    use crate::store::kanban_test_helpers::{make_test_state, seed_workspace_with_board};
    use chrono::Utc;
    use rusqlite::params;

    /// Insert a `(workspace_id, task_id, board_id, column_id, session_id)`
    /// fixture row set. The session row is the minimum legal shape
    /// — status='connecting', no messages.
    fn seed_task_with_session(
        db: &Db,
        workspace_id: &str,
        board_id: &str,
        column_id: &str,
        task_id: &str,
        session_id: &str,
    ) {
        let provider_id = crate::store::kanban_test_helpers::seed_provider(db);
        let now = Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO sessions (id, workspace_id, provider_id, status, runtime_kind, mode, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'ready', 'claude-code', 'wrapped', ?4, ?4)",
                params![session_id, workspace_id, provider_id, now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, session_id, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'peer', 0, 'active', ?4, ?5, ?5)",
                params![task_id, board_id, column_id, session_id, now],
            )
            .unwrap();
    }

    fn seed_task_without_session(db: &Db, board_id: &str, column_id: &str, task_id: &str) {
        let now = Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, 'peer', 0, 'active', ?4, ?4)",
                params![task_id, board_id, column_id, now],
            )
            .unwrap();
    }

    /// Insert the in-memory `task_fixture` row into the DB. Required
    /// before any code that needs the task row to exist (e.g., the
    /// artifact FK on `seed_artifact_row`).
    fn seed_task(db: &Db, task: &Task) {
        let now = Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT INTO tasks (id, board_id, column_id, title, description, position, status, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
                params![
                    task.id,
                    task.board_id,
                    task.column_id,
                    task.title,
                    task.description,
                    task.position,
                    task.status,
                    now,
                ],
            )
            .unwrap();
    }

    fn column_fixture(board_id: &str, column_id: &str) -> Column {
        Column {
            id: column_id.into(),
            board_id: board_id.into(),
            name: "Backlog".into(),
            position: 0,
            specialist_id: Some("dev".into()),
            auto_trigger: true,
            freeze_description: false,
            required_fields: vec![],
            required_artifact_types: vec![],
            runtime_kind: None,
            stage: ColumnStage::Backlog,
            created_at: "2026-06-12T00:00:00Z".into(),
        }
    }

    fn task_fixture(task_id: &str, board_id: &str, column_id: &str) -> Task {
        Task {
            id: task_id.into(),
            board_id: board_id.into(),
            column_id: column_id.into(),
            title: "current card".into(),
            description: Some("body".into()),
            position: 100,
            status: "active".into(),
            session_id: None,
            acceptance_criteria: None,
            completion_summary: None,
            verification_report: None,
            priority: None,
            labels: None,
            scope: None,
            verification_commands: None,
            test_cases: None,
            created_at: "2026-06-12T00:00:00Z".into(),
            updated_at: "2026-06-12T00:00:00Z".into(),
        }
    }

    #[tokio::test]
    async fn test_assemble_returns_empty_lane_when_solo_card() {
        let state = crate::store::kanban_test_helpers::make_test_state();
        let (ws, board_id, column_id) = seed_workspace_with_board(&state.db);
        let column = column_fixture(&board_id, &column_id);
        let task = task_fixture("task-current", &board_id, &column_id);
        let ctx = assemble_kanban_prompt_context(&state, &ws, &task, &column)
            .await
            .expect("assemble");
        assert!(ctx.lane_sessions.is_empty());
        assert!(ctx.is_first_active_run);
        assert!(ctx.present_artifact_types.is_empty());
        assert_eq!(ctx.board.id, board_id);
    }

    #[tokio::test]
    async fn test_assemble_finds_lane_sessions_for_other_cards() {
        let state = crate::store::kanban_test_helpers::make_test_state();
        let (ws, board_id, column_id) = seed_workspace_with_board(&state.db);
        let column = column_fixture(&board_id, &column_id);
        let current = task_fixture("task-current", &board_id, &column_id);
        // Two peer tasks in the same column, both with sessions.
        seed_task_with_session(
            &state.db,
            &ws,
            &board_id,
            &column_id,
            "task-peer-1",
            "sess-1",
        );
        seed_task_with_session(
            &state.db,
            &ws,
            &board_id,
            &column_id,
            "task-peer-2",
            "sess-2",
        );
        let ctx = assemble_kanban_prompt_context(&state, &ws, &current, &column)
            .await
            .expect("assemble");
        assert_eq!(ctx.lane_sessions.len(), 2);
        assert!(!ctx.is_first_active_run);
        let titles: Vec<&str> = ctx
            .lane_sessions
            .iter()
            .map(|s| s.task_title.as_str())
            .collect();
        // Both peers share the seeded title "peer"; only the ids
        // and session ids distinguish them.
        assert_eq!(titles, vec!["peer", "peer"]);
        let sids: Vec<&str> = ctx
            .lane_sessions
            .iter()
            .map(|s| s.session_id.as_str())
            .collect();
        assert!(sids.contains(&"sess-1"));
        assert!(sids.contains(&"sess-2"));
        // Self exclusion — current card's id is "task-current" and
        // it has no session, so it must not appear.
        assert!(!sids.iter().any(|s| s.contains("task-current")));
    }

    #[tokio::test]
    async fn test_assemble_caps_lane_history_at_five() {
        let state = crate::store::kanban_test_helpers::make_test_state();
        let (ws, board_id, column_id) = seed_workspace_with_board(&state.db);
        let column = column_fixture(&board_id, &column_id);
        let current = task_fixture("task-current", &board_id, &column_id);
        // Seed 8 peers — the assembler must cap at 5.
        for i in 0..8 {
            let task_id = format!("task-peer-{i}");
            let sess_id = format!("sess-{i}");
            seed_task_with_session(&state.db, &ws, &board_id, &column_id, &task_id, &sess_id);
        }
        let ctx = assemble_kanban_prompt_context(&state, &ws, &current, &column)
            .await
            .expect("assemble");
        assert_eq!(ctx.lane_sessions.len(), 5);
    }

    #[tokio::test]
    async fn test_assemble_skips_peers_without_sessions() {
        let state = crate::store::kanban_test_helpers::make_test_state();
        let (ws, board_id, column_id) = seed_workspace_with_board(&state.db);
        let column = column_fixture(&board_id, &column_id);
        let current = task_fixture("task-current", &board_id, &column_id);
        // 1 peer with session, 2 peers without.
        seed_task_with_session(
            &state.db,
            &ws,
            &board_id,
            &column_id,
            "task-peer-sess",
            "sess-only",
        );
        seed_task_without_session(&state.db, &board_id, &column_id, "task-peer-no-sess-1");
        seed_task_without_session(&state.db, &board_id, &column_id, "task-peer-no-sess-2");
        let ctx = assemble_kanban_prompt_context(&state, &ws, &current, &column)
            .await
            .expect("assemble");
        assert_eq!(ctx.lane_sessions.len(), 1);
        assert_eq!(ctx.lane_sessions[0].session_id, "sess-only");
    }

    #[tokio::test]
    async fn test_assemble_collects_present_artifact_types() {
        let state = make_test_state();
        let (ws, board_id, column_id) = seed_workspace_with_board(&state.db);
        let column = column_fixture(&board_id, &column_id);
        let current = task_fixture("task-current", &board_id, &column_id);
        // The artifact FK needs the task row in the DB. Insert it.
        seed_task(&state.db, &current);
        seed_artifact_row(&state.db.conn(), &current.id, "test_results", "ok");
        seed_artifact_row(&state.db.conn(), &current.id, "screenshot", "png");
        let ctx = assemble_kanban_prompt_context(&state, &ws, &current, &column)
            .await
            .expect("assemble");
        assert!(ctx.present_artifact_types.contains("test_results"));
        assert!(ctx.present_artifact_types.contains("screenshot"));
        assert_eq!(ctx.present_artifact_types.len(), 2);
    }

    #[tokio::test]
    async fn test_assemble_rejects_wrong_workspace() {
        // The board row exists in workspace A; the assembler is
        // called with workspace B's id. `get_in_workspace` returns
        // NotFound — the assembler must surface it.
        let state = crate::store::kanban_test_helpers::make_test_state();
        let (_ws, board_id, column_id) = seed_workspace_with_board(&state.db);
        let column = column_fixture(&board_id, &column_id);
        let current = task_fixture("task-current", &board_id, &column_id);
        let err = assemble_kanban_prompt_context(&state, "ws-other", &current, &column)
            .await
            .unwrap_err();
        match err {
            AppError::NotFound { resource, .. } => {
                assert_eq!(resource, "board");
            }
            other => panic!("expected NotFound(board), got {other:?}"),
        }
    }
}
