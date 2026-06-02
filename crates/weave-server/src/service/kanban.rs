//! Lane automation: when a task moves to a column with `auto_trigger=true`
//! and a `specialist_id` bound, automatically spin up a session, link it
//! to the task, send an initial prompt, and broadcast a `session_started`
//! event on the board's SSE channel.
//!
//! This is the orchestrator that turns the kanban from a passive tracking
//! surface into a self-driving workflow — moving a card into a lane
//! becomes the trigger for an agent run. The card becomes the unit of
//! work; the column's `specialist_id` is the role the agent plays.

use crate::error::AppError;
use crate::service::sessions::SessionService;
use crate::sse::SseWireEvent;
use crate::store::columns::Column;
use crate::store::providers::ProviderStore;
use crate::store::tasks::{Task, TaskStore, UpdateTask};
use crate::AppState;

/// Build the initial prompt for the auto-spawned session.
///
/// Format is fixed by the feat-025 spec: `"Process task: {title}\n{description}"`.
/// When `description` is `None`, the prompt is `"Process task: {title}\n"` —
/// a literal trailing newline so the agent sees an explicit "no body" cue.
pub fn build_initial_prompt(task: &Task) -> String {
    match task.description.as_deref() {
        Some(desc) => format!("Process task: {}\n{}", task.title, desc),
        None => format!("Process task: {}\n", task.title),
    }
}

/// Pick the first provider in DB-creation order.
///
/// Providers are global (no `workspace_id` column), so a workspace with zero
/// providers globally has zero providers here. The decision per the feat-025
/// spec is: fail with a 400 if no provider exists; otherwise pick the first
/// in `created_at ASC` order (which `ProviderStore::list` already returns).
fn first_provider_id(db: &crate::db::Db) -> Result<String, AppError> {
    ProviderStore::list(db)?
        .into_iter()
        .next()
        .map(|p| p.id)
        .ok_or_else(|| {
            AppError::Validation(
                "no provider configured in workspace; add one via POST /api/providers \
                 before moving tasks to auto-trigger columns"
                    .into(),
            )
        })
}

/// Resolve the workspace id for a task via its board.
///
/// Mirrors the lookup at `api/kanban.rs:lookup_workspace_for_task`. Kept
/// inline here (rather than calling the API helper) so this module has no
/// dependency on the API layer.
fn workspace_id_for_task(db: &crate::db::Db, task_id: &str) -> Result<String, AppError> {
    db.conn()
        .query_row(
            "SELECT b.workspace_id FROM tasks t
             JOIN boards b ON b.id = t.board_id
             WHERE t.id = ?1",
            [task_id],
            |r| r.get(0),
        )
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => AppError::NotFound {
                resource: "task".into(),
                id: task_id.into(),
            },
            other => other.into(),
        })
}

/// Run lane automation after a task move.
///
/// Returns `Ok(None)` when the destination column is not auto-trigger or
/// has no specialist bound. Returns `Ok(Some(session_id))` when a session
/// was created and the initial prompt was submitted. Returns `Err(_)` for
/// setup failures (no provider, specialist not loaded on disk) — these
/// should be surfaced to the HTTP client as 400s so the user can fix them.
///
/// Caller is expected to have ALREADY pre-validated `column.auto_trigger`
/// and `column.specialist_id.is_some()` if it wants to short-circuit on
/// non-auto columns without paying for the function call. This function
/// re-checks defensively so it's safe to call unconditionally.
pub async fn try_automate_lane(
    state: &AppState,
    task: &Task,
    column: &Column,
) -> Result<Option<String>, AppError> {
    // Short-circuit: not an auto-trigger column. No work to do.
    if !column.auto_trigger {
        return Ok(None);
    }
    let specialist_id = match column.specialist_id.as_deref() {
        Some(id) if !id.is_empty() => id,
        // `validate_auto_trigger` already enforces this at column create/update,
        // so this branch should be unreachable in production. Defensive guard:
        // if a column somehow has `auto_trigger=true` with no specialist, treat
        // it as a non-auto column rather than error out (the spec doesn't
        // define a 4xx for this — silently skipping is the most forgiving
        // behavior).
        _ => return Ok(None),
    };

    // Pre-check 1: provider exists. Spec: fail with 4xx if no provider
    // is configured. This runs BEFORE the move would be safe — but in the
    // current call site (api/kanban.rs:update_task), the move has already
    // been committed by the time we get here. The error is still surfaced
    // to the user so they can fix it. Future improvement: move the
    // pre-check into the HTTP handler so the task isn't moved when
    // setup is invalid.
    let provider_id = first_provider_id(&state.db)?;

    // Pre-check 2: specialist is loaded. The DB doesn't FK on specialist_id
    // (specialists live on disk), so a typo'd `column.specialist_id` would
    // otherwise create a session that runs without a system prompt. Fail
    // fast with a clear 400.
    if state.specialists.get_by_name(specialist_id).is_none() {
        return Err(AppError::Validation(format!(
            "specialist '{specialist_id}' is not loaded; check resources/specialists/ \
             for a markdown file with `name: {specialist_id}` in its frontmatter"
        )));
    }

    // Create the session. The session starts in `connecting` status; the
    // spawned streaming task will transition it to `ready` then back to
    // `ready`/`completed`/etc. as the agent runs.
    let workspace_id = workspace_id_for_task(&state.db, &task.id)?;
    let session = SessionService::create_session(
        &state.db,
        &workspace_id,
        &provider_id,
        Some(specialist_id),
        None,
        None,
        None,
    )?;

    // Link the session to the task. `session_id: Some(Some(sid))` is the
    // tri-state "set" value — distinct from `None` (no change) and
    // `Some(None)` (clear).
    let link_update = UpdateTask {
        session_id: Some(Some(session.id.clone())),
        ..empty_update()
    };
    TaskStore::update(&state.db, &task.id, &workspace_id, &link_update)?;

    // Send the initial prompt. `send_prompt` is async — it persists the
    // user message, spawns the streaming task, and returns the user
    // message id. Errors here abort the lane (the session exists but
    // isn't running); the caller can decide whether to surface or ignore.
    let prompt = build_initial_prompt(task);
    SessionService::send_prompt(
        &state.db,
        &state.registry,
        &state.specialists,
        &state.active_sessions,
        &state.sse_manager,
        &state.tools,
        &session.id,
        &prompt,
    )
    .await?;

    // Broadcast the session-started event on the board's SSE channel.
    state.sse_manager.broadcast(
        &format!("board:{}", task.board_id),
        SseWireEvent::SessionStarted {
            session_id: session.id.clone(),
            task_id: task.id.clone(),
            specialist_id: specialist_id.to_string(),
            board_id: task.board_id.clone(),
        },
    );

    Ok(Some(session.id))
}

