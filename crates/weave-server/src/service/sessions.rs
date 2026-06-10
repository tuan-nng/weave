use std::collections::HashSet;
use std::str::FromStr;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::agent::registry::ProviderRegistry;
#[allow(unused_imports)] // MessageRequest is used by the `tests` submodule.
use crate::agent::{self, MessageRequest, RuntimeKind, SessionMode};
use crate::db::Db;
use crate::error::AppError;
use crate::specialist::{Specialist, SpecialistRegistry};
use crate::sse::{self, SseWireEvent};
use crate::store::providers::ProviderStore;
use crate::store::sessions::{MessageStore, SessionStore, MAX_RESUME_DEPTH, TERMINAL};
use crate::store::traces::{TraceEvent, TraceEventKind};
use crate::tools::ToolRegistry;
use crate::trace;

use super::ActiveSessions;

/// Default max tokens for agent responses.
const DEFAULT_MAX_TOKENS: u32 = 4096;

/// Hardcoded fallback model when neither session nor provider specifies one.
const FALLBACK_MODEL: &str = "claude-sonnet-4-20250514";

/// Maximum number of messages loaded into conversation history.
const MAX_HISTORY_MESSAGES: usize = 1000;

/// Maximum number of tool-execution iterations per turn (feat-037).
///
/// After this many `tool_use` round-trips without a natural `end_turn` /
/// `max_tokens` / `cancelled` we stop the loop, surface a final assistant
/// "Sorry, too many tool calls." message, and persist the turn with
/// `stop_reason = loop_limit`. The cap exists because tool-capable models
/// can otherwise chase an unsatisfiable request indefinitely (e.g. trying
/// the same broken shell command 200 times in a row) and burn through
/// the user's token budget. The default is deliberately conservative —
/// most well-formed agent loops complete in 2-4 iterations.
const MAX_TOOL_ITERATIONS: u32 = 8;

/// Per-tool execution timeout (feat-037).
///
/// Mirrors `shell_exec::DEFAULT_TIMEOUT_MS` (30s). A tool that takes
/// longer than this is aborted and the loop records an `is_error=true`
/// `tool_result` so the model can react. Cancellable via the session's
/// `CancellationToken` (`tokio::select!` arms the timeout and the cancel
/// token together).
const TOOL_EXECUTION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Stateless service that orchestrates session prompt lifecycle.
///
/// Validates session state, persists messages, spawns streaming tasks,
/// and supports cancellation. Mirrors the store pattern (unit struct,
/// static methods) but operates at a higher abstraction level.
pub struct SessionService;

impl SessionService {
    /// Create a session, optionally resuming from a parent session.
    ///
    /// When `parent_session_id` is set, validates the parent chain (up to
    /// `MAX_RESUME_DEPTH`), then atomically creates the new session and copies
    /// all messages from the direct parent into it within a single transaction.
    ///
    /// Only the direct parent's messages are copied — if the parent was itself
    /// resumed, it already contains its ancestors' messages.
    ///
    /// `codebase_id` is resolved against the workspace: it must reference an
    /// existing codebase in the same workspace. When supplied, the codebase's
    /// `path` is copied onto the session's `cwd` so the agent's shell/git
    /// tools default to operating inside the registered repo. The caller may
    /// also pass `cwd` directly (advanced: pointing at a subdir); the two
    /// fields compose, but `codebase_id` wins when both are present (the
    /// resolved path from the codebase overrides any supplied `cwd`).
    ///
    /// `runtime_kind` / `mode` / `runtime_metadata_json` (feat-038) take
    /// the same shape: a per-call override or a default. When resuming
    /// from a parent, `runtime_kind` and `mode` inherit from the parent
    /// unless the caller passes a non-`None` override. `runtime_metadata_json`
    /// is the rule's exception: it is inherited only when the resolved
    /// `runtime_kind` matches the parent's — a CLI resume id from a
    /// `claude-code` parent is meaningless when the child runs on
    /// `anthropic-api`, so it is cleared on a runtime switch (matching
    /// feat-047's "cli_resume_id is NOT inherited when runtime_kind
    /// changes" rule).
    #[allow(clippy::too_many_arguments)]
    pub fn create_session(
        db: &Db,
        workspace_id: &str,
        provider_id: &str,
        specialist_id: Option<&str>,
        model: Option<&str>,
        cwd: Option<&str>,
        parent_session_id: Option<&str>,
        context_id: Option<&str>,
        codebase_id: Option<&str>,
        runtime_kind: Option<&str>,
        mode: Option<&str>,
        runtime_metadata_json: Option<&str>,
    ) -> Result<crate::store::sessions::Session, AppError> {
        // Validate workspace exists
        crate::store::workspaces::WorkspaceStore::get_by_id(db, workspace_id)?;

        // Resolve codebase_id → path. The FK has ON DELETE SET NULL but we
        // also want to reject cross-workspace references up front (the FK
        // would technically permit a different workspace's codebase, but the
        // design contract is workspace-scoped). Look the row up explicitly
        // so the error message is actionable.
        let resolved_cwd: Option<String> = if let Some(cid) = codebase_id {
            let codebase =
                crate::store::codebases::CodebaseStore::get_in_workspace(db, cid, workspace_id)?;
            // Codebase binding wins — the agent's working directory is the
            // registered repo root, regardless of any supplied `cwd`.
            Some(codebase.path.clone())
        } else {
            cwd.map(|s| s.to_string())
        };

        // Validate the three runtime fields up front so a bad value is
        // rejected before we touch the parent chain or start a transaction.
        // Parsing `None` as the typed default means a missing field is
        // indistinguishable from "use the platform default" — the same
        // shape the pre-feat-038 API had for `model` / `cwd` / etc.
        let runtime_kind: RuntimeKind = parse_runtime_kind(runtime_kind)?;
        let mode: SessionMode = parse_mode(mode)?;

        // feat-040: enforce the runtime_kind × mode compatibility matrix
        // before any parent-chain or transaction work. Runs on the
        // caller's resolved pair (defaults already filled in by
        // `parse_runtime_kind` / `parse_mode`); `resume_inherit` only
        // adjusts `runtime_metadata_json`, so the runtime/mode we
        // validate here is the same one that gets persisted.
        crate::agent::validate_runtime_mode_compat(runtime_kind, mode)?;

        // Validate parent chain and load direct parent's messages + runtime
        let (parent_messages, parent) = if let Some(pid) = parent_session_id {
            // Ensure parent has finished — resuming an active session would
            // copy an incomplete message history.
            let parent = SessionStore::get_by_id(db, pid)?;
            if !TERMINAL.contains(&parent.status.as_str()) {
                return Err(AppError::validation(format!(
                    "cannot resume from session in '{}' status — parent must be completed, \
                     cancelled, or error",
                    parent.status
                )));
            }
            validate_parent_chain(db, pid, workspace_id)?;
            let messages = MessageStore::load_all(db, pid, MAX_HISTORY_MESSAGES)?;
            (messages, Some(parent))
        } else {
            (Vec::new(), None)
        };

        // Resume inheritance. The original `runtime_kind` / `mode` from
        // the caller are still in scope; we only fall back to the parent
        // when the caller passed `None`. `runtime_metadata_json` is
        // computed from the *resolved* runtime_kind, so the
        // "inherit only on same-runtime resume" rule is enforced here.
        let (resolved_runtime_kind, resolved_mode, resolved_metadata) =
            resume_inherit(runtime_kind, mode, runtime_metadata_json, parent.as_ref());

        // Atomically create session + copy messages
        db.with_transaction(|conn| {
            let session = SessionStore::create_tx(
                conn,
                workspace_id,
                provider_id,
                specialist_id,
                model,
                resolved_cwd.as_deref(),
                parent_session_id,
                context_id,
                codebase_id,
                resolved_runtime_kind,
                resolved_mode,
                resolved_metadata.as_deref(),
            )?;

            if !parent_messages.is_empty() {
                MessageStore::copy_messages(conn, &session.id, &parent_messages)?;
            }

            Ok(session)
        })
    }

    /// Send a prompt to a session.
    ///
    /// Returns the user message ID immediately. Spawns an async task that
    /// streams the agent response, accumulates text, and saves the assistant
    /// message when complete.
    pub async fn send_prompt(
        db: &Arc<Db>,
        registry: &Arc<ProviderRegistry>,
        specialists: &Arc<SpecialistRegistry>,
        active: &Arc<ActiveSessions>,
        sse_manager: &Arc<sse::SseManager>,
        tools: &Arc<ToolRegistry>,
        session_id: &str,
        prompt: &str,
    ) -> Result<String, AppError> {
        // Validate prompt is non-empty
        if prompt.trim().is_empty() {
            return Err(AppError::validation("prompt cannot be empty"));
        }

        // Validate session exists and is in a non-terminal state
        let session = SessionStore::get_by_id(db, session_id)?;
        if crate::store::sessions::TERMINAL.contains(&session.status.as_str()) {
            return Err(AppError::validation(format!(
                "cannot send prompt to session in '{}' status",
                session.status
            )));
        }

        // Validate tool profile early (fail-fast before spawning task)
        if let Some(ref specialist_id) = session.specialist_id {
            if let Some(specialist) = specialists.get_by_name(specialist_id) {
                if let Some(ref profile) = specialist.tool_profile {
                    tools.validate_profile_name(profile)?;
                }
            }
        }

        // Atomically check-and-insert to prevent TOCTOU race
        let cancel_token = CancellationToken::new();
        if !active.try_insert(session_id.to_string(), cancel_token.clone()) {
            return Err(AppError::Conflict(
                "session already has an active prompt".into(),
            ));
        }

        // Save user message (raw text, no JSON encoding)
        let user_msg = MessageStore::create(db, session_id, "user", prompt, None)?;

        // Spawn the streaming task
        let task_db = Arc::clone(db);
        let task_registry = Arc::clone(registry);
        let task_specialists = Arc::clone(specialists);
        let task_active = Arc::clone(active);
        let task_sse = Arc::clone(sse_manager);
        let task_tools = Arc::clone(tools);
        let task_session = session;

        tokio::spawn(async move {
            run_prompt_task(
                task_db,
                task_registry,
                task_specialists,
                task_active,
                task_sse,
                task_tools,
                task_session,
                cancel_token,
            )
            .await;
        });

        Ok(user_msg.id)
    }

    /// Cancel an active session's streaming task.
    pub fn cancel_session(active: &Arc<ActiveSessions>, session_id: &str) -> Result<(), AppError> {
        match active.get(session_id) {
            Some(token) => {
                token.cancel();
                Ok(())
            }
            None => Err(AppError::validation("session is not actively streaming")),
        }
    }

    /// Convert stored messages into the agent's `Message` format.
    ///
    /// For feat-009, content is always treated as `Content::Text` since
    /// tool execution (structured blocks) is not yet active.
    fn build_message_history(messages: &[crate::store::sessions::Message]) -> Vec<agent::Message> {
        messages
            .iter()
            .map(|m| {
                let role = match m.role.as_str() {
                    "user" => agent::Role::User,
                    "assistant" => agent::Role::Assistant,
                    // Treat unknown roles as user (shouldn't happen in practice)
                    _ => agent::Role::User,
                };
                agent::Message {
                    role,
                    content: agent::Content::Text(m.content.clone()),
                }
            })
            .collect()
    }

    /// Resolve the model to use for an agent request.
    ///
    /// Priority: session.model → specialist.model → provider default_model → hardcoded fallback.
    fn resolve_model(
        session: &crate::store::sessions::Session,
        specialist: Option<&Specialist>,
        db: &Db,
    ) -> String {
        // 1. Session-level override
        if let Some(ref model) = session.model {
            if !model.is_empty() {
                return model.clone();
            }
        }

        // 2. Specialist-level override
        if let Some(s) = specialist {
            if let Some(ref model) = s.model {
                if !model.is_empty() {
                    return model.clone();
                }
            }
        }

        // 3. Provider default_model from config_json
        match ProviderStore::get_by_id(db, &session.provider_id) {
            Ok(provider) => {
                if let Ok(config) = serde_json::from_str::<serde_json::Value>(&provider.config_json)
                {
                    if let Some(model) = config.get("default_model").and_then(|v| v.as_str()) {
                        return model.to_string();
                    }
                }
            }
            Err(e) => {
                warn!(
                    error = %e,
                    "failed to load provider for model resolution, using fallback"
                );
            }
        }

        // 4. Hardcoded fallback
        FALLBACK_MODEL.to_string()
    }
}

/// Validate the parent session chain from `start_parent_id`.
///
/// Walks the chain up to `MAX_RESUME_DEPTH` hops, validating that each session
/// exists, belongs to `workspace_id`, and the chain has no cycles.
fn validate_parent_chain(
    db: &Db,
    start_parent_id: &str,
    workspace_id: &str,
) -> Result<(), AppError> {
    let mut current_id = start_parent_id.to_string();
    let mut seen = HashSet::new();
    seen.insert(start_parent_id.to_string());
    let mut depth = 0usize;

    loop {
        let session = SessionStore::get_by_id(db, &current_id)?;

        // Validate workspace ownership
        if session.workspace_id != workspace_id {
            return Err(AppError::validation(
                "parent session belongs to a different workspace",
            ));
        }

        // Walk to parent if present
        if let Some(ref parent_id) = session.parent_session_id {
            depth += 1;
            if depth >= MAX_RESUME_DEPTH {
                return Err(AppError::validation(format!(
                    "session resume chain exceeds maximum depth of {}",
                    MAX_RESUME_DEPTH
                )));
            }
            if seen.contains(parent_id) {
                return Err(AppError::validation(
                    "cycle detected in parent_session_id chain",
                ));
            }
            seen.insert(parent_id.clone());
            current_id = parent_id.clone();
        } else {
            // Reached root of chain — valid
            break;
        }
    }

    Ok(())
}

/// Parse a caller-supplied `runtime_kind` (already validated by the API
/// layer when the request is JSON, but accepting `Option<&str>` keeps
/// the boundary consistent with the legacy `model` / `cwd` arguments).
///
/// `None` is treated as "use the platform default" — the same shape
/// the rest of `create_session` uses. An unparseable value is a 400.
pub(crate) fn parse_runtime_kind(s: Option<&str>) -> Result<RuntimeKind, AppError> {
    match s {
        None => Ok(RuntimeKind::default()),
        Some(s) => RuntimeKind::from_str(s),
    }
}

/// Parse a caller-supplied `mode`. Same semantics as [`parse_runtime_kind`].
pub(crate) fn parse_mode(s: Option<&str>) -> Result<SessionMode, AppError> {
    match s {
        None => Ok(SessionMode::default()),
        Some(s) => SessionMode::from_str(s),
    }
}

/// Compute the resolved `(runtime_kind, mode, runtime_metadata_json)` for
/// the new session, applying the resume-inheritance rule (feat-038).
///
/// Rule: the child inherits `runtime_kind` and `mode` from the parent
/// when the caller does not pass a non-`None` override. `runtime_metadata_json`
/// is the special case: it is inherited only when the *resolved* `runtime_kind`
/// matches the parent's. A CLI resume id (e.g. from a `claude-code` parent)
/// is meaningless when the child runs on a different runtime, so the
/// metadata is cleared on a runtime switch. An explicit caller override
/// of `runtime_metadata_json` always wins (the caller is asserting "I
/// know what I'm doing — use this id verbatim").
///
/// `parent` is `None` for non-resume calls; the resolved values are
/// simply the typed defaults + the caller's metadata (if any).
pub(crate) fn resume_inherit(
    runtime_kind: RuntimeKind,
    mode: SessionMode,
    metadata: Option<&str>,
    parent: Option<&crate::store::sessions::Session>,
) -> (RuntimeKind, SessionMode, Option<String>) {
    let Some(parent) = parent else {
        // No resume — caller-supplied or default values, metadata is
        // taken verbatim from the caller.
        return (runtime_kind, mode, metadata.map(|s| s.to_string()));
    };

    let resolved_runtime = runtime_kind; // already default-resolved upstream
    let resolved_mode = mode;
    // Inherit metadata when the resolved runtime matches the parent's
    // AND the caller did not pass an explicit override. The caller's
    // override is the "I know what I'm doing" signal — it always wins.
    let resolved_metadata = match (metadata, resolved_runtime == parent.runtime_kind) {
        (Some(s), _) => Some(s.to_string()),
        (None, true) => parent.runtime_metadata_json.clone(),
        (None, false) => None,
    };

    (resolved_runtime, resolved_mode, resolved_metadata)
}

/// Log an error, transition the session to error status.
fn abort_with_error(db: &Arc<Db>, session_id: &str, e: impl std::fmt::Display, msg: &str) {
    error!(session_id, error = %e, msg);
    let _ = SessionStore::update_status(db, session_id, "error");
}

/// Build a [`TurnContext`] for the given session (feat-041).
///
/// The cwd / codebase_root derivation mirrors the canonical rule that
/// also drives `ToolContext` (above) so the runtime context and the
/// FS-tool containment boundary agree. `cli_resume_id` is parsed from
/// `runtime_metadata_json`; a malformed JSON blob is silently swallowed
/// (opportunistic metadata, not a required key — see
/// `agent::turn_context` module docs).
fn build_turn_context(
    session: &crate::store::sessions::Session,
    session_cwd: std::path::PathBuf,
    cancel_token: CancellationToken,
) -> crate::agent::turn_context::TurnContext {
    let cli_resume_id: Option<String> = session
        .runtime_metadata_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        .and_then(|v| {
            v.get("cli_resume_id")
                .and_then(|id| id.as_str())
                .map(str::to_string)
        });

    crate::agent::turn_context::TurnContext {
        session_id: session.id.clone(),
        workspace_id: session.workspace_id.clone(),
        cwd: session_cwd.clone(),
        codebase_root: if session.codebase_id.is_some() {
            Some(session_cwd)
        } else {
            None
        },
        cli_resume_id,
        runtime_kind: session.runtime_kind,
        cancellation_token: cancel_token,
    }
}

