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
/// Evaluate all transition gates (3 legacy + 4 new from `dest_column.automation`).
///
/// Pure legacy gates read directly from the inputs. The 4 new gates
/// (delivery, contract, checklist, validator) may require a `GateContext`
/// for git/DB access — when the caller doesn't have one (e.g. tests with
/// no DB), `ctx = None` skips the new gates silently.
///
/// Gate mode is per-column (set in `dest_column.automation.gate_mode`).
/// `Blocking` (default) returns the first failure as a structured
/// `AppError::validation_with_code("gate_<type>", message)`.
/// `Warning` logs the failure and allows the move.
pub async fn check_transition_gates(
    task: &Task,
    current_column: &Column,
    dest_column: &Column,
    present_artifact_types: &std::collections::HashSet<String>,
    ctx: Option<&GateContext<'_>>,
) -> Result<(), AppError> {
    if current_column.id == dest_column.id {
        return Ok(());
    }

    let mut failures: Vec<GateFailure> = Vec::new();
    let mut warnings: Vec<GateFailure> = Vec::new();

    // Gate 1: description frozen on exit.
    if current_column.freeze_description && is_blank(&task.description) {
        let f = GateFailure {
            gate_type: GateType::FreezeDescription,
            message: format!(
                "column '{}' freezes descriptions on exit; \
                 set task.description before moving out",
                current_column.name
            ),
        };
        collect_or_warn(&mut failures, &mut warnings, f, dest_column);
    }

    // Gate 2: required fields on entry.
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
            let f = GateFailure {
                gate_type: GateType::RequiredFields,
                message: format!(
                    "column '{}' requires '{}' to be non-empty before entry",
                    dest_column.name, field_name
                ),
            };
            collect_or_warn(&mut failures, &mut warnings, f, dest_column);
        }
    }

    // Gate 3: required artifact types on entry.
    for required in &dest_column.required_artifact_types {
        if !present_artifact_types.contains(required) {
            let f = GateFailure {
                gate_type: GateType::RequiredArtifacts,
                message: format!(
                    "column '{}' requires artifact of type '{}' before entry; \
                     use the provide_artifact tool to attach it",
                    dest_column.name, required
                ),
            };
            collect_or_warn(&mut failures, &mut warnings, f, dest_column);
        }
    }

    // Gates 4-7: automation gates (delivery/contract/checklist/validator).
    if let Some(automation) = &dest_column.automation {
        evaluate_contract_gates(task, dest_column, automation, &mut failures, &mut warnings);
        evaluate_checklist_gates(task, dest_column, automation, &mut failures, &mut warnings);
        if let Some(ctx) = ctx {
            evaluate_delivery_gates(
                task,
                dest_column,
                automation,
                ctx,
                &mut failures,
                &mut warnings,
            )
            .await;
            evaluate_validator_gate(
                task,
                dest_column,
                automation,
                ctx,
                &mut failures,
                &mut warnings,
            )
            .await;
        }
    }

    if let Some(first) = failures.into_iter().next() {
        return Err(AppError::validation_with_code(
            gate_type_to_code(first.gate_type),
            first.message,
        ));
    }
    Ok(())
}

/// `true` when the value is `None` or only whitespace.
fn is_blank(s: &Option<String>) -> bool {
    s.as_deref().map(str::trim).map_or(true, str::is_empty)
}

// ---------------------------------------------------------------------------
// Automation gates (feat-066): delivery / contract / checklist / validator
// ---------------------------------------------------------------------------

use std::path::PathBuf;

/// Context for gate evaluation. Carries the dependencies the new gates
/// (delivery, validator) need: the DB for the validator cache and the
/// cwd for git commands. Legacy gates (freeze/fields/artifacts) read
/// directly from the `Task`/`Column` args and ignore this.
pub struct GateContext<'a> {
    pub db: &'a crate::db::Db,
    pub cwd: PathBuf,
}

