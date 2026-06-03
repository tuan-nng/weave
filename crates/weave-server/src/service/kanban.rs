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
        None, // context_id — not used in kanban lane automation
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
// Transition gates (feat-028)
// ---------------------------------------------------------------------------

/// Enforce the three transition gates (feat-028) on a cross-column move.
///
/// 1. **Description frozen on exit** — if `current_column.freeze_description`,
///    the task must already have a non-empty `description`. The freeze
///    means "by the time you leave this column, the description is
///    captured".
///
/// 2. **Required fields on entry** — for each name in
///    `dest_column.required_fields`, the task's corresponding field must
///    be non-empty. Unknown field names are silently ignored (logged
///    at debug level).
///
/// 3. **Required artifact types on entry** (feat-031) — for each name
///    in `dest_column.required_artifact_types`, the task must have a
///    `provide_artifact` row of that type. The set of provided types
///    is pre-loaded by the caller via
///    `ArtifactStore::list_types_for_task` so the gate itself stays
///    a pure function over its inputs.
///
/// Pure function (read-only on the inputs, no I/O). Caller is expected
/// to have already loaded the source and destination columns AND the
/// set of artifact types present on the task.
///
/// A same-column move (source.id == dest.id) is not a transition and
/// short-circuits to `Ok(())` so callers don't have to special-case it.
/// A no-policy default template (no freeze, no required fields, no
/// required artifacts) also short-circuits before the per-field loop.
pub fn check_transition_gates(
    task: &Task,
    current_column: &Column,
    dest_column: &Column,
    present_artifact_types: &std::collections::HashSet<String>,
) -> Result<(), AppError> {
    // Same-column "move" is not a transition.
    if current_column.id == dest_column.id {
        return Ok(());
    }

    // Fast-path: no policies on either end of the move.
    if !current_column.freeze_description
        && dest_column.required_fields.is_empty()
        && dest_column.required_artifact_types.is_empty()
    {
        return Ok(());
    }

    // Gate 1: description frozen on exit.
    if current_column.freeze_description && is_blank(&task.description) {
        return Err(AppError::Validation(format!(
            "column '{}' freezes descriptions on exit; \
             set task.description before moving out",
            current_column.name
        )));
    }

    // Gate 2: required fields on entry. Each name maps to a known
    // `Task` field; unknown names are logged and skipped (column
    // config may be stale relative to the schema).
    for field_name in &dest_column.required_fields {
        let value: &Option<String> = match field_name.as_str() {
            "acceptance_criteria" => &task.acceptance_criteria,
            "completion_summary" => &task.completion_summary,
            "verification_report" => &task.verification_report,
            _ => {
                tracing::debug!(
                    field = %field_name,
                    "unknown required_field name; ignoring"
                );
                continue;
            }
        };
        if is_blank(value) {
            return Err(AppError::Validation(format!(
                "column '{}' requires '{}' to be non-empty before entry",
                dest_column.name, field_name
            )));
        }
    }

    // Gate 3: required artifact types on entry (feat-031). The
    // caller pre-loaded the set of types present on the task via
    // `ArtifactStore::list_types_for_task`. A missing required type
    // rejects the move; the error message names the missing type
    // and points the agent at `provide_artifact` for remediation.
    for required in &dest_column.required_artifact_types {
        if !present_artifact_types.contains(required) {
            return Err(AppError::Validation(format!(
                "column '{}' requires artifact of type '{}' before entry; \
                 use the provide_artifact tool to attach it",
                dest_column.name, required
            )));
        }
    }

    Ok(())
}

