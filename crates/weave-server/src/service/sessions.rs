use std::collections::HashSet;
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::agent::registry::ProviderRegistry;
#[allow(unused_imports)] // MessageRequest is used by the `tests` submodule.
use crate::agent::{self, MessageRequest};
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
    pub fn create_session(
        db: &Db,
        workspace_id: &str,
        provider_id: &str,
        specialist_id: Option<&str>,
        model: Option<&str>,
        cwd: Option<&str>,
        parent_session_id: Option<&str>,
        context_id: Option<&str>,
    ) -> Result<crate::store::sessions::Session, AppError> {
        // Validate workspace exists
        crate::store::workspaces::WorkspaceStore::get_by_id(db, workspace_id)?;

        // Validate parent chain and load direct parent's messages
        let parent_messages = if let Some(pid) = parent_session_id {
            // Ensure parent has finished — resuming an active session would
            // copy an incomplete message history.
            let parent = SessionStore::get_by_id(db, pid)?;
            if !TERMINAL.contains(&parent.status.as_str()) {
                return Err(AppError::Validation(format!(
                    "cannot resume from session in '{}' status — parent must be completed, \
                     cancelled, or error",
                    parent.status
                )));
            }
            validate_parent_chain(db, pid, workspace_id)?;
            MessageStore::load_all(db, pid, MAX_HISTORY_MESSAGES)?
        } else {
            Vec::new()
        };

        // Atomically create session + copy messages
        db.with_transaction(|conn| {
            let session = SessionStore::create_tx(
                conn,
                workspace_id,
                provider_id,
                specialist_id,
                model,
                cwd,
                parent_session_id,
                context_id,
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
            return Err(AppError::Validation("prompt cannot be empty".into()));
        }

        // Validate session exists and is in a non-terminal state
        let session = SessionStore::get_by_id(db, session_id)?;
        if crate::store::sessions::TERMINAL.contains(&session.status.as_str()) {
            return Err(AppError::Validation(format!(
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
            None => Err(AppError::Validation(
                "session is not actively streaming".into(),
            )),
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
            return Err(AppError::Validation(
                "parent session belongs to a different workspace".into(),
            ));
        }

        // Walk to parent if present
        if let Some(ref parent_id) = session.parent_session_id {
            depth += 1;
            if depth >= MAX_RESUME_DEPTH {
                return Err(AppError::Validation(format!(
                    "session resume chain exceeds maximum depth of {}",
                    MAX_RESUME_DEPTH
                )));
            }
            if seen.contains(parent_id) {
                return Err(AppError::Validation(
                    "cycle detected in parent_session_id chain".into(),
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

/// Log an error, transition the session to error status.
fn abort_with_error(db: &Arc<Db>, session_id: &str, e: impl std::fmt::Display, msg: &str) {
    error!(session_id, error = %e, msg);
    let _ = SessionStore::update_status(db, session_id, "error");
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
    let tool_ctx = crate::tools::ToolContext {
        session_id: session_id.to_string(),
        workspace_id: session.workspace_id.clone(),
        cwd: session
            .cwd
            .as_deref()
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::PathBuf::from(".")),
        codebase_root: std::path::PathBuf::from("."),
        trace_collector: std::sync::Arc::new(trace_collector.clone()),
    };

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
        },
    );

    // Broadcast the terminal `done` event last. The frontend's `done`
    // handler invalidates the history query, which now refetches a
    // history that contains the row we just broadcast in
    // `message_persisted` — no race, no flash.
    sse_manager.broadcast(session_id, SseWireEvent::Done { stop_reason });
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

    for iteration in 0..MAX_TOOL_ITERATIONS {
        if cancel_token.is_cancelled() {
            cancelled = true;
            final_stop_reason = agent::StopReason::Cancelled;
            break;
        }

        // Call the agent.
        let stream = match agent
            .send_message(agent::MessageRequest {
                model: model.clone(),
                messages: history.clone(),
                system: system_prompt.clone(),
                max_tokens,
                tools: tool_defs.clone(),
            })
            .await
        {
            Ok(s) => s,
            Err(e) => {
                error!(session_id, error = %e, "agent send_message failed mid-loop");
                sse_manager.broadcast(
                    session_id,
                    SseWireEvent::Error {
                        message: e.to_string(),
                    },
                );
                had_error = true;
                final_stop_reason = agent::StopReason::EndTurn;
                break;
            }
        };

        let mut stream = stream;
        let mut turn_stop_reason: Option<agent::StopReason> = None;
        pending_tool_requests.clear();
        turn_text.clear();

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
                                        sse::stream_event_to_wire(event.clone()),
                                    );
                                }
                            }
                            match event {
                                StreamEvent::TextDelta { text } => {
                                    turn_text.push_str(&text);
                                    accumulated.push_str(&text);
                                }
                                StreamEvent::ToolUseStart { id, name, input } => {
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
                                StreamEvent::Thinking { .. } => {
                                    // Forwarded to SSE above; no state change.
                                }
                                StreamEvent::Done { stop_reason } => {
                                    turn_stop_reason = Some(stop_reason);
                                    break;
                                }
                                StreamEvent::Error { message } => {
                                    error!(session_id, error = %message, "agent stream error");
                                    trace_collector.emit(TraceEvent {
                                        session_id: session_id.to_string(),
                                        kind: TraceEventKind::Error {
                                            message: message.clone(),
                                        },
                                        timestamp: chrono::Utc::now().to_rfc3339(),
                                    });
                                    had_error = true;
                                    turn_stop_reason = Some(agent::StopReason::EndTurn);
                                    break;
                                }
                            }
                        }
                        Some(Err(e)) => {
                            error!(session_id, error = %e, "agent stream provider error");
                            had_error = true;
                            sse_manager.broadcast(
                                session_id,
                                SseWireEvent::Error { message: e.to_string() },
                            );
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
    use crate::tools::test_support::MockTool;
    use crate::tools::{ToolContext, ToolExecutor, ToolRegistry, ToolResult};
    use async_trait::async_trait;
    use std::sync::Mutex;

    fn test_db() -> Arc<Db> {
        Arc::new(Db::open(std::path::Path::new(":memory:")).unwrap())
    }

    /// A mock agent that captures the MessageRequest it receives.
    struct CapturingAgent {
        captured: Arc<Mutex<Option<MessageRequest>>>,
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

        assert!(matches!(result, Err(AppError::Validation(_))));
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

        assert!(matches!(result, Err(AppError::Validation(_))));
    }

    #[tokio::test]
    async fn test_cancel_session_not_active() {
        let (_, _, _, active, _, _) = test_state();

        let result = SessionService::cancel_session(&active, "nonexistent");
        assert!(matches!(result, Err(AppError::Validation(_))));
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
                            message: "provider stream interrupted".into(),
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
                    stop_reason: agent::StopReason::EndTurn
                }
            ),
            "expected last event to be Done{{EndTurn}}"
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
        let captured = Arc::new(Mutex::new(None));
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
        registry.add_agent(
            &provider.id,
            Arc::new(CapturingAgent {
                captured: Arc::clone(&captured),
            }),
        );

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
        let captured = Arc::new(Mutex::new(None));
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
        registry.add_agent(
            &provider.id,
            Arc::new(CapturingAgent {
                captured: Arc::clone(&captured),
            }),
        );

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

        assert!(matches!(result, Err(AppError::Validation(_))));
        match result.unwrap_err() {
            AppError::Validation(msg) => {
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
        );

        match result {
            Err(AppError::Validation(msg)) => {
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
        );

        match result {
            Err(AppError::Validation(msg)) => {
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
        );

        match result {
            Err(AppError::Validation(msg)) => {
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
        );

        match result {
            Err(AppError::Validation(msg)) => {
                assert!(msg.contains("connecting"), "unexpected message: {}", msg);
            }
            other => panic!("expected Validation for active parent, got: {:?}", other),
        }
    }
}