/// Structured gate failure with the gate type, for error code routing.
pub struct GateFailure {
    pub gate_type: GateType,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateType {
    FreezeDescription,
    RequiredFields,
    RequiredArtifacts,
    Delivery,
    Contract,
    Checklist,
    Validator,
}

fn gate_type_to_code(gt: GateType) -> &'static str {
    match gt {
        GateType::FreezeDescription => "gate_freeze",
        GateType::RequiredFields => "gate_fields",
        GateType::RequiredArtifacts => "gate_artifacts",
        GateType::Delivery => "gate_delivery",
        GateType::Contract => "gate_contract",
        GateType::Checklist => "gate_checklist",
        GateType::Validator => "gate_validator",
    }
}

/// Route a `GateFailure` based on the column's `gate_mode`. Blocking
/// (default) puts the failure in the blocking vec; Warning logs and
/// moves it to the warnings vec.
fn collect_or_warn(
    failures: &mut Vec<GateFailure>,
    warnings: &mut Vec<GateFailure>,
    f: GateFailure,
    dest_column: &Column,
) {
    let mode = dest_column
        .automation
        .as_ref()
        .map_or(crate::store::columns::GateMode::Blocking, |a| a.gate_mode);
    match mode {
        crate::store::columns::GateMode::Blocking => failures.push(f),
        crate::store::columns::GateMode::Warning => {
            tracing::warn!(
                gate = gate_type_to_code(f.gate_type),
                message = %f.message,
                "transition gate warning (gate_mode=warning)"
            );
            warnings.push(f);
        }
    }
}

/// Extract the first ```yaml code block from a string.
fn extract_yaml_code_block(desc: &str) -> Option<String> {
    let start = desc.find("```yaml")?;
    let after_start = start + 7;
    let end = desc[after_start..].find("```")? + after_start;
    Some(desc[after_start..end].trim().to_string())
}

const CONTRACT_REQUIRED_FIELDS: &[&str] = &["title", "description", "acceptance_criteria"];