/// Construct an `UpdateTask` where every field is `None` (no-op).
///
/// Used to set `session_id` without touching any other field. The store
/// treats all-None as a no-op and returns the current row, but with
/// `session_id: Some(Some(sid))` it does write only that field.
fn empty_update() -> UpdateTask {
    UpdateTask {
        title: None,
        description: None,
        column_id: None,
        position: None,
        status: None,
        session_id: None,
        acceptance_criteria: None,
        completion_summary: None,
        verification_report: None,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::kanban_test_helpers::{make_test_state, seed_provider_and_specialist};

    fn default_column(auto_trigger: bool, specialist_id: Option<&str>) -> Column {
        Column {
            id: "col-test".into(),
            board_id: "board-test".into(),
            name: "Test".into(),
            position: 0,
            specialist_id: specialist_id.map(String::from),
            auto_trigger,
            created_at: "2026-06-02T00:00:00Z".into(),
        }
    }

    fn task_with(title: &str, description: Option<&str>) -> Task {
        Task {
            id: "task-test".into(),
            board_id: "board-test".into(),
            column_id: "col-test".into(),
            title: title.into(),
            description: description.map(String::from),
            position: 0,
            status: "active".into(),
            session_id: None,
            acceptance_criteria: None,
            completion_summary: None,
            verification_report: None,
            created_at: "2026-06-02T00:00:00Z".into(),
            updated_at: "2026-06-02T00:00:00Z".into(),
        }
    }

    #[test]
    fn test_build_initial_prompt_with_description() {
        let task = task_with("T", Some("D"));
        assert_eq!(build_initial_prompt(&task), "Process task: T\nD");
    }

    #[test]
    fn test_build_initial_prompt_without_description_uses_trailing_newline() {
        let task = task_with("T", None);
        // Literal trailing newline per spec; the agent interprets
        // "no body follows" as a cue that the description was empty.
        assert_eq!(build_initial_prompt(&task), "Process task: T\n");
    }

    #[tokio::test]
    async fn test_try_automate_lane_no_auto_trigger_is_noop() {
        let state = make_test_state();
        let column = default_column(false, Some("dev"));
        let task = task_with("T", None);
        let result = try_automate_lane(&state, &task, &column).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_try_automate_lane_auto_trigger_no_specialist_is_noop() {
        let state = make_test_state();
        // Defensive: if a column somehow has auto_trigger=true with no
        // specialist (the store's `validate_auto_trigger` should prevent
        // this), treat as non-auto.
        let column = default_column(true, None);
        let task = task_with("T", None);
        let result = try_automate_lane(&state, &task, &column).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_try_automate_lane_no_provider_returns_400_equivalent() {
        let state = make_test_state();
        // No `seed_provider` call — registry is empty.
        let column = default_column(true, Some("dev"));
        let task = task_with("T", None);
        let err = try_automate_lane(&state, &task, &column).await.unwrap_err();
        match err {
            AppError::Validation(msg) => assert!(msg.contains("no provider"), "got: {msg}"),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_try_automate_lane_specialist_missing_returns_400_equivalent() {
        let mut state = make_test_state();
        let (_provider_id, _specialist_name) = seed_provider_and_specialist(&mut state, "loaded");
        // Column references a different specialist that isn't loaded.
        let column = default_column(true, Some("ghost"));
        let task = task_with("T", None);
        let err = try_automate_lane(&state, &task, &column).await.unwrap_err();
        match err {
            AppError::Validation(msg) => {
                assert!(msg.contains("ghost"), "got: {msg}");
                assert!(msg.contains("not loaded"), "got: {msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }
}