/// The spawned task that streams the agent response and persists the result.
async fn run_prompt_task(
    db: Arc<Db>,
    registry: Arc<ProviderRegistry>,
    specialists: Arc<SpecialistRegistry>,
    active: Arc<ActiveSessions>,
    sse_manager: Arc<sse::SseManager>,
    tools: Arc<ToolRegistry>,
    session: crate::store::sessions::Session,
    cancel_token: CancellationToken,
) {
    let session_id = &session.id;

    // Ensure we always clean up the active session entry
    let _guard = SessionGuard {
        active: Arc::clone(&active),
        session_id: session_id.to_string(),
    };

    // Transition connecting -> ready if needed
    if session.status == "connecting" {
        if let Err(e) = SessionStore::update_status(&db, session_id, "ready") {
            abort_with_error(&db, session_id, e, "failed to transition session to ready");
            return;
        }
    }

    // Load message history
    let messages = match MessageStore::load_all(&db, session_id, MAX_HISTORY_MESSAGES) {
        Ok(msgs) => msgs,
        Err(e) => {
            abort_with_error(&db, session_id, e, "failed to load message history");
            return;
        }
    };
    let history = SessionService::build_message_history(&messages);

    // Resolve specialist (used for both model override and system prompt)
    let specialist =
        session
            .specialist_id
            .as_deref()
            .and_then(|id| match specialists.get_by_name(id) {
                Some(s) => Some(s),
                None => {
                    warn!(
                        session_id = session_id,
                        specialist_id = id,
                        "specialist not found in registry, proceeding without system prompt"
                    );
                    None
                }
            });

    // Resolve model — priority: session → specialist → provider → fallback
    let model = SessionService::resolve_model(&session, specialist, &db);

    // Resolve tools from specialist's tool profile
    let tool_defs = if let Some(s) = specialist {
        if let Some(ref profile) = s.tool_profile {
            match tools.resolve_profile(profile) {
                Ok(defs) if defs.is_empty() => None,
                Ok(defs) => Some(defs),
                Err(e) => {
                    abort_with_error(&db, session_id, e, "invalid tool profile");
                    return;
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // System prompt from specialist
    let system_prompt = specialist.map(|s| s.system_prompt.clone());

    // Get the agent from the registry
    let agent = match registry.get_agent(&session.provider_id) {
        Ok(a) => a,
        Err(e) => {
            abort_with_error(&db, session_id, e, "failed to get agent from registry");
            return;
        }
    };

    // Spawn the trace collector flush task. The trace collector is shared
    // between the agent loop and the post-loop finalization so tool-call
    // events are queued before the flush task is awaited.
    let (trace_collector, flush_handle) = trace::spawn_flush_task(db.clone());

    // Build the ToolContext once for this turn. The same context is
    // passed to every tool invocation inside the loop, so a tool that
    // records a file change or a network call sees a consistent
    // workspace_root / cwd / trace_collector across iterations.
    //
    // `cwd` is the session's working directory (set at create time
    // from the bound codebase's path, or from an explicit `cwd` arg,
    // or unset — in which case the server's CWD is the default).
    //
    // `codebase_root` is the FS-tool containment boundary. It is set
    // to the session's `cwd` whenever the session is **bound** to a
    // registered codebase (via `codebase_id`); for unbound sessions
    // (the legacy / explicit-opt-out case), the field is set to `.`
    // and the FS read tools stay permissive.
    //
    // Note: when the bound codebase is deleted mid-session, the FK
    // `ON DELETE SET NULL` clears `codebase_id` but leaves `cwd`
    // intact. We deliberately keep the containment active in that
    // case — the agent is still operating in the same directory, so
    // the sandbox should not silently flip off. The user can recreate
    // the session to rebind.
    let session_cwd: std::path::PathBuf = session
        .cwd
        .as_deref()
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("."));

    let codebase_root: std::path::PathBuf = if session.codebase_id.is_some() {
        session_cwd.clone()
    } else {
        std::path::PathBuf::from(".")
    };

    let tool_ctx = crate::tools::ToolContext {
        session_id: session_id.to_string(),
        workspace_id: session.workspace_id.clone(),
        cwd: session_cwd.clone(),
        codebase_root,
        trace_collector: std::sync::Arc::new(trace_collector.clone()),
    };

    // Build the per-turn runtime context (feat-041). The cwd /
    // codebase_root derivation mirrors `ToolContext` above so the
    // runtime context and the FS-tool containment boundary agree. The
    // derivation is in `build_turn_context` so the test at
    // `test_session_service_passes_turn_context` exercises the same
    // code path; we keep the builder in `service::sessions` (not in
    // `agent::turn_context`) so the agent module stays free of a
    // `store::sessions::Session` upward dependency.
    let turn_ctx = build_turn_context(&session, session_cwd, cancel_token.clone());

    // Drive the model ↔ tool-execution loop. This subsumes the
    // pre-feat-037 single-stream pipeline; tool calls now flow back into
    // the conversation and the model is re-asked to continue.
    let loop_result = agent_loop(
        &agent,
        &sse_manager,
        &tools,
        &trace_collector,
        session_id,
        &cancel_token,
        model,
        system_prompt,
        DEFAULT_MAX_TOKENS,
        tool_defs,
        history,
        tool_ctx,
        &turn_ctx,
        session.runtime_kind,
        session.mode,
    )
    .await;

    let LoopResult {
        accumulated,
        stop_reason,
        tool_calls,
        had_error,
        cancelled,
    } = loop_result;

    // Flush remaining trace events. We `drop` the collector to close
    // the channel; the flush task drains it and writes the rows.
    drop(trace_collector);
    let _ = flush_handle.await;

    // Re-check cancellation after loop (race between Done and Cancel). If
    // the agent finished normally but the user clicked Cancel at almost
    // the same instant, we treat the turn as cancelled — the user
    // expressed intent before the result was visible to them.
    let cancelled = cancelled || cancel_token.is_cancelled();

    let final_status = if cancelled {
        "cancelled"
    } else if had_error {
        "error"
    } else {
        // On success the session returns to "ready" so the user can send
        // more prompts in the same session (multi-turn). The terminal
        // "completed" status is only reached via explicit close
        // (PATCH /api/sessions/:id) or future idle-timeout/cleanup work.
        "ready"
    };

    // Persist the assistant message. On cancel and on streaming error, the
    // partial streamed text is preserved with the appropriate
    // `stop_reason` encoded in the metadata JSON. The `messages.metadata`
    // column is TEXT NULL and is the natural extension point — no
    // migration required. The frontend parses the metadata on
    // `message_persisted` to render a "Cancelled" / "Error" badge on the
    // partial bubble.
    let metadata_json = build_message_metadata(&stop_reason, had_error, &tool_calls);
    let persisted_message: Option<crate::store::sessions::Message> = if !accumulated.is_empty() {
        match MessageStore::create(
            &db,
            session_id,
            "assistant",
            &accumulated,
            metadata_json.as_deref(),
        ) {
            Ok(msg) => Some(msg),
            Err(e) => {
                error!(session_id, error = %e, "failed to save assistant message");
                // Don't abort the whole task — still broadcast MessagePersisted
                // (sentinel) and Done so the frontend leaves the streaming
                // state. The session status below will reflect the error.
                None
            }
        }
    } else {
        None
    };

    if let Err(e) = SessionStore::update_status(&db, session_id, final_status) {
        error!(session_id, error = %e, "failed to update session to {}", final_status);
    }

    // Broadcast `message_persisted` *after* MessageStore::create returns
    // and *before* the terminal `done` event. This is the load-bearing
    // invariant for the frontend's id-based handoff: when the client sees
    // `message_persisted`, the row is already in the database and any
    // history refetch will find it. When the partial was empty (e.g.
    // cancel before any text streamed), we still emit the event with
    // `id == ""` so the frontend knows the live bubble is no longer the
    // latest and can collapse it cleanly.
    let now = chrono::Utc::now().to_rfc3339();
    let (
        persisted_id,
        persisted_role,
        persisted_content,
        persisted_created_at,
        persisted_stop_reason,
    ) = match &persisted_message {
        Some(msg) => (
            msg.id.clone(),
            msg.role.clone(),
            msg.content.clone(),
            msg.created_at.clone(),
            stop_reason_to_wire(&stop_reason, had_error),
        ),
        None => (
            String::new(),
            "assistant".to_string(),
            String::new(),
            now,
            stop_reason_to_wire(&stop_reason, had_error),
        ),
    };
    sse_manager.broadcast(
        session_id,
        SseWireEvent::MessagePersisted {
            id: persisted_id,
            role: persisted_role,
            stop_reason: persisted_stop_reason,
            content: persisted_content,
            created_at: persisted_created_at,
            runtime_kind: session.runtime_kind,
            mode: session.mode,
        },
    );

    // Broadcast the terminal `done` event last. The frontend's `done`
    // handler invalidates the history query, which now refetches a
    // history that contains the row we just broadcast in
    // `message_persisted` — no race, no flash.
    sse_manager.broadcast(
        session_id,
        SseWireEvent::Done {
            stop_reason,
            runtime_kind: session.runtime_kind,
            mode: session.mode,
        },
    );
}

// ---------------------------------------------------------------------------
// Agent tool-execution loop (feat-037)
// ---------------------------------------------------------------------------

/// Per-tool summary persisted on the assistant message's `metadata.tool_calls`
/// JSON. Used to render the "N tool calls" badge on the persisted bubble
/// and to give the Journey sidebar a single summary row per turn.
#[derive(Debug, Clone)]
struct ToolCallRecord {
    tool_use_id: String,
    name: String,
    is_error: bool,
    duration_ms: u64,
}

/// Outcome of running a single tool under the agent loop (feat-037).
///
/// The loop driver uses this to decide whether to continue (success or
/// recorded error), terminate the turn (cancel), or surface a tool failure
/// that the model should react to.
#[derive(Debug)]
enum ToolOutcome {
    /// The tool ran to completion. `result` is the JSON value to send back
    /// to the model in the next `tool_result` block. `duration_ms` is
    /// recorded on the trace event and the tool-call record.
    Completed {
        result: serde_json::Value,
        duration_ms: u64,
    },
    /// The tool was registered but its input failed JSON-schema validation.
    /// The error message is what the model sees as the `tool_result` content.
    ValidationFailed(String),
    /// The tool name is not registered in the active profile.
    UnknownTool(String),
    /// The tool returned `ToolResult { success: false, .. }` from its
    /// implementation. The error string is forwarded to the model.
    ToolError(String),
    /// The tool hit the per-tool execution timeout or the session cancel
    /// token fired mid-execution. We do NOT push a synthetic `tool_result`
    /// to the model — the turn terminates with `StopReason::Cancelled` and
    /// any partial assistant text is persisted. This matches the
    /// "cancel-mid-loop" branch of the feat-037 spec.
    Aborted,
}

/// Return value of the inner `agent_loop` helper. The outer
/// `run_prompt_task` uses these fields to drive finalization
/// (persistence, status, broadcast).
#[derive(Debug)]
struct LoopResult {
    /// Concatenated assistant text from every model turn in this loop.
    accumulated: String,
    /// Final `StopReason` after the loop exits. Distinct from the
    /// "in-turn" `StopReason::ToolUse` that *drives* the loop — by the
    /// time we return, the model either ended naturally, was cancelled,
    /// hit `max_tokens`, or tripped the per-turn iteration cap.
    stop_reason: agent::StopReason,
    /// Per-tool records collected across every iteration. Empty if the
    /// model never called a tool.
    tool_calls: Vec<ToolCallRecord>,
    /// True if the agent stream produced an `Error` event or the
    /// provider errored before producing a `Done`.
    had_error: bool,
    /// True if the session's cancel token fired (or the per-tool
    /// `tokio::select!` cancelled an in-flight tool).
    cancelled: bool,
}

/// Run one tool call and translate the outcome into something the loop
/// can act on. Always returns within `TOOL_EXECUTION_TIMEOUT` unless
/// the session is cancelled — in which case it returns `Aborted` and
/// the in-flight tool future is dropped.
async fn execute_tool_call(
    tools: &ToolRegistry,
    ctx: &crate::tools::ToolContext,
    cancel_token: &CancellationToken,
    _tool_use_id: &str,
    name: &str,
    input: &serde_json::Value,
) -> ToolOutcome {
    use crate::tools::sanitize_tool_input;
    use std::time::Instant;
    let started = Instant::now();
    // Sanitize: trim string leaves so schema validation isn't tripped by
    // accidental whitespace from the streamed input.
    let input = sanitize_tool_input(input);

    // Unknown tool name → mark the result so the loop records an
    // `is_error=true` tool_result and the model can try something else.
    let Some(tool) = tools.get(name) else {
        warn!(session_id = %ctx.session_id, tool = name, "tool call to unknown tool");
        return ToolOutcome::UnknownTool(name.to_string());
    };

    // JSON-schema validation against the tool's declared `input_schema`.
    // Failure here is recoverable: the loop surfaces a ValidationFailed
    // outcome and the model gets to see why its call was rejected.
    let schema = tool.input_schema();
    match jsonschema::JSONSchema::options()
        .with_draft(jsonschema::Draft::Draft7)
        .compile(&schema)
    {
        Ok(validator) => {
            if let Err(errors) = validator.validate(&input) {
                let detail = errors.map(|e| e.to_string()).collect::<Vec<_>>().join("; ");
                warn!(
                    session_id = %ctx.session_id,
                    tool = name,
                    detail = %detail,
                    "tool input failed JSON-schema validation"
                );
                return ToolOutcome::ValidationFailed(detail);
            }
        }
        Err(e) => {
            // A tool that ships an unparseable schema is a build-time bug.
            // We log and let the call proceed — better to attempt the call
            // than to wedge the loop on a tool that probably works anyway.
            warn!(
                session_id = %ctx.session_id,
                tool = name,
                error = %e,
                "tool input_schema failed to compile; skipping validation"
            );
        }
    }

    // Race the tool future against the per-tool timeout and the session
    // cancel token. Whichever fires first wins. On cancel we drop the
    // tool future WITHOUT pushing a synthetic tool_result (per spec).
    let tool_fut = tool.execute(input, ctx);
    tokio::select! {
        biased;
        _ = cancel_token.cancelled() => {
            warn!(session_id = %ctx.session_id, tool = name, "tool aborted by cancel token");
            ToolOutcome::Aborted
        }
        result = tokio::time::timeout(TOOL_EXECUTION_TIMEOUT, tool_fut) => {
            let duration_ms = started.elapsed().as_millis() as u64;
            match result {
                Ok(tool_result) => {
                    if tool_result.success {
                        ToolOutcome::Completed {
                            result: tool_result.data,
                            duration_ms,
                        }
                    } else {
                        let detail = tool_result
                            .error
                            .unwrap_or_else(|| "tool returned success=false".to_string());
                        warn!(
                            session_id = %ctx.session_id,
                            tool = name,
                            detail = %detail,
                            "tool reported failure"
                        );
                        ToolOutcome::ToolError(detail)
                    }
                }
                Err(_elapsed) => {
                    warn!(
                        session_id = %ctx.session_id,
                        tool = name,
                        timeout_ms = TOOL_EXECUTION_TIMEOUT.as_millis() as u64,
                        "tool execution timed out"
                    );
                    ToolOutcome::ToolError(format!(
                        "tool '{}' exceeded the {}-second execution timeout",
                        name,
                        TOOL_EXECUTION_TIMEOUT.as_secs()
                    ))
                }
            }
        }
    }
}

/// Flush the per-turn `Thinking` buffer as a single `Decision` trace event.
///
/// The Anthropic provider streams extended thinking as many small chunks
/// (2-3 words each). The Journey sidebar shows one row per reasoning pass,
/// not dozens of fragments, so we coalesce them into a single `Decision`
/// event at the natural block boundary (a `TextDelta`, a `ToolUseStart`,
/// the end of the turn, or a mid-stream `Error`). A whitespace-only buffer
/// is dropped without emitting anything — the model occasionally emits a
/// stray newline that is not worth recording.
///
/// Mirrors the pre-feat-037 helper of the same name. Lives outside
/// `agent_loop` so the test module can call it directly.
fn flush_thinking(trace_collector: &trace::TraceCollector, session_id: &str, buf: &mut String) {
    if buf.trim().is_empty() {
        buf.clear();
        return;
    }
    let text = std::mem::take(buf);
    trace_collector.emit(TraceEvent {
        session_id: session_id.to_string(),
        kind: TraceEventKind::Decision { text },
        timestamp: chrono::Utc::now().to_rfc3339(),
    });
}

fn emit_error_trace(trace_collector: &trace::TraceCollector, session_id: &str, message: String) {
    trace_collector.emit(TraceEvent {
        session_id: session_id.to_string(),
        kind: TraceEventKind::Error { message },
        timestamp: chrono::Utc::now().to_rfc3339(),
    });
}

fn provider_error_trace_message(e: &crate::error::ProviderError) -> String {
    match e {
        crate::error::ProviderError::AuthFailed => "Authentication failed".to_string(),
        crate::error::ProviderError::RateLimited { retry_after_ms } => {
            format!("Rate limited, retry after {retry_after_ms}ms")
        }
        crate::error::ProviderError::ModelNotFound { model } => {
            format!("Model not found: {model}")
        }
        crate::error::ProviderError::Unreachable(_) => "Provider unreachable".to_string(),
        crate::error::ProviderError::StreamInterrupted(_) => {
            "Provider stream interrupted".to_string()
        }
    }
}

fn stream_event_error_trace_message() -> String {
    "Provider stream interrupted".to_string()
}

/// Drive the model ↔ tool-execution loop for a single user turn.
///
/// Responsibilities:
///   * Build the initial `MessageRequest` from session history + tools.
///   * Stream the agent response, forwarding every event to SSE
///     subscribers and accumulating text/thinking/tool-use records.
///   * On `StopReason::ToolUse`, execute the tool(s) through
///     `ToolRegistry` (with JSON-schema validation, timeout, and
///     cancel-aware teardown), append a `tool_result` block to the
///     conversation history, and re-call the agent.
///   * Stop on `EndTurn` / `MaxTokens` / `Cancelled` / after
///     `MAX_TOOL_ITERATIONS` iterations.
///   * On cancel mid-loop, drop in-flight tool futures and persist
///     whatever assistant text was already streamed.
#[allow(clippy::too_many_arguments)]
async fn agent_loop(
    agent: &Arc<dyn crate::agent::CodingAgent>,
    sse_manager: &Arc<crate::sse::SseManager>,
    tools: &Arc<ToolRegistry>,
    trace_collector: &trace::TraceCollector,
    session_id: &str,
    cancel_token: &CancellationToken,
    model: String,
    system_prompt: Option<String>,
    max_tokens: u32,
    mut tool_defs: Option<Vec<agent::ToolDefinition>>,
    mut history: Vec<agent::Message>,
    tool_ctx: crate::tools::ToolContext,
    turn_ctx: &crate::agent::turn_context::TurnContext,
    runtime_kind: agent::RuntimeKind,
    mode: agent::SessionMode,
) -> LoopResult {
    use agent::StreamEvent;
    use futures_util::StreamExt;
    use std::collections::BTreeMap;

    let mut accumulated = String::new();
    let mut tool_calls: Vec<ToolCallRecord> = Vec::new();
    let mut final_stop_reason: agent::StopReason = agent::StopReason::EndTurn;
    let mut had_error = false;
    let mut cancelled = false;

    // In-flight tool_use blocks received in the current model turn that
    // have not yet been paired with a tool_result. The current Anthropic
    // model emits one tool_use per turn (no parallel calls yet), but we
    // keep the map shape so that future model upgrades to parallel
    // tool_use don't need a structural rewrite here. BTreeMap (not
    // HashMap) so the order in which tool_use blocks are spliced into
    // history is deterministic — important for reproducible persisted
    // messages and stable test assertions.
    let mut pending_tool_requests: BTreeMap<String, (String, serde_json::Value)> = BTreeMap::new();
    // Track the assistant text emitted in the current turn so we can
    // splice it into history as a single structured message before the
    // next agent call.
    let mut turn_text = String::new();
    // Coalesce `Thinking` stream deltas into a single per-block buffer.
    // Flushed into a `Decision` trace event at the natural boundary
    // (TextDelta / ToolUseStart / Done / Error) by `flush_thinking`.
    // See `flush_thinking` for the rationale on coalescing.
    let mut thinking_buffer = String::new();

    for iteration in 0..MAX_TOOL_ITERATIONS {
        if cancel_token.is_cancelled() {
            cancelled = true;
            final_stop_reason = agent::StopReason::Cancelled;
            break;
        }

        // Call the agent. The per-turn context is built once in
        // `run_prompt_task` and reused across iterations — the cancel
        // token and session/workspace ids don't change mid-loop. HTTP
        // (`AnthropicAgent`) implementations ignore every field today;
        // future `CliCodingAgent` (feat-043+) will read `cwd`,
        // `cli_resume_id`, and `effective_permissions`.
        let stream = match agent
            .send_message(
                agent::MessageRequest {
                    model: model.clone(),
                    messages: history.clone(),
                    system: system_prompt.clone(),
                    max_tokens,
                    tools: tool_defs.clone(),
                },
                turn_ctx,
            )
            .await
        {
            Ok(s) => s,
            Err(e) => {
                error!(session_id, error = %e, "agent send_message failed mid-loop");
                let trace_message = provider_error_trace_message(&e);
                let message = e.to_string();
                emit_error_trace(trace_collector, session_id, trace_message);
                sse_manager.broadcast(session_id, SseWireEvent::Error { message });
                had_error = true;
                final_stop_reason = agent::StopReason::EndTurn;
                break;
            }
        };

        let mut stream = stream;
        let mut turn_stop_reason: Option<agent::StopReason> = None;
        pending_tool_requests.clear();
        turn_text.clear();
        thinking_buffer.clear();

        // Stream this turn to completion.
        loop {
            tokio::select! {
                _ = cancel_token.cancelled() => {
                    info!(session_id, "session cancelled by user (mid-iteration {iteration})");
                    cancelled = true;
                    final_stop_reason = agent::StopReason::Cancelled;
                    break;
                }
                item = StreamExt::next(&mut stream) => {
                    match item {
                        Some(Ok(event)) => {
                            // Forward every event to SSE subscribers. The
                            // `Done` event is forwarded after we know the
                            // final stop reason (see below).
                            match &event {
                                StreamEvent::Done { .. } => {
                                    // Captured into `turn_stop_reason`
                                    // below; not broadcast here.
                                }
                                _ => {
                                    sse_manager.broadcast(
                                        session_id,
                                        sse::stream_event_to_wire(
                                            event.clone(),
                                            runtime_kind,
                                            mode,
                                        ),
                                    );
                                }
                            }
                            match event {
                                StreamEvent::TextDelta { text } => {
                                    // Text after a thinking block is a
                                    // natural Decision boundary — flush
                                    // whatever has accumulated so far.
                                    flush_thinking(trace_collector, session_id, &mut thinking_buffer);
                                    turn_text.push_str(&text);
                                    accumulated.push_str(&text);
                                }
                                StreamEvent::ToolUseStart { id, name, input } => {
                                    // A tool_use after thinking is also a
                                    // natural Decision boundary.
                                    flush_thinking(trace_collector, session_id, &mut thinking_buffer);
                                    pending_tool_requests.insert(id, (name, input));
                                }
                                StreamEvent::ToolUseDelta { .. } => {
                                    // Already handled at block stop in the
                                    // EventConverter (assembled `Value` is
                                    // delivered here as a single ToolUseStart).
                                }
                                StreamEvent::ToolResult { .. } => {
                                    // The native agent loop produces its own
                                    // `ToolResult` events (see below); the
                                    // provider stream does not emit them.
                                }
                                StreamEvent::Thinking { text } => {
                                    // Forwarded to SSE above; coalesce into
                                    // the per-block buffer. Flushed on the
                                    // next boundary event.
                                    thinking_buffer.push_str(&text);
                                }
                                StreamEvent::Done { stop_reason } => {
                                    // End of turn — flush any pending
                                    // thinking before recording the stop
                                    // reason and breaking.
                                    flush_thinking(trace_collector, session_id, &mut thinking_buffer);
                                    turn_stop_reason = Some(stop_reason);
                                    break;
                                }
                                StreamEvent::Error { message } => {
                                    error!(session_id, error = %message, "agent stream error");
                                    flush_thinking(trace_collector, session_id, &mut thinking_buffer);
                                    emit_error_trace(
                                        trace_collector,
                                        session_id,
                                        stream_event_error_trace_message(),
                                    );
                                    had_error = true;
                                    turn_stop_reason = Some(agent::StopReason::EndTurn);
                                    break;
                                }
                            }
                        }
                        Some(Err(e)) => {
                            error!(session_id, error = %e, "agent stream provider error");
                            flush_thinking(trace_collector, session_id, &mut thinking_buffer);
                            let trace_message = provider_error_trace_message(&e);
                            let message = e.to_string();
                            emit_error_trace(trace_collector, session_id, trace_message);
                            had_error = true;
                            sse_manager.broadcast(session_id, SseWireEvent::Error { message });
                            turn_stop_reason = Some(agent::StopReason::EndTurn);
                            break;
                        }
                        None => {
                            // Stream ended without a Done event.
                            break;
                        }
                    }
                }
            }
        }

        // Re-check cancel after the per-turn stream loop.
        if cancel_token.is_cancelled() {
            cancelled = true;
            final_stop_reason = agent::StopReason::Cancelled;
            break;
        }

        let turn_stop = turn_stop_reason.unwrap_or(agent::StopReason::EndTurn);

        // Natural terminal stop reasons: persist and exit.
        match &turn_stop {
            agent::StopReason::EndTurn
            | agent::StopReason::MaxTokens
            | agent::StopReason::Cancelled => {
                // Append the assistant turn to history (so the next user
                // prompt has a coherent record) and exit the loop.
                if !turn_text.is_empty() || !pending_tool_requests.is_empty() {
                    let mut blocks = Vec::new();
                    if !turn_text.is_empty() {
                        blocks.push(agent::ContentBlock::Text {
                            text: turn_text.clone(),
                        });
                    }
                    for (id, (name, input)) in std::mem::take(&mut pending_tool_requests) {
                        blocks.push(agent::ContentBlock::ToolUse { id, name, input });
                    }
                    history.push(agent::Message {
                        role: agent::Role::Assistant,
                        content: agent::Content::Blocks(blocks),
                    });
                }
                final_stop_reason = turn_stop;
                break;
            }
            agent::StopReason::LoopLimit { .. } => {
                // The agent itself should not be able to produce a
                // LoopLimit; if it does, treat it as EndTurn and break.
                final_stop_reason = agent::StopReason::EndTurn;
                break;
            }
            agent::StopReason::ToolUse => {
                // The model wants to call one or more tools. Execute them.
            }
        }

        if pending_tool_requests.is_empty() {
            // Defensive: the model said `tool_use` but produced no
            // tool_use blocks. Persist the turn text and exit so we
            // don't loop forever.
            warn!(
                session_id,
                "model returned tool_use stop_reason with no tool_use blocks"
            );
            if !turn_text.is_empty() {
                history.push(agent::Message {
                    role: agent::Role::Assistant,
                    content: agent::Content::Text(turn_text.clone()),
                });
            }
            final_stop_reason = agent::StopReason::EndTurn;
            break;
        }

        // Persist this turn into history as a single structured assistant
        // message (text + tool_use blocks). The next user-role message in
        // history will be the structured `tool_result` block.
        let mut turn_blocks: Vec<agent::ContentBlock> = Vec::new();
        if !turn_text.is_empty() {
            turn_blocks.push(agent::ContentBlock::Text {
                text: turn_text.clone(),
            });
        }
        let tool_use_ids: Vec<String> = pending_tool_requests.keys().cloned().collect();
        for id in &tool_use_ids {
            if let Some((name, input)) = pending_tool_requests.get(id) {
                turn_blocks.push(agent::ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: input.clone(),
                });
            }
        }
        history.push(agent::Message {
            role: agent::Role::Assistant,
            content: agent::Content::Blocks(turn_blocks),
        });

        // Execute each tool and append a `tool_result` block to history.
        let mut any_aborted = false;
        for tool_use_id in &tool_use_ids {
            let (name, input) = match pending_tool_requests.get(tool_use_id) {
                Some(v) => v.clone(),
                None => continue,
            };
            let outcome =
                execute_tool_call(tools, &tool_ctx, cancel_token, tool_use_id, &name, &input).await;

            // Translate the `ToolOutcome` into the three fields the
            // post-tool loop cares about: (is_error, content_str,
            // duration_ms). `Aborted` is handled separately below.
            let (is_error, content, duration_ms) = match outcome {
                ToolOutcome::Completed {
                    result,
                    duration_ms,
                } => {
                    let s = serde_json::to_string(&result).unwrap_or_else(|_| result.to_string());
                    (false, s, duration_ms)
                }
                ToolOutcome::ValidationFailed(detail) | ToolOutcome::ToolError(detail) => {
                    (true, detail, 0)
                }
                ToolOutcome::UnknownTool(name) => (true, format!("unknown tool: {name}"), 0),
                ToolOutcome::Aborted => {
                    // Per spec: do NOT push a synthetic tool_result. Drop
                    // the in-flight tool future; the outer loop will see
                    // cancel_token.cancelled() on the next iteration and
                    // break with StopReason::Cancelled.
                    any_aborted = true;
                    break;
                }
            };

            // Emit a `ToolCall` trace event (regression guard for the
            // feat-017 emission that was lost in feat-037). All non-Aborted
            // outcomes are recorded so the Journey sidebar can show what
            // the model tried — including validation failures and unknown
            // tool names. `duration_ms == 0` for the failure paths is the
            // honest signal: we never started the actual tool execution.
            let trace_ts = chrono::Utc::now().to_rfc3339();
            trace_collector.emit(TraceEvent {
                session_id: session_id.to_string(),
                kind: TraceEventKind::ToolCall {
                    tool_name: name.clone(),
                    input_json: input.to_string(),
                    output_json: content.clone(),
                    duration_ms,
                },
                timestamp: trace_ts.clone(),
            });
            // Extract and emit `FileChange` events for the tools that
            // mutate files. Other tools contribute no rows. Existing
            // helper; `fs_write` / `fs_edit` are recognised by name.
            for fc in trace::extract_file_changes(session_id, &name, &input, &trace_ts) {
                trace_collector.emit(fc);
            }

            // Single emission path: broadcast the visible ToolResult, record
            // the call in the tool-call summary, and push a tool_result
            // content block back into the conversation history so the model
            // sees the outcome on its next turn.
            sse_manager.broadcast(
                session_id,
                SseWireEvent::ToolResult {
                    id: tool_use_id.clone(),
                    result: content.clone(),
                },
            );
            tool_calls.push(ToolCallRecord {
                tool_use_id: tool_use_id.clone(),
                name: name.clone(),
                is_error,
                duration_ms,
            });
            history.push(agent::Message {
                role: agent::Role::User,
                content: agent::Content::Blocks(vec![agent::ContentBlock::ToolResult {
                    tool_use_id: tool_use_id.clone(),
                    content,
                    is_error,
                }]),
            });
        }

        if any_aborted || cancel_token.is_cancelled() {
            cancelled = true;
            final_stop_reason = agent::StopReason::Cancelled;
            break;
        }

        // Continue the loop; the next iteration will re-call the agent
        // with the updated history (now including the tool_result blocks).
    }

    // If we got here without setting final_stop_reason, we hit the cap.
    if !cancelled
        && !had_error
        && final_stop_reason == agent::StopReason::EndTurn
        && tool_calls.len() as u32 >= MAX_TOOL_ITERATIONS
    {
        // The for-loop exhausted MAX_TOOL_ITERATIONS without breaking on
        // a terminal stop reason. Surface a final user-visible message
        // and stop with LoopLimit.
        let note =
            format!("Sorry, too many tool calls (>{MAX_TOOL_ITERATIONS}). Stopping the loop.");
        let note_for_history = note.clone();
        accumulated.push_str(&note);
        sse_manager.broadcast(session_id, SseWireEvent::TextDelta { text: note });
        history.push(agent::Message {
            role: agent::Role::Assistant,
            content: agent::Content::Text(note_for_history),
        });
        final_stop_reason = agent::StopReason::LoopLimit {
            iterations: MAX_TOOL_ITERATIONS,
        };
    }

    // Strip tool_defs from the final request shape: we don't want to
    // re-send them on a hypothetical follow-up call in the same task.
    let _ = tool_defs.take();

    LoopResult {
        accumulated,
        stop_reason: final_stop_reason,
        tool_calls,
        had_error,
        cancelled,
    }
}

/// Build the `messages.metadata` JSON string for a persisted assistant
/// message. Returns `None` only for the boring "no tools called and the
/// turn ended cleanly" case so we don't write empty `{}` rows to the DB.
/// As soon as the loop executed at least one tool — or the turn ended
/// for any other reason (cancel, error, max_tokens, loop limit) — we
/// return a JSON row so the frontend can render the appropriate badge
/// and the "N tool calls" summary.
///
/// The frontend parses this JSON to render a "Cancelled" / "Error" /
/// "LoopLimit" / "MaxTokens" badge on the persisted bubble, and the
/// "N tool calls" summary.
fn build_message_metadata(
    stop_reason: &agent::StopReason,
    had_error: bool,
    tool_call_summary: &[ToolCallRecord],
) -> Option<String> {
    let tag: &str = if had_error {
        "error"
    } else {
        match stop_reason {
            agent::StopReason::Cancelled => "cancelled",
            agent::StopReason::MaxTokens => "max_tokens",
            agent::StopReason::LoopLimit { .. } => "loop_limit",
            agent::StopReason::EndTurn | agent::StopReason::ToolUse => "end_turn",
        }
    };
    // The "no tools called AND end_turn/tool_use" case is the only one we
    // suppress — it's the pre-feat-037 "single-shot, nothing interesting
    // to say about it" path, and writing `{"stop_reason":"end_turn"}` for
    // it would double the row size with no information the frontend
    // doesn't already have from `message_persisted.stop_reason`.
    if tool_call_summary.is_empty()
        && matches!(
            stop_reason,
            agent::StopReason::EndTurn | agent::StopReason::ToolUse
        )
        && !had_error
    {
        return None;
    }
    // feat-037: include a tool-call summary when the loop executed at least
    // one tool. This is what powers the "12 tool calls" badge on the
    // persisted bubble and gives the Journey sidebar a single row to
    // summarize the turn rather than 12 fragmented ones.
    let mut obj = serde_json::json!({ "stop_reason": tag });
    if !tool_call_summary.is_empty() {
        let calls: Vec<serde_json::Value> = tool_call_summary
            .iter()
            .map(|r| {
                serde_json::json!({
                    "tool_use_id": r.tool_use_id,
                    "name": r.name,
                    "is_error": r.is_error,
                    "duration_ms": r.duration_ms,
                })
            })
            .collect();
        obj["tool_calls"] = serde_json::Value::Array(calls);
    }
    Some(obj.to_string())
}

/// Compute the wire-format string carried in
/// `message_persisted.stop_reason`. The internal `agent::StopReason` enum
/// has no `Error` variant, so a streaming error is encoded as
/// `("end_turn", had_error=true)` in the internal flow but the wire
/// string is "error" so the frontend can render an Error badge without
/// having to inspect the metadata JSON. The metadata JSON still carries
/// the same "error" tag for symmetry with how the persisted row records
/// it. `None` is reserved for the "no message was persisted this turn"
/// sentinel — every other case returns a stable string.
fn stop_reason_to_wire(stop_reason: &agent::StopReason, had_error: bool) -> Option<String> {
    let s = if had_error {
        "error"
    } else {
        match stop_reason {
            agent::StopReason::EndTurn => "end_turn",
            agent::StopReason::MaxTokens => "max_tokens",
            agent::StopReason::ToolUse => "tool_use",
            agent::StopReason::Cancelled => "cancelled",
            agent::StopReason::LoopLimit { .. } => "loop_limit",
        }
    };
    Some(s.to_string())
}

/// Drop guard that ensures `active_sessions.remove()` is called even on panic.
struct SessionGuard {
    active: Arc<ActiveSessions>,
    session_id: String,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.active.remove(&self.session_id);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::sessions::tests::seed_deps;
    use crate::store::traces::TraceStore;
    use crate::tools::test_support::MockTool;
    use crate::tools::{ToolContext, ToolExecutor, ToolRegistry, ToolResult};
    use async_trait::async_trait;
    use std::sync::Mutex;

    fn test_db() -> Arc<Db> {
        Arc::new(Db::open(std::path::Path::new(":memory:")).unwrap())
    }

    /// A mock agent that captures the `MessageRequest` and the
    /// `TurnContext` it receives (feat-041). Used by
    /// `test_turn_context_passes_cwd_and_codebase` to assert the
    /// per-turn context reaches the agent intact.
    struct CapturingAgent {
        captured: Arc<Mutex<Option<MessageRequest>>>,
        captured_turn: Arc<Mutex<Option<agent::turn_context::TurnContext>>>,
    }

    impl CapturingAgent {
        fn new() -> Self {
            Self {
                captured: Arc::new(Mutex::new(None)),
                captured_turn: Arc::new(Mutex::new(None)),
            }
        }
    }

    #[async_trait]
    impl crate::agent::CodingAgent for CapturingAgent {
        fn provider_type(&self) -> &str {
            "mock"
        }
        fn display_name(&self) -> &str {
            "Mock"
        }
        async fn list_models(
            &self,
        ) -> Result<Vec<crate::agent::ModelInfo>, crate::error::ProviderError> {
            Ok(vec![])
        }
        async fn send_message(
            &self,
            request: MessageRequest,
            turn: &agent::turn_context::TurnContext,
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn futures_core::Stream<
                            Item = Result<agent::StreamEvent, crate::error::ProviderError>,
                        > + Send,
                >,
            >,
            crate::error::ProviderError,
        > {
            *self.captured.lock().unwrap() = Some(request);
            *self.captured_turn.lock().unwrap() = Some(turn.clone());
            let (tx, rx) = tokio::sync::mpsc::channel(16);
            tokio::spawn(async move {
                let _ = tx
                    .send(Ok(agent::StreamEvent::Done {
                        stop_reason: agent::StopReason::EndTurn,
                    }))
                    .await;
            });
            Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
        }
        async fn health_check(
            &self,
        ) -> Result<crate::agent::ProviderHealth, crate::error::ProviderError> {
            Ok(crate::agent::ProviderHealth {
                healthy: true,
                latency_ms: 0,
                error: None,
            })
        }
    }

    fn test_state() -> (
        Arc<Db>,
        Arc<ProviderRegistry>,
        Arc<SpecialistRegistry>,
        Arc<ActiveSessions>,
        Arc<crate::sse::SseManager>,
        Arc<ToolRegistry>,
    ) {
        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let specialists = Arc::new(SpecialistRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());
        let tools = Arc::new(ToolRegistry::new());
        (db, registry, specialists, active, sse, tools)
    }

    #[test]
    fn test_build_message_history_user_only() {
        let messages = vec![crate::store::sessions::Message {
            id: "1".into(),
            session_id: "s1".into(),
            role: "user".into(),
            content: "hello".into(),
            metadata: None,
            created_at: "2026-01-01T00:00:00Z".into(),
        }];

        let history = SessionService::build_message_history(&messages);
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].role, agent::Role::User);
        match &history[0].content {
            agent::Content::Text(t) => assert_eq!(t, "hello"),
            _ => panic!("expected Content::Text"),
        }
    }

    #[test]
    fn test_build_message_history_mixed_roles() {
        let messages = vec![
            crate::store::sessions::Message {
                id: "1".into(),
                session_id: "s1".into(),
                role: "user".into(),
                content: "hi".into(),
                metadata: None,
                created_at: "2026-01-01T00:00:00Z".into(),
            },
            crate::store::sessions::Message {
                id: "2".into(),
                session_id: "s1".into(),
                role: "assistant".into(),
                content: "hello!".into(),
                metadata: None,
                created_at: "2026-01-01T00:00:01Z".into(),
            },
        ];

        let history = SessionService::build_message_history(&messages);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].role, agent::Role::User);
        assert_eq!(history[1].role, agent::Role::Assistant);
    }

    #[test]
    fn test_resolve_model_session_override() {
        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let (ws_id, provider_id) = seed_deps(&db);

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            Some("claude-opus-4-20250514"),
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        let model = SessionService::resolve_model(&session, None, &db);
        assert_eq!(model, "claude-opus-4-20250514");
    }

    #[test]
    fn test_resolve_model_provider_default() {
        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let (ws_id, provider_id) = seed_deps(&db);

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        // seed_deps creates provider with default_model in config_json
        let model = SessionService::resolve_model(&session, None, &db);
        assert_eq!(model, "claude-sonnet-4-20250514");
    }

    #[test]
    fn test_resolve_model_hardcoded_fallback() {
        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();

        // Create a provider with no default_model in config
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "anthropic",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k"}"#,
        )
        .unwrap();

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        let model = SessionService::resolve_model(&session, None, &db);
        assert_eq!(model, FALLBACK_MODEL);
    }

    #[test]
    fn test_active_sessions_try_insert() {
        let active = ActiveSessions::new();
        let token = CancellationToken::new();

        // First insert succeeds
        assert!(active.try_insert("s1".to_string(), token.clone()));
        assert!(active.contains("s1"));

        // Duplicate insert fails (TOCTOU-safe)
        assert!(!active.try_insert("s1".to_string(), CancellationToken::new()));

        // Remove and re-insert works
        active.remove("s1");
        assert!(!active.contains("s1"));
        assert!(active.try_insert("s1".to_string(), token));
    }

    #[tokio::test]
    async fn test_send_prompt_empty_prompt() {
        let (db, registry, specialists, active, sse, tools) = test_state();
        let (ws_id, provider_id) = seed_deps(&db);
        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        let result = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "",
        )
        .await;

        assert!(matches!(
            result,
            Err(AppError::Validation { message: _, .. })
        ));
    }

    #[tokio::test]
    async fn test_send_prompt_session_not_found() {
        let (db, registry, specialists, active, sse, tools) = test_state();

        let result = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            "nonexistent",
            "hello",
        )
        .await;

        assert!(matches!(result, Err(AppError::NotFound { .. })));
    }

    #[tokio::test]
    async fn test_send_prompt_terminal_session() {
        let (db, registry, specialists, active, sse, tools) = test_state();
        let (ws_id, provider_id) = seed_deps(&db);
        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        // Transition to terminal state
        crate::store::sessions::SessionStore::update_status(&db, &session.id, "completed").unwrap();

        let result = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "hello",
        )
        .await;

        assert!(matches!(
            result,
            Err(AppError::Validation { message: _, .. })
        ));
    }

    #[tokio::test]
    async fn test_cancel_session_not_active() {
        let (_, _, _, active, _, _) = test_state();

        let result = SessionService::cancel_session(&active, "nonexistent");
        assert!(matches!(
            result,
            Err(AppError::Validation { message: _, .. })
        ));
    }

    #[tokio::test]
    async fn test_send_prompt_conflict_on_double_send() {
        let (db, registry, specialists, active, sse, tools) = test_state();
        let (ws_id, provider_id) = seed_deps(&db);
        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        // Simulate an active session
        let token = CancellationToken::new();
        active.try_insert(session.id.clone(), token);

        let result = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "hello",
        )
        .await;

        assert!(matches!(result, Err(AppError::Conflict(_))));
    }

    #[tokio::test]
    async fn test_send_prompt_flow() {
        let (db, registry, specialists, active, sse, tools) = test_state();
        let (ws_id, provider_id) = seed_deps(&db);
        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        // Send prompt — should succeed and return a message ID
        let result = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "hello",
        )
        .await;

        let message_id = result.unwrap();
        assert!(!message_id.is_empty());

        // User message should be saved with raw content (no JSON encoding)
        let messages =
            crate::store::sessions::MessageStore::list_by_session(&db, &session.id, None, 10)
                .unwrap();
        assert_eq!(messages.data.len(), 1);
        assert_eq!(messages.data[0].role, "user");
        assert_eq!(messages.data[0].content, "hello");
        assert_eq!(messages.data[0].id, message_id);
    }

    #[tokio::test]
    async fn test_cancel_session() {
        // Verifies the cancel API rejects non-active sessions
        let (_, _, _, active, _, _) = test_state();
        let result = SessionService::cancel_session(&active, "nonexistent");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cancel_session_with_mock_agent() {
        use async_trait::async_trait;
        use futures_core::Stream;
        use std::pin::Pin;

        /// A mock agent that streams events with delays, allowing cancel to fire.
        struct SlowAgent;

        #[async_trait]
        impl crate::agent::CodingAgent for SlowAgent {
            fn provider_type(&self) -> &str {
                "mock"
            }
            fn display_name(&self) -> &str {
                "Mock"
            }
            async fn list_models(
                &self,
            ) -> Result<Vec<crate::agent::ModelInfo>, crate::error::ProviderError> {
                Ok(vec![])
            }
            async fn send_message(
                &self,
                _request: MessageRequest,
                _turn: &agent::turn_context::TurnContext,
            ) -> Result<
                Pin<
                    Box<
                        dyn Stream<Item = Result<agent::StreamEvent, crate::error::ProviderError>>
                            + Send,
                    >,
                >,
                crate::error::ProviderError,
            > {
                let (tx, rx) = tokio::sync::mpsc::channel(16);
                tokio::spawn(async move {
                    // Send a text delta, then wait long enough for cancel to fire
                    let _ = tx
                        .send(Ok(agent::StreamEvent::TextDelta {
                            text: "partial".into(),
                        }))
                        .await;
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                    let _ = tx
                        .send(Ok(agent::StreamEvent::Done {
                            stop_reason: agent::StopReason::EndTurn,
                        }))
                        .await;
                });
                Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
            }
            async fn health_check(
                &self,
            ) -> Result<crate::agent::ProviderHealth, crate::error::ProviderError> {
                Ok(crate::agent::ProviderHealth {
                    healthy: true,
                    latency_ms: 0,
                    error: None,
                })
            }
        }

        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let specialists = Arc::new(SpecialistRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());
        let tools = Arc::new(ToolRegistry::new());

        // Create workspace, provider, and register the mock agent
        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "mock",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k","default_model":"mock"}"#,
        )
        .unwrap();
        registry.add_agent(&provider.id, Arc::new(SlowAgent));

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        // Send prompt — the mock agent will stream slowly
        let message_id = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "hello",
        )
        .await
        .unwrap();
        assert!(!message_id.is_empty());
        assert!(active.contains(&session.id));

        // Cancel — should succeed
        let result = SessionService::cancel_session(&active, &session.id);
        assert!(result.is_ok());

        // Wait for the spawned task to complete (poll up to 2 seconds)
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while active.contains(&session.id) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            !active.contains(&session.id),
            "session should be removed from active set after cancel"
        );

        // Session status should be cancelled
        let session = crate::store::sessions::SessionStore::get_by_id(&db, &session.id).unwrap();
        assert_eq!(session.status, "cancelled");
    }

    #[tokio::test]
    async fn test_cancel_persists_partial_text_with_metadata() {
        // The cancel-during-stream path must:
        //   1. Persist the partial text as an assistant message
        //   2. Encode stop_reason=cancelled in messages.metadata
        //   3. Set session.status = "cancelled"
        //   4. Broadcast MessagePersisted (with the persisted id) BEFORE Done{Cancelled}
        // This is the load-bearing new behavior — without it, the user
        // would see the streamed text disappear on cancel with no
        // persisted row to anchor to.
        use async_trait::async_trait;
        use futures_core::Stream;
        use std::pin::Pin;

        struct PartialAgent;

        #[async_trait]
        impl crate::agent::CodingAgent for PartialAgent {
            fn provider_type(&self) -> &str {
                "mock"
            }
            fn display_name(&self) -> &str {
                "Mock"
            }
            async fn list_models(
                &self,
            ) -> Result<Vec<crate::agent::ModelInfo>, crate::error::ProviderError> {
                Ok(vec![])
            }
            async fn send_message(
                &self,
                _request: MessageRequest,
                _turn: &agent::turn_context::TurnContext,
            ) -> Result<
                Pin<
                    Box<
                        dyn Stream<Item = Result<agent::StreamEvent, crate::error::ProviderError>>
                            + Send,
                    >,
                >,
                crate::error::ProviderError,
            > {
                let (tx, rx) = tokio::sync::mpsc::channel(16);
                tokio::spawn(async move {
                    let _ = tx
                        .send(Ok(agent::StreamEvent::TextDelta {
                            text: "first chunk ".into(),
                        }))
                        .await;
                    let _ = tx
                        .send(Ok(agent::StreamEvent::TextDelta {
                            text: "second chunk".into(),
                        }))
                        .await;
                    // Sleep long enough for cancel to fire
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                });
                Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
            }
            async fn health_check(
                &self,
            ) -> Result<crate::agent::ProviderHealth, crate::error::ProviderError> {
                Ok(crate::agent::ProviderHealth {
                    healthy: true,
                    latency_ms: 0,
                    error: None,
                })
            }
        }

        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let specialists = Arc::new(SpecialistRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());
        let tools = Arc::new(ToolRegistry::new());

        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "mock",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k","default_model":"mock"}"#,
        )
        .unwrap();
        registry.add_agent(&provider.id, Arc::new(PartialAgent));

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        let mut rx = sse.subscribe(&session.id);

        SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "go",
        )
        .await
        .unwrap();

        // Wait for at least one text_delta to land
        let _ = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;

        // Cancel
        SessionService::cancel_session(&active, &session.id).unwrap();

        // Collect remaining events
        let mut events = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(event)) => {
                    let is_done = matches!(&event, crate::sse::SseWireEvent::Done { .. });
                    events.push(event);
                    if is_done {
                        break;
                    }
                }
                _ => break,
            }
        }

        // Find the MessagePersisted event; it must have a non-empty id
        // (the partial was persisted) and stop_reason = cancelled.
        let mp = events
            .iter()
            .find_map(|e| match e {
                crate::sse::SseWireEvent::MessagePersisted {
                    id,
                    stop_reason,
                    content,
                    ..
                } => Some((id.clone(), stop_reason.clone(), content.clone())),
                _ => None,
            })
            .expect("expected MessagePersisted event on cancel");
        let (mp_id, mp_stop, mp_content) = mp;
        assert!(
            !mp_id.is_empty(),
            "MessagePersisted id must be non-empty when partial was persisted"
        );
        assert_eq!(mp_stop.as_deref(), Some("cancelled"));
        assert_eq!(mp_content, "first chunk second chunk");

        // The MessagePersisted must come before the Done event in the stream.
        let mp_idx = events
            .iter()
            .position(|e| matches!(e, crate::sse::SseWireEvent::MessagePersisted { .. }))
            .unwrap();
        let done_idx = events
            .iter()
            .position(|e| matches!(e, crate::sse::SseWireEvent::Done { .. }))
            .unwrap();
        assert!(
            mp_idx < done_idx,
            "MessagePersisted must come before Done; got mp_idx={} done_idx={}",
            mp_idx,
            done_idx
        );

        // Wait for the task to clean up
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while active.contains(&session.id) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // Session is cancelled
        let session = crate::store::sessions::SessionStore::get_by_id(&db, &session.id).unwrap();
        assert_eq!(session.status, "cancelled");

        // The persisted row exists with the right metadata
        let history =
            crate::store::sessions::MessageStore::load_all(&db, &session.id, 100).unwrap();
        let partial = history
            .iter()
            .find(|m| m.role == "assistant")
            .expect("partial assistant message");
        assert_eq!(partial.content, "first chunk second chunk");
        let parsed: serde_json::Value =
            serde_json::from_str(partial.metadata.as_deref().unwrap()).unwrap();
        assert_eq!(parsed["stop_reason"], "cancelled");
        // The persisted id matches what the SSE event carried
        assert_eq!(partial.id, mp_id);
    }

    #[tokio::test]
    async fn test_stream_error_persists_partial_text_with_metadata() {
        // When the agent emits StreamEvent::Error mid-stream, the partial
        // streamed text must be persisted with metadata = {"stop_reason":"error"}
        // and the session status must be "error". MessagePersisted
        // carries the persisted id; Done closes the stream.
        use async_trait::async_trait;
        use futures_core::Stream;
        use std::pin::Pin;

        struct ErroringAgent;

        #[async_trait]
        impl crate::agent::CodingAgent for ErroringAgent {
            fn provider_type(&self) -> &str {
                "mock"
            }
            fn display_name(&self) -> &str {
                "Mock"
            }
            async fn list_models(
                &self,
            ) -> Result<Vec<crate::agent::ModelInfo>, crate::error::ProviderError> {
                Ok(vec![])
            }
            async fn send_message(
                &self,
                _request: MessageRequest,
                _turn: &agent::turn_context::TurnContext,
            ) -> Result<
                Pin<
                    Box<
                        dyn Stream<Item = Result<agent::StreamEvent, crate::error::ProviderError>>
                            + Send,
                    >,
                >,
                crate::error::ProviderError,
            > {
                let (tx, rx) = tokio::sync::mpsc::channel(16);
                tokio::spawn(async move {
                    let _ = tx
                        .send(Ok(agent::StreamEvent::TextDelta {
                            text: "before error".into(),
                        }))
                        .await;
                    let _ = tx
                        .send(Ok(agent::StreamEvent::Error {
                            message: "provider stream interrupted token=sk-test".into(),
                        }))
                        .await;
                });
                Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
            }
            async fn health_check(
                &self,
            ) -> Result<crate::agent::ProviderHealth, crate::error::ProviderError> {
                Ok(crate::agent::ProviderHealth {
                    healthy: true,
                    latency_ms: 0,
                    error: None,
                })
            }
        }

        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let specialists = Arc::new(SpecialistRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());
        let tools = Arc::new(ToolRegistry::new());

        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "mock",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k","default_model":"mock"}"#,
        )
        .unwrap();
        registry.add_agent(&provider.id, Arc::new(ErroringAgent));

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        let mut rx = sse.subscribe(&session.id);

        SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "go",
        )
        .await
        .unwrap();

        // Collect events through to Done
        let mut events = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(3);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(event)) => {
                    let is_done = matches!(&event, crate::sse::SseWireEvent::Done { .. });
                    events.push(event);
                    if is_done {
                        break;
                    }
                }
                _ => break,
            }
        }

        // Find the partial MessagePersisted (with non-empty id) and Done
        let mp = events
            .iter()
            .find_map(|e| match e {
                crate::sse::SseWireEvent::MessagePersisted {
                    id,
                    stop_reason,
                    content,
                    ..
                } => Some((id.clone(), stop_reason.clone(), content.clone())),
                _ => None,
            })
            .expect("MessagePersisted on stream error");
        let (mp_id, mp_stop, mp_content) = mp;
        assert!(!mp_id.is_empty());
        assert_eq!(mp_stop.as_deref(), Some("error"));
        assert_eq!(mp_content, "before error");

        // Session is in error status
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while active.contains(&session.id) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let session = crate::store::sessions::SessionStore::get_by_id(&db, &session.id).unwrap();
        assert_eq!(session.status, "error");

        // Persisted row has the error metadata
        let history =
            crate::store::sessions::MessageStore::load_all(&db, &session.id, 100).unwrap();
        let partial = history.iter().find(|m| m.role == "assistant").unwrap();
        let parsed: serde_json::Value =
            serde_json::from_str(partial.metadata.as_deref().unwrap()).unwrap();
        assert_eq!(parsed["stop_reason"], "error");

        let journey = TraceStore::list_journey(&db, &session.id).unwrap();
        assert_eq!(journey.len(), 1);
        assert_eq!(journey[0].event_type, "error");
        assert_eq!(journey[0].summary, "Provider stream interrupted");
        assert!(!journey[0].summary.contains("sk-test"));
    }

    #[tokio::test]
    async fn test_provider_stream_error_writes_journey_error_trace() {
        use async_trait::async_trait;
        use futures_core::Stream;
        use std::pin::Pin;

        struct ProviderErrorAgent;

        #[async_trait]
        impl crate::agent::CodingAgent for ProviderErrorAgent {
            fn provider_type(&self) -> &str {
                "mock"
            }
            fn display_name(&self) -> &str {
                "Mock"
            }
            async fn list_models(
                &self,
            ) -> Result<Vec<crate::agent::ModelInfo>, crate::error::ProviderError> {
                Ok(vec![])
            }
            async fn send_message(
                &self,
                _request: MessageRequest,
                _turn: &agent::turn_context::TurnContext,
            ) -> Result<
                Pin<
                    Box<
                        dyn Stream<Item = Result<agent::StreamEvent, crate::error::ProviderError>>
                            + Send,
                    >,
                >,
                crate::error::ProviderError,
            > {
                let (tx, rx) = tokio::sync::mpsc::channel(16);
                tokio::spawn(async move {
                    let _ = tx
                        .send(Err(crate::error::ProviderError::StreamInterrupted(
                            "network dropped token=sk-test".into(),
                        )))
                        .await;
                });
                Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
            }
            async fn health_check(
                &self,
            ) -> Result<crate::agent::ProviderHealth, crate::error::ProviderError> {
                Ok(crate::agent::ProviderHealth {
                    healthy: true,
                    latency_ms: 0,
                    error: None,
                })
            }
        }

        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let specialists = Arc::new(SpecialistRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());
        let tools = Arc::new(ToolRegistry::new());

        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "mock",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k","default_model":"mock"}"#,
        )
        .unwrap();
        registry.add_agent(&provider.id, Arc::new(ProviderErrorAgent));

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "go",
        )
        .await
        .unwrap();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while active.contains(&session.id) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let session = crate::store::sessions::SessionStore::get_by_id(&db, &session.id).unwrap();
        assert_eq!(session.status, "error");

        let journey = TraceStore::list_journey(&db, &session.id).unwrap();
        assert_eq!(journey.len(), 1);
        assert_eq!(journey[0].event_type, "error");
        assert_eq!(journey[0].summary, "Provider stream interrupted");
        assert!(!journey[0].summary.contains("sk-test"));
    }

    #[tokio::test]
    async fn test_send_message_error_writes_sanitized_journey_error_trace() {
        use async_trait::async_trait;
        use futures_core::Stream;
        use std::pin::Pin;

        struct SendMessageErrorAgent;

        #[async_trait]
        impl crate::agent::CodingAgent for SendMessageErrorAgent {
            fn provider_type(&self) -> &str {
                "mock"
            }
            fn display_name(&self) -> &str {
                "Mock"
            }
            async fn list_models(
                &self,
            ) -> Result<Vec<crate::agent::ModelInfo>, crate::error::ProviderError> {
                Ok(vec![])
            }
            async fn send_message(
                &self,
                _request: MessageRequest,
                _turn: &agent::turn_context::TurnContext,
            ) -> Result<
                Pin<
                    Box<
                        dyn Stream<Item = Result<agent::StreamEvent, crate::error::ProviderError>>
                            + Send,
                    >,
                >,
                crate::error::ProviderError,
            > {
                Err(crate::error::ProviderError::StreamInterrupted(
                    "pre-stream token=sk-test".into(),
                ))
            }
            async fn health_check(
                &self,
            ) -> Result<crate::agent::ProviderHealth, crate::error::ProviderError> {
                Ok(crate::agent::ProviderHealth {
                    healthy: true,
                    latency_ms: 0,
                    error: None,
                })
            }
        }

        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let specialists = Arc::new(SpecialistRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());
        let tools = Arc::new(ToolRegistry::new());

        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "mock",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k","default_model":"mock"}"#,
        )
        .unwrap();
        registry.add_agent(&provider.id, Arc::new(SendMessageErrorAgent));

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "go",
        )
        .await
        .unwrap();

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while active.contains(&session.id) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let session = crate::store::sessions::SessionStore::get_by_id(&db, &session.id).unwrap();
        assert_eq!(session.status, "error");

        let journey = TraceStore::list_journey(&db, &session.id).unwrap();
        assert_eq!(journey.len(), 1);
        assert_eq!(journey[0].event_type, "error");
        assert_eq!(journey[0].summary, "Provider stream interrupted");
        assert!(!journey[0].summary.contains("sk-test"));
    }

    #[tokio::test]
    async fn test_empty_turn_emits_sentinel_message_persisted() {
        // A turn that streams no text (e.g. tool-only or instant Done
        // with no text) must still emit a MessagePersisted event with
        // id="" so the frontend collapses any live bubble. No row
        // is written to the messages table in this case.
        use async_trait::async_trait;
        use futures_core::Stream;
        use std::pin::Pin;

        struct EmptyAgent;

        #[async_trait]
        impl crate::agent::CodingAgent for EmptyAgent {
            fn provider_type(&self) -> &str {
                "mock"
            }
            fn display_name(&self) -> &str {
                "Mock"
            }
            async fn list_models(
                &self,
            ) -> Result<Vec<crate::agent::ModelInfo>, crate::error::ProviderError> {
                Ok(vec![])
            }
            async fn send_message(
                &self,
                _request: MessageRequest,
                _turn: &agent::turn_context::TurnContext,
            ) -> Result<
                Pin<
                    Box<
                        dyn Stream<Item = Result<agent::StreamEvent, crate::error::ProviderError>>
                            + Send,
                    >,
                >,
                crate::error::ProviderError,
            > {
                let (tx, rx) = tokio::sync::mpsc::channel(16);
                tokio::spawn(async move {
                    let _ = tx
                        .send(Ok(agent::StreamEvent::Done {
                            stop_reason: agent::StopReason::EndTurn,
                        }))
                        .await;
                });
                Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
            }
            async fn health_check(
                &self,
            ) -> Result<crate::agent::ProviderHealth, crate::error::ProviderError> {
                Ok(crate::agent::ProviderHealth {
                    healthy: true,
                    latency_ms: 0,
                    error: None,
                })
            }
        }

        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let specialists = Arc::new(SpecialistRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());
        let tools = Arc::new(ToolRegistry::new());

        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "mock",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k","default_model":"mock"}"#,
        )
        .unwrap();
        registry.add_agent(&provider.id, Arc::new(EmptyAgent));

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        let mut rx = sse.subscribe(&session.id);
        SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "go",
        )
        .await
        .unwrap();

        // Collect events through Done
        let mut events = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(event)) => {
                    let is_done = matches!(&event, crate::sse::SseWireEvent::Done { .. });
                    events.push(event);
                    if is_done {
                        break;
                    }
                }
                _ => break,
            }
        }

        // There must be exactly one MessagePersisted event, and it must
        // carry id="" (sentinel for "no row persisted").
        let mp_events: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                crate::sse::SseWireEvent::MessagePersisted { id, .. } => Some(id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(mp_events.len(), 1);
        assert_eq!(mp_events[0], "", "empty turn should emit sentinel id=\"\"");

        // No assistant row was created
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while active.contains(&session.id) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let history =
            crate::store::sessions::MessageStore::load_all(&db, &session.id, 100).unwrap();
        assert!(!history.iter().any(|m| m.role == "assistant"));
    }

    #[tokio::test]
    async fn test_sse_broadcast_on_prompt() {
        use async_trait::async_trait;
        use futures_core::Stream;
        use std::pin::Pin;

        /// A mock agent that streams text deltas and a done event.
        struct MockStreamAgent;

        #[async_trait]
        impl crate::agent::CodingAgent for MockStreamAgent {
            fn provider_type(&self) -> &str {
                "mock"
            }
            fn display_name(&self) -> &str {
                "Mock"
            }
            async fn list_models(
                &self,
            ) -> Result<Vec<crate::agent::ModelInfo>, crate::error::ProviderError> {
                Ok(vec![])
            }
            async fn send_message(
                &self,
                _request: MessageRequest,
                _turn: &agent::turn_context::TurnContext,
            ) -> Result<
                Pin<
                    Box<
                        dyn Stream<Item = Result<agent::StreamEvent, crate::error::ProviderError>>
                            + Send,
                    >,
                >,
                crate::error::ProviderError,
            > {
                let (tx, rx) = tokio::sync::mpsc::channel(16);
                tokio::spawn(async move {
                    let _ = tx
                        .send(Ok(agent::StreamEvent::TextDelta {
                            text: "Hello".into(),
                        }))
                        .await;
                    let _ = tx
                        .send(Ok(agent::StreamEvent::TextDelta {
                            text: " World".into(),
                        }))
                        .await;
                    let _ = tx
                        .send(Ok(agent::StreamEvent::Done {
                            stop_reason: agent::StopReason::EndTurn,
                        }))
                        .await;
                });
                Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
            }
            async fn health_check(
                &self,
            ) -> Result<crate::agent::ProviderHealth, crate::error::ProviderError> {
                Ok(crate::agent::ProviderHealth {
                    healthy: true,
                    latency_ms: 0,
                    error: None,
                })
            }
        }

        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let specialists = Arc::new(SpecialistRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());
        let tools = Arc::new(ToolRegistry::new());

        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "mock",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k","default_model":"mock"}"#,
        )
        .unwrap();
        registry.add_agent(&provider.id, Arc::new(MockStreamAgent));

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        // Subscribe to SSE stream before sending prompt
        let mut rx = sse.subscribe(&session.id);

        // Send prompt
        let message_id = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "hello",
        )
        .await
        .unwrap();
        assert!(!message_id.is_empty());

        // Collect SSE events until we see the Done event
        let mut events = Vec::new();
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await {
                Ok(Ok(event)) => {
                    let is_done = matches!(&event, crate::sse::SseWireEvent::Done { .. });
                    events.push(event);
                    if is_done {
                        break;
                    }
                }
                _ => break,
            }
        }

        // Verify: should have 2 TextDelta + 1 Done
        assert!(
            events.len() >= 3,
            "expected at least 3 events, got {}",
            events.len()
        );

        let text_deltas: Vec<_> = events
            .iter()
            .filter_map(|e| match e {
                crate::sse::SseWireEvent::TextDelta { text } => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(text_deltas, vec!["Hello", " World"]);

        // The last two events must be MessagePersisted (carrying the
        // persisted row) followed by Done. The id-based handoff
        // depends on this exact ordering: when the client sees
        // MessagePersisted, the row is in the DB; when it sees Done,
        // it invalidates the history cache, which refetches a history
        // that already contains the row.
        assert!(
            events.len() >= 4,
            "expected at least 4 events (2 TextDelta + MessagePersisted + Done), got {}",
            events.len()
        );
        match &events[events.len() - 2] {
            crate::sse::SseWireEvent::MessagePersisted {
                id,
                role,
                content,
                stop_reason,
                ..
            } => {
                assert!(
                    !id.is_empty(),
                    "MessagePersisted id must be non-empty for a real turn"
                );
                assert_eq!(role, "assistant");
                assert_eq!(content, "Hello World");
                assert_eq!(stop_reason.as_deref(), Some("end_turn"));
            }
            other => panic!(
                "expected second-to-last event to be MessagePersisted, got {:?}",
                std::mem::discriminant(other)
            ),
        }
        let last = &events[events.len() - 1];
        assert!(
            matches!(
                last,
                crate::sse::SseWireEvent::Done {
                    stop_reason: agent::StopReason::EndTurn,
                    ..
                }
            ),
            "expected last event to be Done{{EndTurn, ..}}"
        );

        // Wait for task to finish
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while active.contains(&session.id) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // Session should be back to "ready" after a successful turn (multi-turn).
        // The terminal "completed" status is only reached via explicit close.
        let session = crate::store::sessions::SessionStore::get_by_id(&db, &session.id).unwrap();
        assert_eq!(session.status, "ready");
    }

    #[tokio::test]
    async fn test_specialist_system_prompt_injection() {
        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let agent = CapturingAgent::new();
        let captured = Arc::clone(&agent.captured);
        let registry = Arc::new(ProviderRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());
        let tools = Arc::new(ToolRegistry::new());

        // Build a specialist registry with one specialist
        let mut specialist_reg = SpecialistRegistry::new();
        let dir = std::env::temp_dir().join("weave-specialist-test-injection");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("coder.md"),
            r#"---
name: coder
description: Writes code
model: claude-opus-4-20250514
---
You are a senior Rust engineer."#,
        )
        .unwrap();
        specialist_reg.load_from_dir(&dir);
        let specialists = Arc::new(specialist_reg);

        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "mock",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k","default_model":"mock"}"#,
        )
        .unwrap();
        registry.add_agent(&provider.id, Arc::new(agent));

        // Create session WITH specialist_id set
        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            Some("coder"), // specialist_id
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        let message_id = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "write a function",
        )
        .await
        .unwrap();
        assert!(!message_id.is_empty());

        // Wait for the spawned task to complete
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while active.contains(&session.id) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // Verify the captured request
        let request = captured
            .lock()
            .unwrap()
            .take()
            .expect("request was not captured");

        // System prompt should be set from specialist
        assert_eq!(
            request.system,
            Some("You are a senior Rust engineer.".to_string())
        );

        // Model should be overridden by specialist (claude-opus-4-20250514)
        assert_eq!(request.model, "claude-opus-4-20250514");

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_tool_profile_filtering() {
        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let agent = CapturingAgent::new();
        let captured = Arc::clone(&agent.captured);
        let registry = Arc::new(ProviderRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());

        // Build a tool registry with tools matching the "planning" profile
        let mut tool_registry = ToolRegistry::new();
        tool_registry.register(Arc::new(MockTool::new("get_task")));
        tool_registry.register(Arc::new(MockTool::new("list_tasks")));
        tool_registry.register(Arc::new(MockTool::new("update_task_fields")));
        tool_registry.register(Arc::new(MockTool::new("get_board")));
        tool_registry.register(Arc::new(MockTool::new("create_card")));
        tool_registry.register(Arc::new(MockTool::new("move_card")));
        tool_registry.register(Arc::new(MockTool::new("search_cards")));
        tool_registry.register(Arc::new(MockTool::new("read_note")));
        tool_registry.register(Arc::new(MockTool::new("list_notes")));
        let tools = Arc::new(tool_registry);

        // Build a specialist registry with a specialist using "planning" profile
        let mut specialist_reg = SpecialistRegistry::new();
        let dir = std::env::temp_dir().join("weave-specialist-test-tool-filtering");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("planner.md"),
            r#"---
name: planner
description: Plans tasks
tool_profile: planning
---
You are a planning specialist."#,
        )
        .unwrap();
        specialist_reg.load_from_dir(&dir);
        let specialists = Arc::new(specialist_reg);

        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "mock",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k","default_model":"mock"}"#,
        )
        .unwrap();
        registry.add_agent(&provider.id, Arc::new(agent));

        // Create session WITH specialist_id set to "planner"
        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            Some("planner"),
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        let message_id = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "plan my work",
        )
        .await
        .unwrap();
        assert!(!message_id.is_empty());

        // Wait for the spawned task to complete
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while active.contains(&session.id) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        // Verify the captured request has the planning profile tools
        let request = captured
            .lock()
            .unwrap()
            .take()
            .expect("request was not captured");

        let tool_defs = request.tools.expect("expected tools to be set");
        let mut tool_names: Vec<&str> = tool_defs.iter().map(|d| d.name.as_str()).collect();
        tool_names.sort();
        assert_eq!(
            tool_names,
            vec![
                "create_card",
                "get_board",
                "get_task",
                "list_notes",
                "list_tasks",
                "move_card",
                "read_note",
                "search_cards",
                "update_task_fields"
            ]
        );

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_send_prompt_invalid_tool_profile() {
        // Create a specialist with an invalid tool_profile
        let (db, registry, _, active, sse, _) = test_state();
        let tools = {
            let mut tr = ToolRegistry::new();
            tr.register(Arc::new(MockTool::new("task")));
            Arc::new(tr)
        };

        let mut specialist_reg = SpecialistRegistry::new();
        let dir = std::env::temp_dir().join("weave-specialist-test-invalid-profile");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("broken.md"),
            r#"---
name: broken
description: Broken specialist
tool_profile: nonexistent_profile
---
You are broken."#,
        )
        .unwrap();
        specialist_reg.load_from_dir(&dir);
        let specialists = Arc::new(specialist_reg);

        let (ws_id, provider_id) = seed_deps(&db);
        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            Some("broken"),
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        let result = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &session.id,
            "hello",
        )
        .await;

        assert!(matches!(
            result,
            Err(AppError::Validation { message: _, .. })
        ));
        match result.unwrap_err() {
            AppError::Validation { message: msg, .. } => {
                assert!(msg.contains("nonexistent_profile"));
            }
            other => panic!("expected Validation, got: {:?}", other),
        }

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    // The previous `test_thinking_deltas_coalesce_into_single_decision`
    // was deleted in feat-037. It asserted that the streaming loop
    // coalesced Thinking deltas into a single Decision trace event.
    // The new `agent_loop` simply forwards Thinking deltas to SSE
    // subscribers and does not emit Decision trace events at all —
    // the Journey sidebar will switch to consuming the per-tool-call
    // trace events emitted by `agent_loop`/`execute_tool_call` instead
    // of coalesced thinking rows. A follow-up feature should either
    // add Decision trace emission to the new loop or remove the
    // sidebar's reliance on it; either way, that work is out of
    // scope for feat-037 and the old test is no longer load-bearing.

    // --- Native Anthropic tool-execution loop (feat-037) ---

    /// A scripted tool whose every aspect — name, input_schema, return
    /// value, sleep, and a counter of how many times it was called — is
    /// controlled by the test. The test_support `MockTool` returns
    /// `null` unconditionally, which is not expressive enough for
    /// the validation-error, exec-error, and cancellation cases.
    struct ScriptedTool {
        name: String,
        schema: serde_json::Value,
        result: std::sync::Arc<std::sync::Mutex<Option<ToolResult>>>,
        call_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        sleep: std::time::Duration,
    }

    #[async_trait]
    impl ToolExecutor for ScriptedTool {
        fn name(&self) -> &str {
            &self.name
        }
        fn description(&self) -> &str {
            "scripted tool for feat-037 tests"
        }
        fn input_schema(&self) -> serde_json::Value {
            self.schema.clone()
        }
        async fn execute(&self, _input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
            self.call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if !self.sleep.is_zero() {
                tokio::time::sleep(self.sleep).await;
            }
            self.result
                .lock()
                .expect("scripted tool result lock poisoned")
                .clone()
                .unwrap_or(ToolResult {
                    success: true,
                    data: serde_json::json!(null),
                    error: None,
                })
        }
    }

    /// A scripted agent that returns a user-supplied list of StreamEvent
    /// sequences, one per call to `send_message`. After the last
    /// sequence, additional calls return `Done { EndTurn }` so the loop
    /// terminates cleanly even if the test authors more events than
    /// there are calls.
    struct ScriptedAgent {
        scripts: std::sync::Arc<std::sync::Mutex<Vec<Vec<agent::StreamEvent>>>>,
        call_count: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[async_trait]
    impl crate::agent::CodingAgent for ScriptedAgent {
        fn provider_type(&self) -> &str {
            "mock"
        }
        fn display_name(&self) -> &str {
            "Mock"
        }
        async fn list_models(
            &self,
        ) -> Result<Vec<crate::agent::ModelInfo>, crate::error::ProviderError> {
            Ok(vec![])
        }
        async fn send_message(
            &self,
            _request: MessageRequest,
            _turn: &agent::turn_context::TurnContext,
        ) -> Result<
            std::pin::Pin<
                Box<
                    dyn futures_core::Stream<
                            Item = Result<agent::StreamEvent, crate::error::ProviderError>,
                        > + Send,
                >,
            >,
            crate::error::ProviderError,
        > {
            let n = self
                .call_count
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let script = self
                .scripts
                .lock()
                .expect("scripted agent lock poisoned")
                .get(n)
                .cloned()
                .unwrap_or_else(|| {
                    vec![agent::StreamEvent::Done {
                        stop_reason: agent::StopReason::EndTurn,
                    }]
                });
            let (tx, rx) = tokio::sync::mpsc::channel(64);
            tokio::spawn(async move {
                for ev in script {
                    if tx.send(Ok(ev)).await.is_err() {
                        return;
                    }
                }
            });
            Ok(Box::pin(tokio_stream::wrappers::ReceiverStream::new(rx)))
        }
        async fn health_check(
            &self,
        ) -> Result<crate::agent::ProviderHealth, crate::error::ProviderError> {
            Ok(crate::agent::ProviderHealth {
                healthy: true,
                latency_ms: 0,
                error: None,
            })
        }
    }

    /// Helper: build a fully-wired session with a scripted agent and an
    /// optional scripted tool. Returns the suite + the registered
    /// provider id so the test can assert against persisted messages and
    /// the script's call counters.
    #[allow(clippy::type_complexity)]
    fn setup_loop_test(
        scripts: Vec<Vec<agent::StreamEvent>>,
        tool: Option<ScriptedTool>,
    ) -> (
        Arc<Db>,
        Arc<ProviderRegistry>,
        Arc<SpecialistRegistry>,
        Arc<ActiveSessions>,
        Arc<crate::sse::SseManager>,
        Arc<ToolRegistry>,
        String,
        String,                                         // session_id
        std::sync::Arc<std::sync::atomic::AtomicUsize>, // agent call count
        std::sync::Arc<std::sync::atomic::AtomicUsize>, // tool call count
    ) {
        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let registry = Arc::new(ProviderRegistry::new());
        let specialists = Arc::new(SpecialistRegistry::new());
        let active = Arc::new(ActiveSessions::new());
        let sse = Arc::new(crate::sse::SseManager::new());
        // Build the registry as a non-Arc value first so we can
        // register the (optional) tool without the Arc<>-mut borrow
        // problem, then wrap in Arc once the registry is finalized.
        let mut tools = ToolRegistry::new();

        let ws = crate::store::workspaces::WorkspaceStore::create(&db, "test-ws").unwrap();
        let provider = crate::store::providers::ProviderStore::create(
            &db,
            "mock",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k","default_model":"mock"}"#,
        )
        .unwrap();

        let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let agent_call_count = std::sync::Arc::clone(&call_count);
        registry.add_agent(
            &provider.id,
            Arc::new(ScriptedAgent {
                scripts: std::sync::Arc::new(std::sync::Mutex::new(scripts)),
                call_count: agent_call_count,
            }),
        );

        let tool_call_count = if let Some(t) = tool {
            let count = std::sync::Arc::clone(&t.call_count);
            tools.register(Arc::new(t));
            count
        } else {
            std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0))
        };
        let tools = Arc::new(tools);

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::default(),
            SessionMode::default(),
            None, // runtime_metadata_json
        )
        .unwrap();

        (
            db,
            registry,
            specialists,
            active,
            sse,
            tools,
            provider.id,
            session.id,
            call_count,
            tool_call_count,
        )
    }

    /// Wait for `run_prompt_task` to drop the session from the active
    /// set, up to `timeout`. The drop signals finalization is complete
    /// (trace flush awaited, status updated, SSE Done broadcast).
    async fn wait_for_session_done(
        active: &Arc<ActiveSessions>,
        session_id: &str,
        timeout: std::time::Duration,
    ) {
        let deadline = tokio::time::Instant::now() + timeout;
        while active.contains(session_id) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            !active.contains(session_id),
            "session {session_id} did not finalize within {timeout:?}"
        );
    }

    /// Spec test 1 of 7: a model that calls a registered tool sees the
    /// tool's output flow back into the conversation, the agent is
    /// re-asked, and the final assistant message reflects the second
    /// turn's text. The tool should be invoked exactly once.
    #[tokio::test]
    async fn test_native_tool_loop_basic() {
        // Turn 1: model asks to call `echo` with a string. Turn 2: model
        // returns a final `Done` with a wrap-up line.
        let turn1 = vec![
            agent::StreamEvent::TextDelta {
                text: "Let me look that up. ".into(),
            },
            agent::StreamEvent::ToolUseStart {
                id: "tu_1".into(),
                name: "echo".into(),
                input: serde_json::json!({"q": "hello"}),
            },
            agent::StreamEvent::Done {
                stop_reason: agent::StopReason::ToolUse,
            },
        ];
        let turn2 = vec![
            agent::StreamEvent::TextDelta {
                text: "Got it: hello".into(),
            },
            agent::StreamEvent::Done {
                stop_reason: agent::StopReason::EndTurn,
            },
        ];

        let tool = ScriptedTool {
            name: "echo".into(),
            schema: serde_json::json!({
                "type": "object",
                "properties": { "q": { "type": "string" } },
                "required": ["q"]
            }),
            result: std::sync::Arc::new(std::sync::Mutex::new(Some(ToolResult {
                success: true,
                data: serde_json::json!({"echoed": "hello"}),
                error: None,
            }))),
            call_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            sleep: std::time::Duration::ZERO,
        };

        let (db, registry, specialists, active, sse, tools, _pid, sid, _ac, tc) =
            setup_loop_test(vec![turn1, turn2], Some(tool));

        let _ = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &sid,
            "say hi",
        )
        .await
        .unwrap();

        wait_for_session_done(&active, &sid, std::time::Duration::from_secs(2)).await;
        assert_eq!(tc.load(std::sync::atomic::Ordering::SeqCst), 1);

        // Persisted message: assistant text from both turns concatenated.
        let messages = MessageStore::load_all(&db, &sid, 100).unwrap();
        let assistant = messages
            .iter()
            .find(|m| m.role == "assistant")
            .expect("assistant message should be persisted");
        assert!(assistant.content.contains("Let me look that up."));
        assert!(assistant.content.contains("Got it: hello"));

        // Tool-call summary in metadata.
        let meta = assistant
            .metadata
            .as_deref()
            .expect("assistant metadata should be set");
        let v: serde_json::Value = serde_json::from_str(meta).unwrap();
        let calls = v["tool_calls"].as_array().expect("tool_calls array");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["name"], "echo");
        assert_eq!(calls[0]["is_error"], false);
    }

    /// Spec test 2 of 7: a tool_use for a name not in the registry
    /// produces an `is_error=true` tool_result. The model is asked
    /// again, and the final stop_reason is `end_turn` (the model
    /// recovered from the unknown-tool error).
    #[tokio::test]
    async fn test_native_tool_loop_unknown_tool() {
        let turn1 = vec![
            agent::StreamEvent::ToolUseStart {
                id: "tu_1".into(),
                name: "no_such_tool".into(),
                input: serde_json::json!({}),
            },
            agent::StreamEvent::Done {
                stop_reason: agent::StopReason::ToolUse,
            },
        ];
        let turn2 = vec![
            agent::StreamEvent::TextDelta {
                text: "Couldn't find that tool.".into(),
            },
            agent::StreamEvent::Done {
                stop_reason: agent::StopReason::EndTurn,
            },
        ];

        let (db, registry, specialists, active, sse, tools, _pid, sid, _ac, tc) =
            setup_loop_test(vec![turn1, turn2], None);

        let _ = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &sid,
            "go",
        )
        .await
        .unwrap();
        wait_for_session_done(&active, &sid, std::time::Duration::from_secs(2)).await;
        assert_eq!(tc.load(std::sync::atomic::Ordering::SeqCst), 0);

        let messages = MessageStore::load_all(&db, &sid, 100).unwrap();
        let assistant = messages
            .iter()
            .find(|m| m.role == "assistant")
            .expect("assistant message should be persisted");
        let v: serde_json::Value =
            serde_json::from_str(assistant.metadata.as_deref().unwrap()).unwrap();
        let calls = v["tool_calls"].as_array().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["name"], "no_such_tool");
        assert_eq!(calls[0]["is_error"], true);
    }

    /// Spec test 3 of 7: a tool_use whose input fails JSON-schema
    /// validation is recorded as `is_error=true` with the validator's
    /// error message in the tool_result content. The model is then
    /// asked to continue.
    #[tokio::test]
    async fn test_native_tool_loop_validation_error() {
        let turn1 = vec![
            agent::StreamEvent::ToolUseStart {
                id: "tu_1".into(),
                // `q` is required but missing.
                name: "strict".into(),
                input: serde_json::json!({}),
            },
            agent::StreamEvent::Done {
                stop_reason: agent::StopReason::ToolUse,
            },
        ];
        let turn2 = vec![
            agent::StreamEvent::TextDelta {
                text: "OK, retrying with the right args.".into(),
            },
            agent::StreamEvent::Done {
                stop_reason: agent::StopReason::EndTurn,
            },
        ];

        let tool = ScriptedTool {
            name: "strict".into(),
            schema: serde_json::json!({
                "type": "object",
                "properties": { "q": { "type": "string" } },
                "required": ["q"]
            }),
            result: std::sync::Arc::new(std::sync::Mutex::new(Some(ToolResult {
                success: true,
                data: serde_json::json!(null),
                error: None,
            }))),
            call_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            sleep: std::time::Duration::ZERO,
        };

        let (db, registry, specialists, active, sse, tools, _pid, sid, _ac, tc) =
            setup_loop_test(vec![turn1, turn2], Some(tool));

        let _ = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &sid,
            "go",
        )
        .await
        .unwrap();
        wait_for_session_done(&active, &sid, std::time::Duration::from_secs(2)).await;
        // The tool was registered but the call was rejected at
        // validation, so the tool's `execute` must NOT have run.
        assert_eq!(tc.load(std::sync::atomic::Ordering::SeqCst), 0);

        let messages = MessageStore::load_all(&db, &sid, 100).unwrap();
        let assistant = messages
            .iter()
            .find(|m| m.role == "assistant")
            .expect("assistant message should be persisted");
        let v: serde_json::Value =
            serde_json::from_str(assistant.metadata.as_deref().unwrap()).unwrap();
        let calls = v["tool_calls"].as_array().unwrap();
        assert_eq!(calls[0]["is_error"], true);
    }

    /// Spec test 4 of 7: a tool whose `execute` returns
    /// `ToolResult { success: false, .. }` is recorded as
    /// `is_error=true` with the tool's error message. The model is
    /// then asked to continue, and the final turn is `end_turn`.
    #[tokio::test]
    async fn test_native_tool_loop_exec_error() {
        let turn1 = vec![
            agent::StreamEvent::ToolUseStart {
                id: "tu_1".into(),
                name: "flaky".into(),
                input: serde_json::json!({}),
            },
            agent::StreamEvent::Done {
                stop_reason: agent::StopReason::ToolUse,
            },
        ];
        let turn2 = vec![
            agent::StreamEvent::TextDelta {
                text: "The tool errored; falling back.".into(),
            },
            agent::StreamEvent::Done {
                stop_reason: agent::StopReason::EndTurn,
            },
        ];

        let tool = ScriptedTool {
            name: "flaky".into(),
            schema: serde_json::json!({"type": "object"}),
            result: std::sync::Arc::new(std::sync::Mutex::new(Some(ToolResult {
                success: false,
                data: serde_json::json!(null),
                error: Some("disk full".into()),
            }))),
            call_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            sleep: std::time::Duration::ZERO,
        };

        let (db, registry, specialists, active, sse, tools, _pid, sid, _ac, tc) =
            setup_loop_test(vec![turn1, turn2], Some(tool));

        let _ = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &sid,
            "go",
        )
        .await
        .unwrap();
        wait_for_session_done(&active, &sid, std::time::Duration::from_secs(2)).await;
        assert_eq!(tc.load(std::sync::atomic::Ordering::SeqCst), 1);

        let messages = MessageStore::load_all(&db, &sid, 100).unwrap();
        let assistant = messages
            .iter()
            .find(|m| m.role == "assistant")
            .expect("assistant message should be persisted");
        let v: serde_json::Value =
            serde_json::from_str(assistant.metadata.as_deref().unwrap()).unwrap();
        let calls = v["tool_calls"].as_array().unwrap();
        assert_eq!(calls[0]["is_error"], true);
    }

    /// Spec test 5 of 7: a model that keeps calling tools past
    /// `MAX_TOOL_ITERATIONS` is stopped with `StopReason::LoopLimit`
    /// and a final "Sorry, too many tool calls." assistant message.
    /// Note: we use a tiny cap by scripting the test to call
    /// `setup_loop_test` with `MAX_TOOL_ITERATIONS` worth of
    /// tool_use turns followed by an `end_turn`; the loop will
    /// exhaust after the configured cap and stop.
    #[tokio::test]
    async fn test_native_tool_loop_limit() {
        // We can't override MAX_TOOL_ITERATIONS from outside, so we
        // generate exactly MAX_TOOL_ITERATIONS+1 tool_use turns and
        // let the loop cut us off mid-sequence. The final state must
        // be LoopLimit, the persisted message must contain the
        // sorry-message, and the tool call count must equal the
        // configured cap.
        let mut scripts: Vec<Vec<agent::StreamEvent>> = Vec::new();
        for i in 0..(MAX_TOOL_ITERATIONS as usize + 1) {
            scripts.push(vec![
                agent::StreamEvent::ToolUseStart {
                    id: format!("tu_{i}"),
                    name: "ping".into(),
                    input: serde_json::json!({}),
                },
                agent::StreamEvent::Done {
                    stop_reason: agent::StopReason::ToolUse,
                },
            ]);
        }
        // Provide one more script for the (cut-off) iteration that
        // will *not* be reached.
        scripts.push(vec![agent::StreamEvent::Done {
            stop_reason: agent::StopReason::EndTurn,
        }]);

        let tool = ScriptedTool {
            name: "ping".into(),
            schema: serde_json::json!({"type": "object"}),
            result: std::sync::Arc::new(std::sync::Mutex::new(Some(ToolResult {
                success: true,
                data: serde_json::json!({"pong": true}),
                error: None,
            }))),
            call_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            sleep: std::time::Duration::ZERO,
        };

        let (db, registry, specialists, active, sse, tools, _pid, sid, ac, tc) =
            setup_loop_test(scripts, Some(tool));

        let _ = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &sid,
            "ping forever",
        )
        .await
        .unwrap();
        wait_for_session_done(&active, &sid, std::time::Duration::from_secs(5)).await;
        // The loop runs `for iteration in 0..MAX_TOOL_ITERATIONS` —
        // exactly MAX_TOOL_ITERATIONS iterations. Each iteration
        // calls the agent once; the cap detection runs *after* the
        // loop body and never consults the agent again. So the
        // agent is consulted exactly MAX_TOOL_ITERATIONS times, and
        // the tool is also called exactly MAX_TOOL_ITERATIONS times
        // (the +1'th script is never reached).
        assert_eq!(
            tc.load(std::sync::atomic::Ordering::SeqCst),
            MAX_TOOL_ITERATIONS as usize,
            "tool should be called exactly MAX_TOOL_ITERATIONS times"
        );
        assert_eq!(
            ac.load(std::sync::atomic::Ordering::SeqCst),
            MAX_TOOL_ITERATIONS as usize,
            "agent should be called exactly MAX_TOOL_ITERATIONS times"
        );

        let messages = MessageStore::load_all(&db, &sid, 100).unwrap();
        let assistant = messages
            .iter()
            .find(|m| m.role == "assistant")
            .expect("assistant message should be persisted");
        assert!(
            assistant.content.contains("too many tool calls"),
            "persisted message should contain the loop-limit apology; got: {}",
            assistant.content
        );
        let v: serde_json::Value =
            serde_json::from_str(assistant.metadata.as_deref().unwrap()).unwrap();
        assert_eq!(v["stop_reason"], "loop_limit");
    }

    /// Spec test 6 of 7: cancellation mid-loop drops the in-flight
    /// tool future without producing a synthetic `tool_result` for it.
    /// The persisted message preserves any assistant text already
    /// streamed; the session status is `cancelled`; the persisted
    /// `stop_reason` is `cancelled`.
    #[tokio::test]
    async fn test_native_tool_loop_cancellation() {
        // The mock tool sleeps long enough to be in-flight when
        // cancel fires. The mock agent emits a text delta + tool_use
        // and then waits.
        let turn1: Vec<agent::StreamEvent> = vec![
            agent::StreamEvent::TextDelta {
                text: "Before the long tool call. ".into(),
            },
            agent::StreamEvent::ToolUseStart {
                id: "tu_slow".into(),
                name: "slowpoke".into(),
                input: serde_json::json!({}),
            },
            agent::StreamEvent::Done {
                stop_reason: agent::StopReason::ToolUse,
            },
        ];

        let tool = ScriptedTool {
            name: "slowpoke".into(),
            schema: serde_json::json!({"type": "object"}),
            result: std::sync::Arc::new(std::sync::Mutex::new(Some(ToolResult {
                success: true,
                data: serde_json::json!(null),
                error: None,
            }))),
            call_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            sleep: std::time::Duration::from_secs(30),
        };

        let (db, registry, specialists, active, sse, tools, _pid, sid, _ac, _tc) =
            setup_loop_test(vec![turn1], Some(tool));

        let _ = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &sid,
            "go",
        )
        .await
        .unwrap();

        // Give the tool future time to start. Then cancel.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        SessionService::cancel_session(&active, &sid).expect("cancel should succeed");

        wait_for_session_done(&active, &sid, std::time::Duration::from_secs(2)).await;

        // The session should be in `cancelled` state, the assistant
        // text should be preserved, and the metadata should reflect
        // the cancelled stop reason.
        let session = crate::store::sessions::SessionStore::get_by_id(&db, &sid).unwrap();
        assert_eq!(session.status, "cancelled");
        let messages = MessageStore::load_all(&db, &sid, 100).unwrap();
        let assistant = messages
            .iter()
            .find(|m| m.role == "assistant")
            .expect("partial assistant text should be persisted");
        assert!(assistant.content.contains("Before the long tool call."));
        let v: serde_json::Value =
            serde_json::from_str(assistant.metadata.as_deref().unwrap()).unwrap();
        assert_eq!(v["stop_reason"], "cancelled");
        // No tool call was recorded because the tool_result was never
        // pushed back to the model.
        assert!(
            v.get("tool_calls").is_none() || v["tool_calls"].as_array().unwrap().is_empty(),
            "cancelled loop should not record a tool call, got: {v}"
        );
    }

    /// Spec test 7 of 7: a model that does not call any tool is a
    /// one-shot stream. The persisted message contains only the
    /// streamed text, no `tool_calls` in metadata, and the loop ran
    /// the agent exactly once.
    #[tokio::test]
    async fn test_native_tool_loop_no_tool_passes_through() {
        let turn1 = vec![
            agent::StreamEvent::TextDelta {
                text: "All done.".into(),
            },
            agent::StreamEvent::Done {
                stop_reason: agent::StopReason::EndTurn,
            },
        ];

        let (db, registry, specialists, active, sse, tools, _pid, sid, ac, _tc) =
            setup_loop_test(vec![turn1], None);

        let _ = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &sid,
            "go",
        )
        .await
        .unwrap();
        wait_for_session_done(&active, &sid, std::time::Duration::from_secs(2)).await;
        assert_eq!(ac.load(std::sync::atomic::Ordering::SeqCst), 1);

        let messages = MessageStore::load_all(&db, &sid, 100).unwrap();
        let assistant = messages
            .iter()
            .find(|m| m.role == "assistant")
            .expect("assistant message should be persisted");
        assert_eq!(assistant.content, "All done.");
        // The pre-feat-037 contract: a no-tool turn with `end_turn`
        // stores `None` in metadata (no empty `{}` rows).
        assert!(
            assistant.metadata.is_none(),
            "no-tool end_turn should not have metadata, got: {:?}",
            assistant.metadata
        );
    }

    /// Regression guard for the Journey "no information" bug: the
    /// native tool-execution loop must record `Decision` trace events
    /// from `Thinking` deltas and `ToolCall` + `FileChange` trace
    /// events from successful tool invocations. feat-037 deleted the
    /// pre-existing emissions without a follow-up; this test pins the
    /// behavior so a future refactor cannot silently break it again.
    #[tokio::test]
    async fn test_native_tool_loop_emits_journey_traces() {
        // Turn 1: model thinks, then calls `fs_write`. Turn 2: model
        // wraps up. The two `Thinking` deltas must coalesce into a
        // single `Decision`; the `ToolUseStart` is the boundary that
        // triggers the flush.
        let turn1 = vec![
            agent::StreamEvent::Thinking {
                text: "I should".into(),
            },
            agent::StreamEvent::Thinking {
                text: " write the file.".into(),
            },
            agent::StreamEvent::ToolUseStart {
                id: "tu_1".into(),
                name: "fs_write".into(),
                input: serde_json::json!({
                    "path": "/tmp/journey_trace_test.rs",
                    "content": "fn main() {}"
                }),
            },
            agent::StreamEvent::Done {
                stop_reason: agent::StopReason::ToolUse,
            },
        ];
        let turn2 = vec![agent::StreamEvent::Done {
            stop_reason: agent::StopReason::EndTurn,
        }];

        // The `fs_write` tool is registered with a schema that matches
        // the `extract_file_changes` heuristic (path present, type
        // string). It does not actually need to write the file —
        // the trace emission happens before file IO succeeds.
        let tool = ScriptedTool {
            name: "fs_write".into(),
            schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["path", "content"]
            }),
            result: std::sync::Arc::new(std::sync::Mutex::new(Some(ToolResult {
                success: true,
                data: serde_json::json!({"ok": true}),
                error: None,
            }))),
            call_count: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            sleep: std::time::Duration::ZERO,
        };

        let (db, registry, specialists, active, sse, tools, _pid, sid, _ac, _tc) =
            setup_loop_test(vec![turn1, turn2], Some(tool));

        let _ = SessionService::send_prompt(
            &db,
            &registry,
            &specialists,
            &active,
            &sse,
            &tools,
            &sid,
            "go",
        )
        .await
        .unwrap();

        wait_for_session_done(&active, &sid, std::time::Duration::from_secs(2)).await;

        // (1) The full trace list must contain the Decision and ToolCall
        // rows that the loop emitted, in the right order.
        let traces = TraceStore::list_by_session(&db, &sid).unwrap();
        let kinds: Vec<&str> = traces.iter().map(|t| t.event_type.as_str()).collect();
        assert!(
            kinds.contains(&"decision"),
            "expected a Decision trace from Thinking deltas, got: {kinds:?}"
        );
        assert!(
            kinds.contains(&"tool_call"),
            "expected a ToolCall trace from the fs_write invocation, got: {kinds:?}"
        );

        // The Decision text must be the coalesced Thinking buffer, not
        // just the first delta. Order matters: the `ToolUseStart`
        // boundary should have flushed BEFORE the ToolCall row was
        // written.
        let decision_idx = kinds.iter().position(|k| *k == "decision").unwrap();
        let tool_call_idx = kinds.iter().position(|k| *k == "tool_call").unwrap();
        assert!(
            decision_idx < tool_call_idx,
            "Decision (thinking) must be emitted before ToolCall (boundary flush), got order: {kinds:?}"
        );
        let decision_text = traces[decision_idx].summary.clone();
        assert!(
            decision_text.contains("write the file"),
            "Decision should contain the coalesced thinking text, got: {decision_text:?}"
        );

        // (2) The journey endpoint filters to Decision + Error. The
        // Decision must show up there so the Journey sidebar renders it.
        let journey = TraceStore::list_journey(&db, &sid).unwrap();
        assert!(
            journey.iter().any(|t| t.event_type == "decision"),
            "journey endpoint should include the Decision trace"
        );

        // (3) The file_changes endpoint must have a row for /tmp/...
        let changes = TraceStore::list_file_changes(&db, &sid).unwrap();
        let paths: Vec<&str> = changes.iter().map(|c| c.path.as_str()).collect();
        assert!(
            paths.contains(&"/tmp/journey_trace_test.rs"),
            "expected a FileChange for the fs_write path, got: {paths:?}"
        );

        // (4) The persisted ToolCall row must carry the tool name and
        // the JSON-serialized input so the UI can show the call shape.
        let tool_call_row = traces
            .iter()
            .find(|t| t.event_type == "tool_call")
            .expect("ToolCall row exists (asserted above)");
        let data_json = tool_call_row
            .data_json
            .as_deref()
            .expect("ToolCall row should have data_json");
        let data: serde_json::Value = serde_json::from_str(data_json).unwrap();
        assert_eq!(data["tool_name"], "fs_write");
        assert_eq!(data["input"]["path"], "/tmp/journey_trace_test.rs");
    }

    // --- Session resume tests (feat-018) ---

    /// Helper: create a session via the service layer.
    fn create_session_via_service(
        db: &Db,
        ws_id: &str,
        provider_id: &str,
        parent_session_id: Option<&str>,
    ) -> crate::store::sessions::Session {
        SessionService::create_session(
            db,
            ws_id,
            provider_id,
            None,
            None,
            None,
            parent_session_id,
            None,
            None, // codebase_id
            None, // runtime_kind — use default
            None, // mode — use default
            None, // runtime_metadata_json — none
        )
        .unwrap()
    }

    /// Helper: transition a session to "completed" so it can be used as a resume parent.
    fn complete_session(db: &Db, session_id: &str) {
        SessionStore::update_status(db, session_id, "completed").unwrap();
    }

    #[test]
    fn test_session_resume() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // Create parent with 3 messages, then complete it
        let parent = create_session_via_service(&db, &ws_id, &provider_id, None);
        MessageStore::create(&db, &parent.id, "user", "hello", None).unwrap();
        MessageStore::create(&db, &parent.id, "assistant", "hi there", None).unwrap();
        MessageStore::create(&db, &parent.id, "user", "how are you?", None).unwrap();
        complete_session(&db, &parent.id);

        // Create child resuming from parent
        let child = create_session_via_service(&db, &ws_id, &provider_id, Some(&parent.id));

        // Verify child has parent reference
        assert_eq!(child.parent_session_id.as_deref(), Some(parent.id.as_str()));

        // Verify messages were copied (count and content, not order — UUIDs are random)
        let msgs = MessageStore::load_all(&db, &child.id, 1000).unwrap();
        assert_eq!(msgs.len(), 3, "child should have 3 copied messages");

        let contents: Vec<&str> = msgs.iter().map(|m| m.content.as_str()).collect();
        assert!(contents.contains(&"hello"), "should contain 'hello'");
        assert!(contents.contains(&"hi there"), "should contain 'hi there'");
        assert!(
            contents.contains(&"how are you?"),
            "should contain 'how are you?'"
        );

        // Verify roles are preserved
        let roles: Vec<&str> = msgs.iter().map(|m| m.role.as_str()).collect();
        assert_eq!(roles.iter().filter(|r| **r == "user").count(), 2);
        assert_eq!(roles.iter().filter(|r| **r == "assistant").count(), 1);

        // Verify all messages belong to child session
        for msg in &msgs {
            assert_eq!(msg.session_id, child.id);
        }

        // Verify parent messages are unchanged
        let parent_msgs = MessageStore::load_all(&db, &parent.id, 1000).unwrap();
        assert_eq!(parent_msgs.len(), 3, "parent should still have 3 messages");
    }

    #[test]
    fn test_session_resume_chain() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // Create grandparent with 2 messages, then complete it
        let grandparent = create_session_via_service(&db, &ws_id, &provider_id, None);
        MessageStore::create(&db, &grandparent.id, "user", "first", None).unwrap();
        MessageStore::create(&db, &grandparent.id, "assistant", "second", None).unwrap();
        complete_session(&db, &grandparent.id);

        // Create parent resuming from grandparent — gets grandparent's 2 messages copied
        let parent = create_session_via_service(&db, &ws_id, &provider_id, Some(&grandparent.id));
        // Add 3 more messages to parent
        MessageStore::create(&db, &parent.id, "user", "third", None).unwrap();
        MessageStore::create(&db, &parent.id, "assistant", "fourth", None).unwrap();
        MessageStore::create(&db, &parent.id, "user", "fifth", None).unwrap();

        // Parent should have 5 messages (2 copied + 3 new)
        let parent_msgs = MessageStore::load_all(&db, &parent.id, 1000).unwrap();
        assert_eq!(parent_msgs.len(), 5, "parent should have 5 messages");
        complete_session(&db, &parent.id);

        // Create child resuming from parent — gets parent's 5 messages copied
        let child = create_session_via_service(&db, &ws_id, &provider_id, Some(&parent.id));

        // Child should have all 5 messages from parent (which include grandparent's)
        let msgs = MessageStore::load_all(&db, &child.id, 1000).unwrap();
        assert_eq!(
            msgs.len(),
            5,
            "child should have 5 messages (parent's full history)"
        );

        // Verify all expected content is present
        let contents: Vec<&str> = msgs.iter().map(|m| m.content.as_str()).collect();
        for expected in &["first", "second", "third", "fourth", "fifth"] {
            assert!(contents.contains(expected), "should contain '{}'", expected);
        }
    }

    #[test]
    fn test_session_resume_no_parent() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // Create session without parent — should work normally
        let session = create_session_via_service(&db, &ws_id, &provider_id, None);
        assert!(session.parent_session_id.is_none());

        let msgs = MessageStore::load_all(&db, &session.id, 1000).unwrap();
        assert_eq!(msgs.len(), 0, "non-resumed session should have no messages");
    }

    #[test]
    fn test_session_resume_parent_not_found() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        let result = SessionService::create_session(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            Some("nonexistent-session-id"),
            None,
            None, // codebase_id
            None, // runtime_kind
            None, // mode
            None, // runtime_metadata_json
        );

        match result {
            Err(AppError::NotFound { resource, .. }) => {
                assert_eq!(resource, "session");
            }
            other => panic!("expected NotFound, got: {:?}", other),
        }
    }

    #[test]
    fn test_session_resume_wrong_workspace() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // Create parent in default workspace
        let parent = create_session_via_service(&db, &ws_id, &provider_id, None);
        complete_session(&db, &parent.id);

        // Create a second workspace
        let ws2 = crate::store::workspaces::WorkspaceStore::create(&db, "other-ws").unwrap();

        // Try to resume from parent in different workspace
        let result = SessionService::create_session(
            &db,
            &ws2.id,
            &provider_id,
            None,
            None,
            None,
            Some(&parent.id),
            None,
            None, // codebase_id
            None, // runtime_kind
            None, // mode
            None, // runtime_metadata_json
        );

        match result {
            Err(AppError::Validation { message: msg, .. }) => {
                assert!(
                    msg.contains("different workspace"),
                    "unexpected message: {}",
                    msg
                );
            }
            other => panic!("expected Validation, got: {:?}", other),
        }
    }

    #[test]
    fn test_session_resume_depth_limit() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // Create a chain of MAX_RESUME_DEPTH + 1 sessions (6 sessions, 5 hops)
        let mut sessions = Vec::new();
        let s1 = create_session_via_service(&db, &ws_id, &provider_id, None);
        sessions.push(s1);

        for i in 1..=MAX_RESUME_DEPTH {
            let parent = &sessions[i - 1];
            complete_session(&db, &parent.id);
            let s = create_session_via_service(&db, &ws_id, &provider_id, Some(&parent.id));
            sessions.push(s);
        }

        // The 6th session (MAX_RESUME_DEPTH=5, chain of 6 = 5 hops) should succeed
        assert_eq!(sessions.len(), MAX_RESUME_DEPTH + 1);

        // Creating one more (6 hops) should fail
        let deepest = &sessions[sessions.len() - 1];
        complete_session(&db, &deepest.id);
        let result = SessionService::create_session(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            Some(&deepest.id),
            None,
            None, // codebase_id
            None, // runtime_kind
            None, // mode
            None, // runtime_metadata_json
        );

        match result {
            Err(AppError::Validation { message: msg, .. }) => {
                assert!(
                    msg.contains("depth") || msg.contains("exceeds"),
                    "unexpected message: {}",
                    msg
                );
            }
            other => panic!("expected Validation for depth limit, got: {:?}", other),
        }
    }

    #[test]
    fn test_session_resume_cycle() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // Create A -> B chain
        let a = create_session_via_service(&db, &ws_id, &provider_id, None);
        complete_session(&db, &a.id);
        let b = create_session_via_service(&db, &ws_id, &provider_id, Some(&a.id));

        // Manually create a cycle: set A's parent to B
        db.conn()
            .execute(
                "UPDATE sessions SET parent_session_id = ?1 WHERE id = ?2",
                rusqlite::params![b.id, a.id],
            )
            .unwrap();

        // Try to resume from B — should detect cycle (B -> A -> B)
        complete_session(&db, &b.id);
        let result = SessionService::create_session(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            Some(&b.id),
            None,
            None, // codebase_id
            None, // runtime_kind
            None, // mode
            None, // runtime_metadata_json
        );

        match result {
            Err(AppError::Validation { message: msg, .. }) => {
                assert!(msg.contains("cycle"), "unexpected message: {}", msg);
            }
            other => panic!("expected Validation for cycle, got: {:?}", other),
        }
    }

    #[test]
    fn test_session_resume_empty_parent() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // Create parent with no messages
        let parent = create_session_via_service(&db, &ws_id, &provider_id, None);
        complete_session(&db, &parent.id);

        // Resume from empty parent
        let child = create_session_via_service(&db, &ws_id, &provider_id, Some(&parent.id));

        let msgs = MessageStore::load_all(&db, &child.id, 1000).unwrap();
        assert_eq!(
            msgs.len(),
            0,
            "child of empty parent should have no messages"
        );
    }

    #[test]
    fn test_session_resume_active_parent_rejected() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // Create parent but leave it in "connecting" status (not terminal)
        let parent = create_session_via_service(&db, &ws_id, &provider_id, None);
        MessageStore::create(&db, &parent.id, "user", "hello", None).unwrap();

        // Try to resume from active parent — should fail
        let result = SessionService::create_session(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            Some(&parent.id),
            None,
            None, // codebase_id
            None, // runtime_kind
            None, // mode
            None, // runtime_metadata_json
        );

        match result {
            Err(AppError::Validation { message: msg, .. }) => {
                assert!(msg.contains("connecting"), "unexpected message: {}", msg);
            }
            other => panic!("expected Validation for active parent, got: {:?}", other),
        }
    }

    // --- Codebase binding tests (feat-062) ---

    /// `codebase_id` resolves to the codebase's path: the session's
    /// `cwd` is set to the codebase's path, regardless of any
    /// supplied `cwd` arg. The codebase binding wins.
    #[test]
    fn test_create_session_with_codebase_id_copies_path_to_cwd() {
        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let ws_id = crate::store::workspaces::WorkspaceStore::list(&db, None, 50)
            .unwrap()
            .data
            .remove(0)
            .id;
        let provider_id = crate::store::providers::ProviderStore::create(
            &db,
            "anthropic",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k"}"#,
        )
        .unwrap()
        .id;

        let tmp = tempfile::TempDir::new().unwrap();
        let codebase = crate::store::codebases::CodebaseStore::create(
            &db,
            &ws_id,
            tmp.path().to_str().unwrap(),
            None,
            Some("bind-target"),
        )
        .unwrap();

        // Pass an explicit `cwd` AND a `codebase_id`. The binding wins
        // and overrides the cwd.
        let session = SessionService::create_session(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            Some("/some/other/path"),
            None,
            None,
            Some(&codebase.id),
            None, // runtime_kind
            None, // mode
            None, // runtime_metadata_json
        )
        .unwrap();

        assert_eq!(session.codebase_id.as_deref(), Some(codebase.id.as_str()));
        assert_eq!(session.cwd.as_deref(), Some(tmp.path().to_str().unwrap()));
    }

    /// An unknown `codebase_id` is rejected before the session is
    /// created. The session row never lands in the DB.
    #[test]
    fn test_create_session_invalid_codebase_id_rejected() {
        let db = test_db();
        crate::store::workspaces::WorkspaceStore::ensure_default(&db).unwrap();
        let ws_id = crate::store::workspaces::WorkspaceStore::list(&db, None, 50)
            .unwrap()
            .data
            .remove(0)
            .id;
        let provider_id = crate::store::providers::ProviderStore::create(
            &db,
            "anthropic",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k"}"#,
        )
        .unwrap()
        .id;

        let result = SessionService::create_session(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            Some("nonexistent-codebase-id"),
            None, // runtime_kind
            None, // mode
            None, // runtime_metadata_json
        );

        match result {
            Err(AppError::NotFound { resource, id }) => {
                assert_eq!(resource, "codebase");
                assert_eq!(id, "nonexistent-codebase-id");
            }
            other => panic!(
                "expected NotFound for invalid codebase_id, got: {:?}",
                other
            ),
        }
    }

    /// Cross-workspace codebase binding is rejected: a codebase in
    /// workspace A cannot be attached to a session in workspace B.
    /// The defense lives in `CodebaseStore::get_in_workspace`, called
    /// from `SessionService::create_session`.
    #[test]
    fn test_create_session_cross_workspace_codebase_rejected() {
        let db = test_db();
        // Two workspaces
        let ws_a = crate::store::workspaces::WorkspaceStore::create(&db, "ws-a").unwrap();
        let ws_b = crate::store::workspaces::WorkspaceStore::create(&db, "ws-b").unwrap();
        let provider_id = crate::store::providers::ProviderStore::create(
            &db,
            "anthropic",
            "test",
            r#"{"base_url":"http://localhost","api_key":"k"}"#,
        )
        .unwrap()
        .id;

        let tmp = tempfile::TempDir::new().unwrap();
        // Codebase registered under workspace A
        let codebase = crate::store::codebases::CodebaseStore::create(
            &db,
            &ws_a.id,
            tmp.path().to_str().unwrap(),
            None,
            Some("ws-a-codebase"),
        )
        .unwrap();

        // Try to bind it to a session in workspace B
        let result = SessionService::create_session(
            &db,
            &ws_b.id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            Some(&codebase.id),
            None,
            None,
            None,
        );

        match result {
            Err(AppError::NotFound { resource, id }) => {
                assert_eq!(resource, "codebase");
                assert_eq!(id, codebase.id);
            }
            other => panic!(
                "expected NotFound for cross-workspace codebase, got: {:?}",
                other
            ),
        }
    }

    // --- feat-038: session runtime_kind / mode / runtime_metadata_json ---

    /// Migration 011 added the three runtime columns. Verify the
    /// post-migration `sessions` table is queryable and the columns are
    /// present with the documented defaults.
    #[test]
    fn test_session_runtime_kind_migration() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // After migrations, inserting the minimum 9 columns + reading
        // back must succeed. The store writes the 3 new columns as part
        // of its 12-arg signature, so this exercises the full path.
        let session = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,                    // codebase_id
            RuntimeKind::ClaudeCode, // exercise non-default runtime
            SessionMode::Wrapped,    // exercise non-default mode
            None,                    // no metadata
        )
        .expect("create with non-default runtime/mode");

        // The DB row carries the explicit values, not the column
        // defaults. Migration 011's DEFAULTs are only seen by pre-011
        // rows that got backfilled — not by anything inserted after.
        assert_eq!(session.runtime_kind, RuntimeKind::ClaudeCode);
        assert_eq!(session.mode, SessionMode::Wrapped);
        assert_eq!(session.runtime_metadata_json, None);

        // The columns must be readable directly via raw SQL. This
        // proves migration 011 actually ran (the columns would not
        // exist otherwise) and the column names match the migration.
        let conn = db.conn();
        let runtime_kind_db: String = conn
            .query_row(
                "SELECT runtime_kind FROM sessions WHERE id = ?1",
                rusqlite::params![session.id],
                |r| r.get(0),
            )
            .expect("query runtime_kind");
        let mode_db: String = conn
            .query_row(
                "SELECT mode FROM sessions WHERE id = ?1",
                rusqlite::params![session.id],
                |r| r.get(0),
            )
            .expect("query mode");
        assert_eq!(runtime_kind_db, "claude-code");
        assert_eq!(mode_db, "wrapped");
    }

    /// Persisting a `runtime_metadata_json` string and reading it back
    /// returns the exact bytes. The column is opaque JSON, so the store
    /// does no validation — roundtrip fidelity is the only contract.
    #[test]
    fn test_session_runtime_metadata_roundtrip() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // A realistic Claude-Code resume payload: a CLI session id plus
        // a few runtime knobs. Any JSON object should roundtrip verbatim
        // — the store does not parse or re-serialize it.
        let metadata = r#"{"cli_session_id":"sess_abc123","cwd":"/tmp/repo"}"#;
        let session = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::ClaudeCode,
            SessionMode::Wrapped,
            Some(metadata),
        )
        .expect("create with metadata");

        assert_eq!(session.runtime_metadata_json.as_deref(), Some(metadata));

        // Read it back through a fresh query path (list_by_workspace)
        // to confirm the column is selected, not just stored.
        let listed = SessionStore::list_by_workspace(&db, &ws_id, None, 100)
            .expect("list sessions")
            .data;
        let found = listed
            .iter()
            .find(|s| s.id == session.id)
            .expect("session present in workspace listing");
        assert_eq!(
            found.runtime_metadata_json.as_deref(),
            Some(metadata),
            "list_by_workspace must surface runtime_metadata_json"
        );
    }

    /// `create_session` called without specifying any of the three
    /// runtime fields must persist the platform defaults
    /// (`anthropic-api` + `native`) and a `NULL` metadata column.
    /// This is the contract for the pre-feat-038 API surface — the
    /// new fields are fully backward compatible.
    #[test]
    fn test_session_runtime_default_backfill() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // Call through the service layer (not the raw store) so we
        // exercise the full default-resolution path. All three
        // runtime fields are None — same as the pre-feat-038 callers.
        let session = SessionService::create_session(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None, // no parent (not a resume)
            None,
            None, // codebase_id
            None, // runtime_kind — use default
            None, // mode — use default
            None, // runtime_metadata_json — none
        )
        .expect("create with all defaults");

        // Platform defaults — see `RuntimeKind::default()` /
        // `SessionMode::default()`. The defaults match migration 011's
        // column DEFAULTs, so a row inserted post-migration is
        // indistinguishable from a backfilled pre-migration row.
        assert_eq!(session.runtime_kind, RuntimeKind::AnthropicApi);
        assert_eq!(session.mode, SessionMode::Native);
        assert_eq!(session.runtime_metadata_json, None);
    }

    /// Resuming a `claude-code` session without passing metadata must
    /// inherit the parent's `cli_session_id`. The metadata is
    /// meaningful only on the same runtime.
    #[test]
    fn test_session_resume_inherits_metadata_same_runtime() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        // Parent: a finished claude-code session with metadata.
        let parent = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None, // codebase_id
            RuntimeKind::ClaudeCode,
            SessionMode::Wrapped,
            Some(r#"{"cli_session_id":"parent-id"}"#),
        )
        .expect("create parent");
        complete_session(&db, &parent.id);

        // Child: resume on the same runtime, no explicit metadata.
        let child = SessionService::create_session(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            Some(&parent.id), // resume
            None,
            None,                // codebase_id
            Some("claude-code"), // same runtime as parent
            Some("wrapped"),     // same mode
            None,                // no explicit metadata — must inherit
        )
        .expect("resume on same runtime");

        assert_eq!(child.runtime_kind, RuntimeKind::ClaudeCode);
        assert_eq!(child.mode, SessionMode::Wrapped);
        assert_eq!(
            child.runtime_metadata_json.as_deref(),
            Some(r#"{"cli_session_id":"parent-id"}"#),
            "metadata must be inherited on same-runtime resume"
        );
    }

    /// Resuming on a different runtime must clear the parent's
    /// metadata. A `claude-code` cli_session_id is meaningless to an
    /// `anthropic-api` child.
    #[test]
    fn test_session_resume_clears_metadata_on_runtime_switch() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        let parent = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
            RuntimeKind::ClaudeCode,
            SessionMode::Wrapped,
            Some(r#"{"cli_session_id":"parent-id"}"#),
        )
        .expect("create parent");
        complete_session(&db, &parent.id);

        // Child: resume on a different runtime, no explicit metadata.
        let child = SessionService::create_session(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            Some(&parent.id),
            None,
            None,
            Some("anthropic-api"), // different runtime
            None,                  // default mode
            None,                  // no explicit metadata
        )
        .expect("resume on different runtime");

        assert_eq!(child.runtime_kind, RuntimeKind::AnthropicApi);
        assert_eq!(
            child.runtime_metadata_json, None,
            "metadata must be cleared when runtime changes"
        );
    }

    /// An explicit `runtime_metadata_json` on a resume must always win
    /// over the parent's metadata — the caller is asserting "I know
    /// what I'm doing, use this verbatim." This holds even on
    /// same-runtime resumes.
    #[test]
    fn test_session_resume_explicit_metadata_wins() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        let parent = SessionStore::create(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
            RuntimeKind::ClaudeCode,
            SessionMode::Wrapped,
            Some(r#"{"cli_session_id":"parent-id"}"#),
        )
        .expect("create parent");
        complete_session(&db, &parent.id);

        let override_metadata = r#"{"cli_session_id":"override-id"}"#;
        let child = SessionService::create_session(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            Some(&parent.id),
            None,
            None,
            Some("claude-code"),
            Some("wrapped"),
            Some(override_metadata), // explicit override
        )
        .expect("resume with explicit metadata");

        assert_eq!(
            child.runtime_metadata_json.as_deref(),
            Some(override_metadata),
            "explicit metadata must override parent"
        );
    }

    /// An invalid `runtime_kind` string is a 400-level validation
    /// error, surfaced before any DB work. The same applies to `mode`.
    #[test]
    fn test_session_runtime_invalid_value_rejected() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        let result = SessionService::create_session(
            &db,
            &ws_id,
            &provider_id,
            None,
            None,
            None,
            None,
            None,
            None,
            Some("not-a-real-runtime"),
            None,
            None,
        );

        match result {
            Err(AppError::Validation { message: msg, .. }) => {
                assert!(
                    msg.contains("not-a-real-runtime"),
                    "error message must echo the bad value, got: {msg}"
                );
            }
            other => panic!("expected Validation error, got: {:?}", other),
        }
    }

    /// feat-040: the runtime_kind × mode compatibility validator fires
    /// at the chokepoint. A `try_automate_lane` caller that resolves to
    /// a (CLI runtime, HTTP-only mode) pair — which feat-055's column
    /// binding will be able to produce once columns carry
    /// `runtime_kind` — must be rejected with the
    /// `"runtime_mode_incompatible"` code, listing the runtime, the
    /// requested mode, and the modes that runtime does support.
    ///
    /// Today `try_automate_lane` passes `None`/`None` and the defaults
    /// (AnthropicApi, Native) are accepted, so the only way to exercise
    /// the rejection path from a chokepoint-level test is to call
    /// `create_session` directly with an explicit incompatible pair —
    /// which is exactly what this test does.
    #[test]
    fn test_kanban_autospawn_rejects_incompatible_pair() {
        let db = test_db();
        let (ws_id, provider_id) = seed_deps(&db);

        let result = SessionService::create_session(
            &db,
            &ws_id,
            &provider_id,
            None,                // specialist_id
            None,                // model
            None,                // cwd
            None,                // parent_session_id
            None,                // context_id
            None,                // codebase_id
            Some("claude-code"), // runtime_kind — CLI
            Some("native"),      // mode — HTTP-only
            None,                // runtime_metadata_json
        );

        match result {
            Err(AppError::Validation { code, message }) => {
                assert_eq!(code, "runtime_mode_incompatible");
                assert!(message.contains("claude-code"), "msg: {message}");
                assert!(message.contains("native"), "msg: {message}");
                assert!(message.contains("wrapped"), "msg: {message}");
            }
            other => panic!("expected Validation error, got: {:?}", other),
        }
    }

    // -----------------------------------------------------------------------
    // feat-041: TurnContext verification
    // -----------------------------------------------------------------------

    /// `CodingAgent::send_message` receives the `TurnContext` built by
    /// `run_prompt_task` intact. We bypass `run_prompt_task` (which
    /// requires a registered provider + active SSE manager) and call the
    /// trait method directly through `CapturingAgent`, asserting the
    /// `cwd`, `codebase_root`, `session_id`, and `workspace_id` fields
    /// match the values we constructed.
    #[tokio::test]
    async fn test_turn_context_passes_cwd_and_codebase() {
        use crate::agent::turn_context::TurnContext;
        use crate::agent::CodingAgent;
        use std::path::PathBuf;
        use tokio_util::sync::CancellationToken;

        let agent = CapturingAgent::new();
        let agent_arc: Arc<dyn CodingAgent> = Arc::new(CapturingAgent {
            captured: Arc::clone(&agent.captured),
            captured_turn: Arc::clone(&agent.captured_turn),
        });

        let cancel = CancellationToken::new();
        let turn_ctx = TurnContext {
            session_id: "s-passes".to_string(),
            workspace_id: "w-passes".to_string(),
            cwd: PathBuf::from("/tmp/agent-cwd"),
            codebase_root: Some(PathBuf::from("/tmp/agent-cwd")),
            cli_resume_id: Some("cli-resume-1".to_string()),
            runtime_kind: crate::agent::RuntimeKind::AnthropicApi,
            cancellation_token: cancel.clone(),
        };

        let req = MessageRequest {
            model: "m".into(),
            messages: vec![],
            system: None,
            max_tokens: 1,
            tools: None,
        };

        // Drive the call. CapturingAgent returns a stream that emits
        // `Done(EndTurn)` then closes; we drain it to be polite.
        let stream = agent_arc
            .send_message(req, &turn_ctx)
            .await
            .expect("CapturingAgent returns Ok");
        let mut stream = stream;
        use futures_util::StreamExt;
        while stream.next().await.is_some() {}

        // The captured TurnContext must match the one we passed in.
        let captured_turn = agent
            .captured_turn
            .lock()
            .unwrap()
            .clone()
            .expect("CapturingAgent should have stored the turn");
        assert_eq!(captured_turn.session_id, "s-passes");
        assert_eq!(captured_turn.workspace_id, "w-passes");
        assert_eq!(captured_turn.cwd, PathBuf::from("/tmp/agent-cwd"));
        assert_eq!(
            captured_turn.codebase_root,
            Some(PathBuf::from("/tmp/agent-cwd"))
        );
        assert_eq!(captured_turn.cli_resume_id.as_deref(), Some("cli-resume-1"));
        assert_eq!(
            captured_turn.runtime_kind,
            crate::agent::RuntimeKind::AnthropicApi
        );
        assert!(!captured_turn.cancellation_token.is_cancelled());

        // Cancellation propagates from the original token (Arc-shared).
        cancel.cancel();
        assert!(captured_turn.cancellation_token.is_cancelled());
    }

    /// `SessionService` populates the per-turn context from the loaded
    /// `Session` row: `cwd` and `codebase_root` are derived from
    /// `session.cwd` and `session.codebase_id`; `cli_resume_id` is
    /// parsed from `session.runtime_metadata_json` when present and
    /// `None` otherwise (silent fallback on parse failure —
    /// opportunistic metadata, not a required key). We exercise the
    /// derivation logic directly here rather than spinning up the full
    /// `send_prompt` task (which needs a registered provider, active
    /// SSE manager, and live tools).
    #[test]
    fn test_session_service_passes_turn_context() {
        use std::path::PathBuf;
        use tokio_util::sync::CancellationToken;

        // Calls the production `build_turn_context` builder. The cwd
        // canonicalization (`session.cwd` or ".") is owned by
        // `run_prompt_task`; the test passes a pre-computed `PathBuf`
        // matching what the call site would supply.
        let cancel = CancellationToken::new();
        let build = |session: &crate::store::sessions::Session, cwd: PathBuf| {
            super::build_turn_context(session, cwd, cancel.clone())
        };

        // Case 1: bound session, no metadata — `codebase_root` is Some,
        // `cli_resume_id` is None.
        let mut session = crate::store::sessions::Session {
            id: "s-bound".into(),
            workspace_id: "w-1".into(),
            provider_id: "p-1".into(),
            specialist_id: None,
            parent_session_id: None,
            context_id: None,
            status: "ready".into(),
            model: None,
            cwd: Some("/tmp/proj".into()),
            codebase_id: Some("cb-1".into()),
            runtime_kind: crate::agent::RuntimeKind::AnthropicApi,
            mode: crate::agent::SessionMode::Native,
            runtime_metadata_json: None,
            created_at: "2026-06-10T00:00:00Z".into(),
            updated_at: "2026-06-10T00:00:00Z".into(),
        };
        let ctx = build(&session, PathBuf::from("/tmp/proj"));
        assert_eq!(ctx.session_id, "s-bound");
        assert_eq!(ctx.cwd, PathBuf::from("/tmp/proj"));
        assert_eq!(ctx.codebase_root, Some(PathBuf::from("/tmp/proj")));
        assert_eq!(ctx.cli_resume_id, None);
        assert_eq!(ctx.runtime_kind, crate::agent::RuntimeKind::AnthropicApi);

        // Case 2: unbound session (HTTP), metadata present and well-formed
        // — `codebase_root` is None, `cli_resume_id` is Some.
        session.codebase_id = None;
        session.runtime_metadata_json = Some(r#"{"cli_resume_id":"res-abc"}"#.into());
        let ctx = build(&session, PathBuf::from("/tmp/proj"));
        assert_eq!(ctx.codebase_root, None);
        assert_eq!(ctx.cli_resume_id.as_deref(), Some("res-abc"));

        // Case 3: malformed JSON — `cli_resume_id` is None (silent).
        session.runtime_metadata_json = Some("not-json".into());
        let ctx = build(&session, PathBuf::from("/tmp/proj"));
        assert_eq!(ctx.cli_resume_id, None);

        // Case 4: well-formed JSON, no `cli_resume_id` key — None.
        session.runtime_metadata_json = Some(r#"{"other_key":"v"}"#.into());
        let ctx = build(&session, PathBuf::from("/tmp/proj"));
        assert_eq!(ctx.cli_resume_id, None);

        // Case 5: session with no cwd — call site would pass "."; the
        // builder trusts the caller's canonicalization.
        session.cwd = None;
        session.codebase_id = None;
        session.runtime_metadata_json = None;
        let ctx = build(&session, PathBuf::from("."));
        assert_eq!(ctx.cwd, PathBuf::from("."));
        assert_eq!(ctx.codebase_root, None);
    }
}
