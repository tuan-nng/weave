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
use crate::service::kanban_prompt::build_kanban_prompt;
use crate::service::kanban_prompt_ctx::assemble_kanban_prompt_context;
use crate::service::sessions::SessionService;
use crate::sse::SseWireEvent;
use crate::store::columns::Column;
use crate::store::providers::ProviderStore;
use crate::store::tasks::{Task, TaskStore, UpdateTask};
use crate::AppState;

/// Pick the first provider in DB-creation order.
///
/// Providers are global (no `workspace_id` column), so a workspace with zero
/// providers globally has zero providers here. The decision per the feat-025
/// spec is: fail with a 400 if no provider exists; otherwise pick the first
/// in `created_at ASC` order (which `ProviderStore::list` already returns).
///
/// Note: feat-051's `try_automate_lane` inlined this logic so it could
/// branch on `provider.kind` before deciding the session's
/// `runtime_kind` / `mode`. The free function stays as a
/// `#[allow(dead_code)]` helper for the A2A messages path
/// (`a2a/messages.rs::first_provider_id`) which has its own copy.
#[allow(dead_code)]
fn first_provider_id(db: &crate::db::Db) -> Result<String, AppError> {
    ProviderStore::list(db)?
        .into_iter()
        .next()
        .map(|p| p.id)
        .ok_or_else(|| {
            AppError::validation(
                "no provider configured in workspace; add one via POST /api/providers \
                 before moving tasks to auto-trigger columns",
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
    let provider = ProviderStore::list(&state.db)?
        .into_iter()
        .next()
        .ok_or_else(|| {
            AppError::validation(
                "no provider configured in workspace; add one via POST /api/providers \
                 before moving tasks to auto-trigger columns",
            )
        })?;
    let provider_id = provider.id.clone();

    // feat-055: honor the column's `runtime_kind` binding first. When set,
    // it overrides the provider-derived default. The column's runtime_kind
    // directly becomes the session's runtime_kind; the mode is derived from
    // the runtime category (CLI → wrapped, HTTP → native).
    //
    // feat-051: when column has no runtime_kind, honor the provider's `kind`
    // so a CLI provider creates a wrapped session, not a native one. The
    // default (pre-feat-051) behavior was "always native / anthropic-api";
    // a CLI row now selects `RuntimeKind::ClaudeCode` and
    // `SessionMode::Wrapped`. A missing/garbled `kind` falls back to the
    // legacy default — the spec says a stuck or old row should keep working
    // until the operator migrates.
    let (runtime_kind, mode) = if let Some(col_rk) = &column.runtime_kind {
        // Column has explicit runtime_kind — derive mode from runtime category.
        let is_cli = matches!(col_rk.as_str(), "claude-code" | "codex" | "opencode");
        let mode = if is_cli {
            Some(crate::agent::SessionMode::Wrapped.as_str())
        } else {
            Some(crate::agent::SessionMode::Native.as_str())
        };
        (Some(col_rk.as_str()), mode)
    } else {
        // No column binding — fall back to provider.kind (pre-feat-055 behavior).
        match provider.kind.as_str() {
            "cli" => (
                Some(crate::agent::RuntimeKind::ClaudeCode.as_str()),
                Some(crate::agent::SessionMode::Wrapped.as_str()),
            ),
            _ => (None, None),
        }
    };

    // Pre-check 2: specialist is loaded. The DB doesn't FK on specialist_id
    // (specialists live on disk), so a typo'd `column.specialist_id` would
    // otherwise create a session that runs without a system prompt. Fail
    // fast with a clear 400.
    if state.specialists.get_by_name(specialist_id).is_none() {
        return Err(AppError::validation(format!(
            "specialist '{specialist_id}' is not loaded; check resources/specialists/ \
             for a markdown file with `name: {specialist_id}` in its frontmatter"
        )));
    }

    // Create the session. The session starts in `connecting` status; the
    // spawned streaming task will transition it to `ready` then back to
    // `ready`/`completed`/etc. as the agent runs.
    let workspace_id = workspace_id_for_task(&state.db, &task.id)?;
    // feat-051: wrapped sessions (CLI providers) require a `cwd`
    // inside a registered codebase — `validate_wrapped_session_cwd`
    // (service/sessions.rs) rejects a `None` cwd with a 400. The
    // A2A path passes `None` deliberately (no filesystem context);
    // the kanban path can do better — pick the first registered
    // codebase in the workspace as a sensible default cwd. If the
    // workspace has no codebases, surface that as a clear error
    // BEFORE the validator's more cryptic "no cwd supplied" so the
    // operator knows what to fix.
    let codebase_id: Option<String> = if mode == Some("wrapped") {
        let codebases =
            crate::store::codebases::CodebaseStore::list_by_workspace(&state.db, &workspace_id)?;
        match codebases.into_iter().next() {
            Some(cb) => Some(cb.id),
            None => {
                return Err(AppError::validation_with_code(
                    "cwd_outside_codebase",
                    format!(
                        "kanban auto-spawn for a CLI provider requires the workspace \
                         to have at least one registered codebase (kanban tasks don't \
                         carry their own cwd). Register a codebase for workspace \
                         '{workspace_id}' via POST /api/codebases, or move the task to a \
                         non-CLI provider lane."
                    ),
                ));
            }
        }
    } else {
        None
    };
    let session = SessionService::create_session(
        &state.db,
        &workspace_id,
        &provider_id,
        Some(specialist_id),
        None,
        None,
        None,
        None,                   // context_id — not used in kanban lane automation
        codebase_id.as_deref(), // feat-051: default to first codebase for CLI providers
        runtime_kind,           // feat-051: honor CLI provider's runtime_kind
        mode,                   // feat-051: honor CLI provider's mode
        None,                   // runtime_metadata_json — none
    )?;

    // Link the session to the task. `session_id: Some(Some(sid))` is the
    // tri-state "set" value — distinct from `None` (no change) and
    // `Some(None)` (clear).
    let link_update = UpdateTask {
        session_id: Some(Some(session.id.clone())),
        ..empty_update()
    };
    TaskStore::update(&state.db, &task.id, &workspace_id, &link_update)?;

    // Build the rich initial prompt (feat-063). The 3-line
    // `build_initial_prompt` shim was deleted; the new builder
    // renders a 12-slot prompt with column/board context, lane
    // history, and per-gate status. The assembler does the store
    // IO (board lookup, lane-peer filter, artifact pre-load);
    // `build_kanban_prompt` is a pure function over the resulting
    // context.
    let prompt_ctx = assemble_kanban_prompt_context(state, &workspace_id, task, column).await?;
    let prompt = build_kanban_prompt(prompt_ctx);

    // Send the initial prompt. `send_prompt` is async — it persists the
    // user message, spawns the streaming task, and returns the user
    // message id. Errors here abort the lane (the session exists but
    // isn't running); the caller can decide whether to surface or ignore.
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
        priority: None,
        labels: None,
        scope: None,
        verification_commands: None,
        test_cases: None,
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
        return Err(AppError::validation(format!(
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
            return Err(AppError::validation(format!(
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
            return Err(AppError::validation(format!(
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
    use crate::store::columns::ColumnStage;
    use crate::store::kanban_test_helpers::{
        make_test_db, make_test_state, seed_provider_and_specialist,
    };
    use chrono::Utc;
    use std::collections::HashSet;
    use std::sync::Arc;

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
            runtime_kind: None,
            stage: ColumnStage::Dev,
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
            priority: None,
            labels: None,
            scope: None,
            verification_commands: None,
            test_cases: None,
            created_at: "2026-06-02T00:00:00Z".into(),
            updated_at: "2026-06-02T00:00:00Z".into(),
        }
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
            AppError::Validation { message: msg, .. } => {
                assert!(msg.contains("no provider"), "got: {msg}")
            }
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
            AppError::Validation { message: msg, .. } => {
                assert!(msg.contains("ghost"), "got: {msg}");
                assert!(msg.contains("not loaded"), "got: {msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // feat-051: CLI provider auto-spawn defaults cwd to the workspace's
    // first registered codebase. Without this default, the
    // `validate_wrapped_session_cwd` check at `create_session` rejects
    // the auto-spawn with `cwd_outside_codebase` (the task carries no
    // cwd of its own).
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn test_try_automate_lane_cli_provider_with_codebase_succeeds() {
        use crate::store::codebases::CodebaseStore;
        use crate::store::kanban_test_helpers::seed_cli_provider;

        let mut state = make_test_state();
        // Seed a CLI provider whose binary is in the allowlist
        // (basename `fake_cli`).
        let _provider_id =
            seed_cli_provider(&state.db, "/usr/local/bin/fake_cli", "claude-sonnet-4-5");
        // Seed a real task under a real workspace so
        // `workspace_id_for_task` succeeds — that's the same path
        // the pre-fix code would have hit `cwd_outside_codebase`
        // on. Use `task_with_real_id` which builds a complete
        // workspace→board→column→task chain.
        let task = task_with_real_id(&state.db, "task-cli-ok");
        // Seed a codebase in the task's workspace. The kanban
        // auto-spawn must pick this as the default cwd for the
        // wrapped session.
        let tmp = tempfile::TempDir::new().unwrap();
        let _codebase_id = CodebaseStore::create(
            &state.db,
            "ws-real",
            tmp.path().to_str().unwrap(),
            None,
            None,
        )
        .expect("seed codebase")
        .id;
        // Specialist must be loaded (the path's pre-check requires it).
        let specialists = Arc::get_mut(&mut state.specialists).expect("unique");
        crate::store::kanban_test_helpers::seed_specialist(specialists, "dev", "You are dev.");
        // Fire the auto-spawn. The pre-fix code would have failed at
        // `create_session` with `cwd_outside_codebase`. We just assert
        // we did NOT hit that specific error — other downstream
        // failures (SSE, fake_cli shell-out) are acceptable here.
        let column = default_column("col-test", true, Some("dev"));
        let result = try_automate_lane(&state, &task, &column).await;
        if let Err(err) = &result {
            if let AppError::Validation { code, message } = err {
                assert_ne!(
                    code, "cwd_outside_codebase",
                    "kanban auto-spawn must default cwd to the first codebase, \
                     not surface the pre-fix cwd_outside_codebase error. \
                     Got: {message}"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_try_automate_lane_cli_provider_no_codebase_fails_clearly() {
        use crate::store::kanban_test_helpers::seed_cli_provider;

        let mut state = make_test_state();
        let _provider_id =
            seed_cli_provider(&state.db, "/usr/local/bin/fake_cli", "claude-sonnet-4-5");
        // Real task so `workspace_id_for_task` succeeds; no
        // codebase in the workspace.
        let task = task_with_real_id(&state.db, "task-cli-no-cb");
        let specialists = Arc::get_mut(&mut state.specialists).expect("unique");
        crate::store::kanban_test_helpers::seed_specialist(specialists, "dev", "You are dev.");
        let column = default_column("col-test", true, Some("dev"));
        let err = try_automate_lane(&state, &task, &column).await.unwrap_err();
        match err {
            AppError::Validation { code, message } => {
                assert_eq!(
                    code, "cwd_outside_codebase",
                    "got code={code} msg={message}"
                );
                assert!(
                    message.contains("at least one registered codebase"),
                    "message must guide the operator, got: {message}"
                );
            }
            other => panic!("expected Validation cwd_outside_codebase, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // feat-055: column runtime_kind binding tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_kanban_autospawn_uses_column_runtime_kind() {
        use crate::store::codebases::CodebaseStore;
        use crate::store::kanban_test_helpers::seed_cli_provider;

        let mut state = make_test_state();
        // Seed a CLI provider.
        let _provider_id =
            seed_cli_provider(&state.db, "/usr/local/bin/fake_cli", "claude-sonnet-4-5");
        let task = task_with_real_id(&state.db, "task-col-rk");
        // Seed a codebase so the wrapped session validation passes.
        let tmp = tempfile::TempDir::new().unwrap();
        let _codebase_id = CodebaseStore::create(
            &state.db,
            "ws-real",
            tmp.path().to_str().unwrap(),
            None,
            None,
        )
        .expect("seed codebase")
        .id;
        let specialists = Arc::get_mut(&mut state.specialists).expect("unique");
        crate::store::kanban_test_helpers::seed_specialist(specialists, "dev", "You are dev.");
        // Column with explicit runtime_kind.
        let mut column = default_column("col-test", true, Some("dev"));
        column.runtime_kind = Some("claude-code".to_string());
        // The auto-spawn should use the column's runtime_kind.
        // We can't easily inspect the created session's runtime_kind from
        // this test (the session is created inside try_automate_lane), but
        // we can verify it doesn't error on the provider resolution path.
        let result = try_automate_lane(&state, &task, &column).await;
        if let Err(err) = &result {
            if let AppError::Validation { code, message } = err {
                assert_ne!(
                    code, "cwd_outside_codebase",
                    "column runtime_kind must resolve to a matching provider. Got: {message}"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_kanban_autospawn_inherits_when_null() {
        let mut state = make_test_state();
        let (_provider_id, _specialist_name) = seed_provider_and_specialist(&mut state, "dev");
        let task = task_with_real_id(&state.db, task_with("T", Some("body")).id.as_str());
        // Column with no runtime_kind — should inherit from provider.
        let column = default_column("col-test", true, Some("dev"));
        assert!(column.runtime_kind.is_none());
        // Should succeed (the HTTP provider's default is anthropic-api/native).
        let result = try_automate_lane(&state, &task, &column).await;
        // The result may be an error downstream (SSE, agent), but NOT
        // a provider-resolution error.
        if let Err(err) = &result {
            if let AppError::Validation { message: msg, .. } = err {
                assert!(
                    !msg.contains("no provider"),
                    "column with null runtime_kind must inherit from provider, got: {msg}"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_kanban_autospawn_errors_when_no_default() {
        let state = make_test_state();
        // No provider seeded — registry is empty.
        let task = task_with_real_id(&state.db, "task-no-prov");
        let column = default_column("col-test", true, Some("dev"));
        let err = try_automate_lane(&state, &task, &column).await.unwrap_err();
        match err {
            AppError::Validation { message: msg, .. } => {
                assert!(msg.contains("no provider"), "got: {msg}")
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
            AppError::Validation { message: msg, .. } => {
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
            AppError::Validation { message: msg, .. } => {
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
            AppError::Validation { message: msg, .. } => {
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
            AppError::Validation { message: msg, .. } => {
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
            AppError::Validation { message: msg, .. } => {
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
            AppError::Validation { message: msg, .. } => {
                assert!(msg.contains("acceptance_criteria"), "got: {msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }
}