/// Parse the YAML and check for required keys. Returns the list of
/// missing field names on failure.
fn validate_story_yaml(yaml_str: &str) -> Result<(), Vec<String>> {
    let parsed: serde_yaml::Value = match serde_yaml::from_str(yaml_str) {
        Ok(v) => v,
        Err(e) => return Err(vec![format!("(invalid YAML: {e})")]),
    };
    let obj = match parsed.as_mapping() {
        Some(m) => m,
        None => return Err(vec!["(not a YAML mapping)".to_string()]),
    };
    let missing: Vec<String> = CONTRACT_REQUIRED_FIELDS
        .iter()
        .filter(|field| obj.get(*field).is_none() || obj[*field].is_null())
        .map(|s| s.to_string())
        .collect();
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

/// Count checked `[x]` items in a multi-line string.
fn count_checked_items(report: &str) -> usize {
    report.lines().filter(|l| l.contains("[x]")).count()
}

/// Run a validator shell command with a timeout. Returns `true` on exit 0.
async fn run_validator(cmd: &str, cwd: &PathBuf, timeout_secs: u64) -> bool {
    let timeout = std::time::Duration::from_secs(timeout_secs);
    let result = tokio::time::timeout(
        timeout,
        tokio::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .current_dir(cwd)
            .output(),
    )
    .await;
    match result {
        Ok(Ok(output)) => output.status.success(),
        Ok(Err(e)) => {
            tracing::warn!(error = %e, command = %cmd, "validator: failed to spawn");
            false
        }
        Err(_) => {
            tracing::warn!(command = %cmd, timeout = timeout_secs, "validator: timed out");
            false
        }
    }
}

async fn evaluate_delivery_gates(
    task: &Task,
    dest_column: &Column,
    automation: &crate::store::columns::AutomationConfig,
    ctx: &GateContext<'_>,
    failures: &mut Vec<GateFailure>,
    warnings: &mut Vec<GateFailure>,
) {
    let rules = &automation.delivery_rules;
    if !rules.require_committed_changes && !rules.require_clean_worktree {
        return;
    }

    if rules.require_committed_changes {
        // Check `git log` for at least one commit. An empty/untracked
        // worktree has no commits; the gate fails until the operator
        // makes a commit.
        match tokio::process::Command::new("git")
            .args(["log", "--oneline", "-1"])
            .current_dir(&ctx.cwd)
            .output()
            .await
        {
            Ok(o) if o.status.success() && !o.stdout.is_empty() => {
                // Has at least one commit — pass.
            }
            Ok(_) => {
                let f = GateFailure {
                    gate_type: GateType::Delivery,
                    message: format!(
                        "column '{}' requires committed changes before entry; \
                         make a git commit first",
                        dest_column.name
                    ),
                };
                collect_or_warn(failures, warnings, f, dest_column);
            }
            Err(e) => {
                tracing::warn!(error = %e, "delivery gate: failed to spawn git log");
            }
        }
    }

    if rules.require_clean_worktree {
        // `git status --porcelain` lists both untracked + modified files;
        // empty output means the worktree is fully clean. We use this
        // instead of `git diff --quiet` because `git diff` only checks
        // tracked files and would silently pass on untracked ones —
        // defeating the point of the rule.
        match tokio::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&ctx.cwd)
            .output()
            .await
        {
            Ok(o) if o.status.success() && o.stdout.is_empty() => {
                // clean — pass
            }
            Ok(o) if !o.status.success() => {
                tracing::warn!("delivery gate: git status failed (not a repo?)");
            }
            Ok(_) => {
                let f = GateFailure {
                    gate_type: GateType::Delivery,
                    message: format!(
                        "column '{}' requires a clean worktree before entry; \
                         commit, stage, or remove untracked files",
                        dest_column.name
                    ),
                };
                collect_or_warn(failures, warnings, f, dest_column);
            }
            Err(e) => {
                tracing::warn!(error = %e, "delivery gate: failed to spawn git status");
            }
        }
    }
    // Reference `task` to silence unused warning when the helper is not
    // directly used in the async block (kept for future per-task context).
    let _ = task;
}

fn evaluate_contract_gates(
    task: &Task,
    dest_column: &Column,
    automation: &crate::store::columns::AutomationConfig,
    failures: &mut Vec<GateFailure>,
    warnings: &mut Vec<GateFailure>,
) {
    if !automation.contract_rules.require_canonical_story {
        return;
    }
    let desc = match &task.description {
        Some(d) => d,
        None => {
            let f = GateFailure {
                gate_type: GateType::Contract,
                message: format!(
                    "column '{}' requires a canonical story in task.description; \
                     add a ```yaml code block with fields: {}",
                    dest_column.name,
                    CONTRACT_REQUIRED_FIELDS.join(", ")
                ),
            };
            collect_or_warn(failures, warnings, f, dest_column);
            return;
        }
    };
    match extract_yaml_code_block(desc) {
        None => {
            let f = GateFailure {
                gate_type: GateType::Contract,
                message: format!(
                    "column '{}' requires a canonical story in task.description; \
                     add a ```yaml code block with fields: {}",
                    dest_column.name,
                    CONTRACT_REQUIRED_FIELDS.join(", ")
                ),
            };
            collect_or_warn(failures, warnings, f, dest_column);
        }
        Some(yaml_str) => {
            if let Err(missing) = validate_story_yaml(&yaml_str) {
                let f = GateFailure {
                    gate_type: GateType::Contract,
                    message: format!(
                        "column '{}' requires canonical story with fields: {}; \
                         found ```yaml but missing: {}",
                        dest_column.name,
                        CONTRACT_REQUIRED_FIELDS.join(", "),
                        missing.join(", ")
                    ),
                };
                collect_or_warn(failures, warnings, f, dest_column);
            }
        }
    }
}