/// `true` when the value is `None` or only whitespace.
fn is_blank(s: &Option<String>) -> bool {
    s.as_deref().map(str::trim).map_or(true, str::is_empty)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::Db;
    use crate::store::artifacts::seed_artifact_row;
    use crate::store::kanban_test_helpers::{
        make_test_db, make_test_state, seed_provider_and_specialist,
    };
    use chrono::Utc;
    use std::collections::HashSet;

    fn default_column(id: &str, auto_trigger: bool, specialist_id: Option<&str>) -> Column {
        Column {
            id: id.into(),
            board_id: "board-test".into(),
            name: "Test".into(),
            position: 0,
            specialist_id: specialist_id.map(String::from),
            auto_trigger,
            freeze_description: false,
            required_fields: vec![],
            required_artifact_types: vec![],
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
        let column = default_column("col-test", false, Some("dev"));
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
        let column = default_column("col-test", true, None);
        let task = task_with("T", None);
        let result = try_automate_lane(&state, &task, &column).await.unwrap();
        assert_eq!(result, None);
    }

    #[tokio::test]
    async fn test_try_automate_lane_no_provider_returns_400_equivalent() {
        let state = make_test_state();
        // No `seed_provider` call — registry is empty.
        let column = default_column("col-test", true, Some("dev"));
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
        let column = default_column("col-test", true, Some("ghost"));
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

    // -----------------------------------------------------------------------
    // check_transition_gates tests (feat-028)
    // -----------------------------------------------------------------------

    fn gate_column(
        id: &str,
        freeze: bool,
        req_fields: Vec<&str>,
        req_artifacts: Vec<&str>,
    ) -> Column {
        let mut c = default_column(id, false, None);
        c.name = "Gate".into();
        c.freeze_description = freeze;
        c.required_fields = req_fields.into_iter().map(String::from).collect();
        c.required_artifact_types = req_artifacts.into_iter().map(String::from).collect();
        c
    }

    #[test]
    fn test_check_transition_gates_no_policies_passes() {
        let task = task_with("T", None);
        let src = default_column("col-src", false, None);
        let dst = default_column("col-dst", false, None);
        check_transition_gates(&task, &src, &dst, &HashSet::new()).unwrap();
    }

    #[test]
    fn test_check_transition_gates_freeze_blocks_empty_description() {
        let task = task_with("T", None); // no description
        let src = gate_column("col-src", true, vec![], vec![]);
        let dst = default_column("col-dst", false, None);
        let err = check_transition_gates(&task, &src, &dst, &HashSet::new()).unwrap_err();
        match err {
            AppError::Validation(msg) => {
                assert!(msg.contains("freezes descriptions"), "got: {msg}");
                assert!(msg.contains("set task.description"), "got: {msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn test_check_transition_gates_freeze_allows_non_empty_description() {
        let task = task_with("T", Some("body"));
        let src = gate_column("col-src", true, vec![], vec![]);
        let dst = default_column("col-dst", false, None);
        check_transition_gates(&task, &src, &dst, &HashSet::new()).unwrap();
    }

    #[test]
    fn test_check_transition_gates_freeze_allows_whitespace_only_as_blank() {
        // Whitespace-only description is treated as blank — the gate rejects it.
        let task = task_with("T", Some("   \n  "));
        let src = gate_column("col-src", true, vec![], vec![]);
        let dst = default_column("col-dst", false, None);
        assert!(check_transition_gates(&task, &src, &dst, &HashSet::new()).is_err());
    }

    #[test]
    fn test_check_transition_gates_required_field_blocks_when_missing() {
        let mut task = task_with("T", None);
        task.description = Some("body".into()); // bypass freeze
        let src = default_column("col-src", false, None);
        let dst = gate_column("col-dst", false, vec!["acceptance_criteria"], vec![]);
        let err = check_transition_gates(&task, &src, &dst, &HashSet::new()).unwrap_err();
        match err {
            AppError::Validation(msg) => {
                assert!(msg.contains("acceptance_criteria"), "got: {msg}");
                assert!(msg.contains("non-empty"), "got: {msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn test_check_transition_gates_required_field_passes_when_set() {
        let mut task = task_with("T", None);
        task.description = Some("body".into());
        task.acceptance_criteria = Some("AC".into());
        let src = default_column("col-src", false, None);
        let dst = gate_column("col-dst", false, vec!["acceptance_criteria"], vec![]);
        check_transition_gates(&task, &src, &dst, &HashSet::new()).unwrap();
    }

    #[test]
    fn test_check_transition_gates_multiple_required_fields_all_must_be_set() {
        let mut task = task_with("T", None);
        task.description = Some("body".into());
        task.acceptance_criteria = Some("AC".into());
        // verification_report still missing
        let src = default_column("col-src", false, None);
        let dst = gate_column(
            "col-dst",
            false,
            vec!["acceptance_criteria", "verification_report"],
            vec![],
        );
        let err = check_transition_gates(&task, &src, &dst, &HashSet::new()).unwrap_err();
        match err {
            AppError::Validation(msg) => {
                assert!(msg.contains("verification_report"), "got: {msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[test]
    fn test_check_transition_gates_unknown_required_field_name_silently_ignored() {
        // Backwards-compat: a column with a stale field name must not
        // brick every move. Unknown names are dropped (logged at debug).
        let mut task = task_with("T", None);
        task.description = Some("body".into());
        let src = default_column("col-src", false, None);
        let dst = gate_column("col-dst", false, vec!["made_up_field"], vec![]);
        check_transition_gates(&task, &src, &dst, &HashSet::new()).unwrap();
    }

    // -----------------------------------------------------------------------
    // Gate 3 — required artifact types (feat-031)
    // -----------------------------------------------------------------------

    /// Build a `Task` whose id matches a real `tasks` row in `db`.
    /// Required by the gate-3 tests because `ArtifactStore` resolves
    /// task ownership via the row, not the test fixture. Seeds a
    /// minimal `workspace → board → column → task` chain under fixed
    /// ids so the artifact FK (`artifacts.task_id → tasks.id`) is
    /// satisfied.
    fn task_with_real_id(db: &Db, task_id: &str) -> Task {
        let now = Utc::now().to_rfc3339();
        db.conn()
            .execute(
                "INSERT OR IGNORE INTO workspaces (id, name, status, created_at, updated_at)
                 VALUES ('ws-real', 'real', 'active', ?1, ?1)",
                rusqlite::params![now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT OR IGNORE INTO boards (id, workspace_id, name, created_at)
                 VALUES ('board-real', 'ws-real', 'real', ?1)",
                rusqlite::params![now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT OR IGNORE INTO columns (id, board_id, name, position, created_at)
                 VALUES ('col-real', 'board-real', 'real', 0, ?1)",
                rusqlite::params![now],
            )
            .unwrap();
        db.conn()
            .execute(
                "INSERT OR REPLACE INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
                 VALUES (?1, 'board-real', 'col-real', 'T', 0, 'active', ?2, ?2)",
                rusqlite::params![task_id, now],
            )
            .unwrap();
        let mut t = task_with("T", Some("body"));
        t.id = task_id.into();
        t
    }

    #[test]
    fn test_check_transition_gates_required_artifact_blocks_when_missing() {
        let db = make_test_db();
        let task_id = "task-arti-1";
        let task = task_with_real_id(&db, task_id);
        let src = default_column("col-src", false, None);
        let dst = gate_column("col-dst", false, vec![], vec!["test_results"]);
        let present: HashSet<String> = HashSet::new();
        let err = check_transition_gates(&task, &src, &dst, &present).unwrap_err();
        match err {
            AppError::Validation(msg) => {
                assert!(msg.contains("test_results"), "got: {msg}");
                assert!(msg.contains("provide_artifact"), "got: {msg}");
            }
            other => panic!("expected Validation, got: {other:?}"),
        }
    }

    #[test]
    fn test_check_transition_gates_required_artifact_passes_when_present() {
        let db = make_test_db();
        let task_id = "task-arti-2";
        let task = task_with_real_id(&db, task_id);
        seed_artifact_row(&db.conn(), task_id, "test_results", "ok");
        let src = default_column("col-src", false, None);
        let dst = gate_column("col-dst", false, vec![], vec!["test_results"]);
        let mut present = HashSet::new();
        present.insert("test_results".to_string());
        check_transition_gates(&task, &src, &dst, &present).unwrap();
    }

    #[test]
    fn test_check_transition_gates_required_artifact_partial_present_still_blocks() {
        let db = make_test_db();
        let task_id = "task-arti-3";
        let task = task_with_real_id(&db, task_id);
        // One of two required types is present.
        seed_artifact_row(&db.conn(), task_id, "test_results", "ok");
        let src = default_column("col-src", false, None);
        let dst = gate_column("col-dst", false, vec![], vec!["test_results", "screenshot"]);
        let mut present = HashSet::new();
        present.insert("test_results".to_string());
        let err = check_transition_gates(&task, &src, &dst, &present).unwrap_err();
        match err {
            AppError::Validation(msg) => {
                assert!(msg.contains("screenshot"), "got: {msg}");
            }
            other => panic!("expected Validation, got: {other:?}"),
        }
    }

    #[test]
    fn test_check_transition_gates_no_required_artifacts_with_present_set_passes() {
        // Empty requirement, non-empty present set, no other policies.
        let db = make_test_db();
        let task_id = "task-arti-4";
        let task = task_with_real_id(&db, task_id);
        seed_artifact_row(&db.conn(), task_id, "log", "trace");
        let src = default_column("col-src", false, None);
        let dst = gate_column("col-dst", false, vec![], vec![]);
        let mut present = HashSet::new();
        present.insert("log".to_string());
        check_transition_gates(&task, &src, &dst, &present).unwrap();
    }

    #[test]
    fn test_check_transition_gates_freeze_and_required_combined() {
        // A "Review" column with both freeze and required fields.
        let mut task = task_with("T", None);
        task.description = Some("body".into());
        // acceptance_criteria still missing.
        let src = gate_column("col-src", true, vec![], vec![]);
        let dst = gate_column("col-dst", false, vec!["acceptance_criteria"], vec![]);
        // src freeze passes (description is set); dst required fails first.
        let err = check_transition_gates(&task, &src, &dst, &HashSet::new()).unwrap_err();
        match err {
            AppError::Validation(msg) => {
                assert!(msg.contains("acceptance_criteria"), "got: {msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }
}