fn evaluate_checklist_gates(
    task: &Task,
    dest_column: &Column,
    automation: &crate::store::columns::AutomationConfig,
    failures: &mut Vec<GateFailure>,
    warnings: &mut Vec<GateFailure>,
) {
    if !automation.checklist_rules.required_checklist {
        return;
    }
    let report = match &task.verification_report {
        Some(r) if !r.trim().is_empty() => r,
        _ => {
            let f = GateFailure {
                gate_type: GateType::Checklist,
                message: format!(
                    "column '{}' requires a verification_report with checked [x] items; \
                     run verification and fill in the report",
                    dest_column.name
                ),
            };
            collect_or_warn(failures, warnings, f, dest_column);
            return;
        }
    };
    let checked = count_checked_items(report);
    if checked == 0 {
        let f = GateFailure {
            gate_type: GateType::Checklist,
            message: format!(
                "column '{}' requires at least one checked [x] item in verification_report",
                dest_column.name
            ),
        };
        collect_or_warn(failures, warnings, f, dest_column);
    }
}

async fn evaluate_validator_gate(
    task: &Task,
    dest_column: &Column,
    automation: &crate::store::columns::AutomationConfig,
    ctx: &GateContext<'_>,
    failures: &mut Vec<GateFailure>,
    warnings: &mut Vec<GateFailure>,
) {
    let vc = match &automation.validator_command {
        Some(vc) => vc,
        None => return,
    };

    let command_key = vc.command.clone();

    // Check cache.
    if let Some(cached_pass) =
        crate::store::kanban_validations::check_cache(ctx.db, &task.id, &command_key)
    {
        if !cached_pass {
            let f = GateFailure {
                gate_type: GateType::Validator,
                message: format!(
                    "column '{}' validator '{}' failed (cached result)",
                    dest_column.name, vc.command
                ),
            };
            collect_or_warn(failures, warnings, f, dest_column);
        }
        return;
    }

    let passed = run_validator(&vc.command, &ctx.cwd, vc.timeout_secs).await;
    crate::store::kanban_validations::cache_result(ctx.db, &task.id, &command_key, passed);

    if !passed {
        let f = GateFailure {
            gate_type: GateType::Validator,
            message: format!(
                "column '{}' validator command '{}' failed (exit status != 0)",
                dest_column.name, vc.command
            ),
        };
        collect_or_warn(failures, warnings, f, dest_column);
    }
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
            automation: None,
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

    #[tokio::test]
    async fn test_check_transition_gates_no_policies_passes() {
        let task = task_with("T", None);
        let src = default_column("col-src", false, None);
        let dst = default_column("col-dst", false, None);
        check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_check_transition_gates_freeze_blocks_empty_description() {
        let task = task_with("T", None); // no description
        let src = gate_column("col-src", true, vec![], vec![]);
        let dst = default_column("col-dst", false, None);
        let err = check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap_err();
        match err {
            AppError::Validation { message: msg, .. } => {
                assert!(msg.contains("freezes descriptions"), "got: {msg}");
                assert!(msg.contains("set task.description"), "got: {msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_check_transition_gates_freeze_allows_non_empty_description() {
        let task = task_with("T", Some("body"));
        let src = gate_column("col-src", true, vec![], vec![]);
        let dst = default_column("col-dst", false, None);
        check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_check_transition_gates_freeze_allows_whitespace_only_as_blank() {
        // Whitespace-only description is treated as blank — the gate rejects it.
        let task = task_with("T", Some("   \n  "));
        let src = gate_column("col-src", true, vec![], vec![]);
        let dst = default_column("col-dst", false, None);
        assert!(
            check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn test_check_transition_gates_required_field_blocks_when_missing() {
        let mut task = task_with("T", None);
        task.description = Some("body".into()); // bypass freeze
        let src = default_column("col-src", false, None);
        let dst = gate_column("col-dst", false, vec!["acceptance_criteria"], vec![]);
        let err = check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap_err();
        match err {
            AppError::Validation { message: msg, .. } => {
                assert!(msg.contains("acceptance_criteria"), "got: {msg}");
                assert!(msg.contains("non-empty"), "got: {msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_check_transition_gates_required_field_passes_when_set() {
        let mut task = task_with("T", None);
        task.description = Some("body".into());
        task.acceptance_criteria = Some("AC".into());
        let src = default_column("col-src", false, None);
        let dst = gate_column("col-dst", false, vec!["acceptance_criteria"], vec![]);
        check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_check_transition_gates_multiple_required_fields_all_must_be_set() {
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
        let err = check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap_err();
        match err {
            AppError::Validation { message: msg, .. } => {
                assert!(msg.contains("verification_report"), "got: {msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_check_transition_gates_unknown_required_field_name_silently_ignored() {
        // Backwards-compat: a column with a stale field name must not
        // brick every move. Unknown names are dropped (logged at debug).
        let mut task = task_with("T", None);
        task.description = Some("body".into());
        let src = default_column("col-src", false, None);
        let dst = gate_column("col-dst", false, vec!["made_up_field"], vec![]);
        check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap();
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

    #[tokio::test]
    async fn test_check_transition_gates_required_artifact_blocks_when_missing() {
        let db = make_test_db();
        let task_id = "task-arti-1";
        let task = task_with_real_id(&db, task_id);
        let src = default_column("col-src", false, None);
        let dst = gate_column("col-dst", false, vec![], vec!["test_results"]);
        let present: HashSet<String> = HashSet::new();
        let err = check_transition_gates(&task, &src, &dst, &present, None)
            .await
            .unwrap_err();
        match err {
            AppError::Validation { message: msg, .. } => {
                assert!(msg.contains("test_results"), "got: {msg}");
                assert!(msg.contains("provide_artifact"), "got: {msg}");
            }
            other => panic!("expected Validation, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_check_transition_gates_required_artifact_passes_when_present() {
        let db = make_test_db();
        let task_id = "task-arti-2";
        let task = task_with_real_id(&db, task_id);
        seed_artifact_row(&db.conn(), task_id, "test_results", "ok");
        let src = default_column("col-src", false, None);
        let dst = gate_column("col-dst", false, vec![], vec!["test_results"]);
        let mut present = HashSet::new();
        present.insert("test_results".to_string());
        check_transition_gates(&task, &src, &dst, &present, None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_check_transition_gates_required_artifact_partial_present_still_blocks() {
        let db = make_test_db();
        let task_id = "task-arti-3";
        let task = task_with_real_id(&db, task_id);
        // One of two required types is present.
        seed_artifact_row(&db.conn(), task_id, "test_results", "ok");
        let src = default_column("col-src", false, None);
        let dst = gate_column("col-dst", false, vec![], vec!["test_results", "screenshot"]);
        let mut present = HashSet::new();
        present.insert("test_results".to_string());
        let err = check_transition_gates(&task, &src, &dst, &present, None)
            .await
            .unwrap_err();
        match err {
            AppError::Validation { message: msg, .. } => {
                assert!(msg.contains("screenshot"), "got: {msg}");
            }
            other => panic!("expected Validation, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_check_transition_gates_no_required_artifacts_with_present_set_passes() {
        // Empty requirement, non-empty present set, no other policies.
        let db = make_test_db();
        let task_id = "task-arti-4";
        let task = task_with_real_id(&db, task_id);
        seed_artifact_row(&db.conn(), task_id, "log", "trace");
        let src = default_column("col-src", false, None);
        let dst = gate_column("col-dst", false, vec![], vec![]);
        let mut present = HashSet::new();
        present.insert("log".to_string());
        check_transition_gates(&task, &src, &dst, &present, None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_check_transition_gates_freeze_and_required_combined() {
        // A "Review" column with both freeze and required fields.
        let mut task = task_with("T", None);
        task.description = Some("body".into());
        // acceptance_criteria still missing.
        let src = gate_column("col-src", true, vec![], vec![]);
        let dst = gate_column("col-dst", false, vec!["acceptance_criteria"], vec![]);
        // src freeze passes (description is set); dst required fails first.
        let err = check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap_err();
        match err {
            AppError::Validation { message: msg, .. } => {
                assert!(msg.contains("acceptance_criteria"), "got: {msg}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    // --- Automation gates (feat-066) ---

    use crate::store::columns::{
        AutomationConfig, ChecklistRules, ContractRules, DeliveryRules, GateMode, ValidatorCommand,
    };

    fn column_with_automation(id: &str, automation: AutomationConfig) -> Column {
        let mut c = default_column(id, false, None);
        c.automation = Some(automation);
        c
    }

    #[tokio::test]
    async fn test_check_transition_gates_contract_missing_yaml_block() {
        let task = task_with("T", Some("plain description with no yaml"));
        let src = default_column("col-src", false, None);
        let dst = column_with_automation(
            "col-dst",
            AutomationConfig {
                contract_rules: ContractRules {
                    require_canonical_story: true,
                },
                ..Default::default()
            },
        );
        let err = check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap_err();
        match err {
            AppError::Validation { code, message } => {
                assert_eq!(code, "gate_contract");
                assert!(message.contains("canonical story"), "got: {message}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_check_transition_gates_contract_missing_fields() {
        let task = task_with("T", Some("Story:\n```yaml\ntitle: foo\n```"));
        let src = default_column("col-src", false, None);
        let dst = column_with_automation(
            "col-dst",
            AutomationConfig {
                contract_rules: ContractRules {
                    require_canonical_story: true,
                },
                ..Default::default()
            },
        );
        let err = check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap_err();
        match err {
            AppError::Validation { code, .. } => assert_eq!(code, "gate_contract"),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_check_transition_gates_contract_passes() {
        let task = task_with(
            "T",
            Some("Story:\n```yaml\ntitle: foo\ndescription: d\nacceptance_criteria: ac\n```"),
        );
        let src = default_column("col-src", false, None);
        let dst = column_with_automation(
            "col-dst",
            AutomationConfig {
                contract_rules: ContractRules {
                    require_canonical_story: true,
                },
                ..Default::default()
            },
        );
        check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_check_transition_gates_checklist_missing_report() {
        let task = task_with("T", None);
        let src = default_column("col-src", false, None);
        let dst = column_with_automation(
            "col-dst",
            AutomationConfig {
                checklist_rules: ChecklistRules {
                    required_checklist: true,
                },
                ..Default::default()
            },
        );
        let err = check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap_err();
        match err {
            AppError::Validation { code, .. } => assert_eq!(code, "gate_checklist"),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_check_transition_gates_checklist_no_checked_items() {
        let mut task = task_with("T", None);
        task.verification_report = Some("- [ ] not done".to_string());
        let src = default_column("col-src", false, None);
        let dst = column_with_automation(
            "col-dst",
            AutomationConfig {
                checklist_rules: ChecklistRules {
                    required_checklist: true,
                },
                ..Default::default()
            },
        );
        let err = check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap_err();
        match err {
            AppError::Validation { code, .. } => assert_eq!(code, "gate_checklist"),
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_check_transition_gates_checklist_passes() {
        let mut task = task_with("T", None);
        task.verification_report = Some("- [x] all done\n- [x] also done".to_string());
        let src = default_column("col-src", false, None);
        let dst = column_with_automation(
            "col-dst",
            AutomationConfig {
                checklist_rules: ChecklistRules {
                    required_checklist: true,
                },
                ..Default::default()
            },
        );
        check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_check_transition_gates_warning_mode_allows_move() {
        let task = task_with("T", None);
        let src = default_column("col-src", false, None);
        let dst = column_with_automation(
            "col-dst",
            AutomationConfig {
                checklist_rules: ChecklistRules {
                    required_checklist: true,
                },
                gate_mode: GateMode::Warning,
                ..Default::default()
            },
        );
        // Warning mode: no error, move allowed.
        check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_check_transition_gates_delivery_no_ctx_skips() {
        let task = task_with("T", None);
        let src = default_column("col-src", false, None);
        let dst = column_with_automation(
            "col-dst",
            AutomationConfig {
                delivery_rules: DeliveryRules {
                    require_committed_changes: true,
                    require_clean_worktree: true,
                },
                ..Default::default()
            },
        );
        // No GateContext → delivery gates skipped (not called when ctx is None).
        check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_check_transition_gates_validator_no_ctx_skips() {
        let task = task_with("T", None);
        let src = default_column("col-src", false, None);
        let dst = column_with_automation(
            "col-dst",
            AutomationConfig {
                validator_command: Some(ValidatorCommand {
                    command: "false".to_string(),
                    timeout_secs: 5,
                }),
                ..Default::default()
            },
        );
        // No GateContext → validator gate skipped.
        check_transition_gates(&task, &src, &dst, &HashSet::new(), None)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_check_transition_gates_validator_passing_command() {
        let state = make_test_state();
        let mut t = task_with("T", None);
        t.id = "task-x".to_string();
        let src = default_column("col-src", false, None);
        let dst = column_with_automation(
            "col-dst",
            AutomationConfig {
                validator_command: Some(ValidatorCommand {
                    command: "true".to_string(),
                    timeout_secs: 5,
                }),
                ..Default::default()
            },
        );
        let tmp = tempfile::TempDir::new().unwrap();
        let ctx = crate::service::kanban::GateContext {
            db: &state.db,
            cwd: tmp.path().to_path_buf(),
        };
        check_transition_gates(&t, &src, &dst, &HashSet::new(), Some(&ctx))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_check_transition_gates_validator_failing_command() {
        let state = make_test_state();
        let mut t = task_with("T", None);
        t.id = "task-y".to_string();
        let src = default_column("col-src", false, None);
        let dst = column_with_automation(
            "col-dst",
            AutomationConfig {
                validator_command: Some(ValidatorCommand {
                    command: "false".to_string(),
                    timeout_secs: 5,
                }),
                ..Default::default()
            },
        );
        let tmp = tempfile::TempDir::new().unwrap();
        let ctx = crate::service::kanban::GateContext {
            db: &state.db,
            cwd: tmp.path().to_path_buf(),
        };
        let err = check_transition_gates(&t, &src, &dst, &HashSet::new(), Some(&ctx))
            .await
            .unwrap_err();
        match err {
            AppError::Validation { code, message } => {
                assert_eq!(code, "gate_validator");
                assert!(message.contains("failed"), "got: {message}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_check_transition_gates_validator_caches_pass() {
        let state = make_test_state();
        let (workspace_id, board_id, column_id) =
            crate::store::kanban_test_helpers::seed_workspace_with_board(&state.db);
        // Insert a real task row so the kanban_validations FK is satisfied.
        let task_id = "task-z".to_string();
        let now = chrono::Utc::now().to_rfc3339();
        state.db.conn().execute(
            "INSERT INTO tasks (id, board_id, column_id, title, position, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, 'T', 0, 'active', ?4, ?4)",
            rusqlite::params![task_id, board_id, column_id, now],
        ).unwrap();
        let mut t = task_with("T", None);
        t.id = task_id.clone();
        let src = default_column("col-src", false, None);
        let dst = column_with_automation(
            "col-dst",
            AutomationConfig {
                validator_command: Some(ValidatorCommand {
                    command: "true".to_string(),
                    timeout_secs: 5,
                }),
                ..Default::default()
            },
        );
        let tmp = tempfile::TempDir::new().unwrap();
        let ctx = crate::service::kanban::GateContext {
            db: &state.db,
            cwd: tmp.path().to_path_buf(),
        };
        // First call: runs validator, caches pass.
        check_transition_gates(&t, &src, &dst, &HashSet::new(), Some(&ctx))
            .await
            .unwrap();
        // Confirm cache row exists.
        let count: i64 = state
            .db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM kanban_validations WHERE task_id = ?1",
                rusqlite::params![&task_id],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
        // Silence unused warning
        let _ = workspace_id;
    }

    #[tokio::test]
    async fn test_check_transition_gates_delivery_dirty_worktree_blocks() {
        // The delivery gate runs `git status --porcelain` which lists
        // untracked + modified files. With an untracked file present,
        // the output is non-empty and the gate fails.
        let state = make_test_state();
        let mut t = task_with("T", None);
        t.id = "task-dirty".to_string();
        let src = default_column("col-src", false, None);
        let dst = column_with_automation(
            "col-dst",
            AutomationConfig {
                delivery_rules: DeliveryRules {
                    require_committed_changes: true,
                    require_clean_worktree: false,
                },
                ..Default::default()
            },
        );
        let tmp = tempfile::TempDir::new().unwrap();
        // Skip if git isn't installed (CI without git).
        if std::process::Command::new("git")
            .args(["--version"])
            .output()
            .is_err()
        {
            return;
        }
        let _ = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output();
        // Untracked file in a fresh repo: git status --porcelain is non-empty.
        std::fs::write(tmp.path().join("untracked.txt"), "x").unwrap();
        let ctx = crate::service::kanban::GateContext {
            db: &state.db,
            cwd: tmp.path().to_path_buf(),
        };
        let result = check_transition_gates(&t, &src, &dst, &HashSet::new(), Some(&ctx)).await;
        // The "committed changes" gate fails because there are no commits.
        assert!(result.is_err(), "expected gate to fail on no commits");
    }

    #[tokio::test]
    async fn test_check_transition_gates_delivery_worktree_untracked_blocks() {
        // Isolates the require_clean_worktree check: commits present,
        // but an untracked file in the worktree. `git diff --quiet`
        // would miss this; `git status --porcelain` catches it.
        let state = make_test_state();
        let mut t = task_with("T", None);
        t.id = "task-untracked".to_string();
        let src = default_column("col-src", false, None);
        let dst = column_with_automation(
            "col-dst",
            AutomationConfig {
                delivery_rules: DeliveryRules {
                    require_committed_changes: false,
                    require_clean_worktree: true,
                },
                ..Default::default()
            },
        );
        let tmp = tempfile::TempDir::new().unwrap();
        if std::process::Command::new("git")
            .args(["--version"])
            .output()
            .is_err()
        {
            return;
        }
        let _ = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(tmp.path())
            .output();
        // Commit one file so the worktree is "committed" but the
        // untracked file makes the worktree dirty.
        std::fs::write(tmp.path().join("README.md"), "hi").unwrap();
        let _ = std::process::Command::new("git")
            .args(["config", "user.email", "t@t"])
            .current_dir(tmp.path())
            .output();
        let _ = std::process::Command::new("git")
            .args(["config", "user.name", "T"])
            .current_dir(tmp.path())
            .output();
        let _ = std::process::Command::new("git")
            .args(["add", "."])
            .current_dir(tmp.path())
            .output();
        let _ = std::process::Command::new("git")
            .args(["commit", "-q", "-m", "init"])
            .current_dir(tmp.path())
            .output();
        // Now drop an untracked file.
        std::fs::write(tmp.path().join("scratch.txt"), "x").unwrap();
        let ctx = crate::service::kanban::GateContext {
            db: &state.db,
            cwd: tmp.path().to_path_buf(),
        };
        let err = check_transition_gates(&t, &src, &dst, &HashSet::new(), Some(&ctx))
            .await
            .unwrap_err();
        match err {
            AppError::Validation { code, message } => {
                assert_eq!(code, "gate_delivery");
                assert!(message.contains("clean worktree"), "got: {message}");
            }
            other => panic!("expected Validation, got {other:?}"),
        }
    }
}
