use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use tokio_util::sync::CancellationToken;
use tracing::{error, info, warn};

use crate::agent::registry::ProviderRegistry;
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

    // Call the agent
    let stream = match agent
        .send_message(MessageRequest {
            model,
            messages: history,
            system: system_prompt,
            max_tokens: DEFAULT_MAX_TOKENS,
            tools: tool_defs,
        })
        .await
    {
        Ok(s) => s,
        Err(e) => {
            abort_with_error(&db, session_id, e, "agent send_message failed");
            return;
        }
    };

    // Stream and accumulate with cancellation support
    use futures_util::StreamExt;

    // Spawn trace collector flush task
    let (trace_collector, flush_handle) = trace::spawn_flush_task(db.clone());

    // Track pending tool calls for duration computation
    let mut pending_tools: HashMap<String, (String, serde_json::Value, std::time::Instant)> =
        HashMap::new();

    let mut accumulated = String::new();
    let mut stream = stream;
    let mut had_error = false;
    // Buffer for coalescing `StreamEvent::Thinking` deltas into a single
    // `Decision` trace event per contiguous thinking block. The Anthropic
    // provider streams extended thinking as many small chunks (often 2-3
    // word fragments), and emitting one trace row per delta produces a
    // wall of unreadable fragments in the Journey sidebar (e.g. "Hmm,"
    // / "the user" / "greeting" / "."). Flushing at the boundary with
    // the next text/tool event — and at Done/Error — turns 30-50
    // fragments per turn into one readable decision row per actual
    // reasoning pass. The SSE wire still forwards every delta to
    // subscribers unchanged; only the trace persistence is coalesced.
    let mut thinking_buffer = String::new();
    // Captured from the agent's terminal `Done` event so we can broadcast
    // it *after* the assistant message has been persisted to the database.
    // The frontend's `done` handler invalidates the history query, and the
    // refetch would otherwise race ahead of the INSERT and return a
    // history without the new message — causing a visible flash where the
    // streamed text disappears before the persisted message appears.
    let mut pending_stop_reason: Option<agent::StopReason> = None;
    // Set when the user cancelled (either mid-stream or after a race with
    // the agent's `Done`). The post-loop finalization reads this to
    // decide whether to persist a partial message with stop_reason=
    // "cancelled" and to set the session status to "cancelled".
    let mut cancelled: bool = false;

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                info!(session_id, "session cancelled by user");
                cancelled = true;
                // Break out of the loop; the post-loop finalization block
                // is the single source of truth for persist + status +
                // broadcast, including the cancel path. This replaces the
                // prior early-return that silently dropped the partial
                // streamed text on cancel.
                break;
            }
            item = StreamExt::next(&mut stream) => {
                match item {
                    Some(Ok(event)) => {
                        // Broadcast every non-terminal agent event to SSE
                        // subscribers inline. The `Done` event is captured
                        // here and broadcast later, after the assistant
                        // message has been persisted, so the frontend's
                        // history refetch always finds the new message.
                        match &event {
                            agent::StreamEvent::Done { stop_reason } => {
                                pending_stop_reason = Some(stop_reason.clone());
                            }
                            agent::StreamEvent::Error { .. } => {
                                sse_manager.broadcast(
                                    session_id,
                                    sse::stream_event_to_wire(event.clone()),
                                );
                            }
                            _ => {
                                sse_manager.broadcast(
                                    session_id,
                                    sse::stream_event_to_wire(event.clone()),
                                );
                            }
                        }
                        match event {
                            agent::StreamEvent::TextDelta { text } => {
                                flush_thinking(&trace_collector, session_id, &mut thinking_buffer);
                                accumulated.push_str(&text);
                            }
                            agent::StreamEvent::ToolUseStart { id, name, input } => {
                                flush_thinking(&trace_collector, session_id, &mut thinking_buffer);
                                pending_tools.insert(id, (name, input, std::time::Instant::now()));
                            }
                            agent::StreamEvent::ToolResult { id, result } => {
                                flush_thinking(&trace_collector, session_id, &mut thinking_buffer);
                                if let Some((name, input, start)) = pending_tools.remove(&id) {
                                    let duration_ms = start.elapsed().as_millis() as u64;
                                    let ts = chrono::Utc::now().to_rfc3339();
                                    // Extract file changes from tool input
                                    for fc in trace::extract_file_changes(
                                        session_id, &name, &input, &ts,
                                    ) {
                                        trace_collector.emit(fc);
                                    }
                                    // Emit tool call trace event
                                    trace_collector.emit(TraceEvent {
                                        session_id: session_id.to_string(),
                                        kind: TraceEventKind::ToolCall {
                                            tool_name: name,
                                            input_json: input.to_string(),
                                            output_json: result,
                                            duration_ms,
                                        },
                                        timestamp: ts,
                                    });
                                }
                            }
                            agent::StreamEvent::Thinking { text } => {
                                // Coalesce: append to the buffer instead of
                                // emitting per-delta. `flush_thinking()`
                                // fires at the next text/tool/Done/Error
                                // boundary. See `flush_thinking` for the
                                // full rationale.
                                thinking_buffer.push_str(&text);
                            }
                            agent::StreamEvent::Done { .. } => {
                                flush_thinking(&trace_collector, session_id, &mut thinking_buffer);
                                break;
                            }
                            agent::StreamEvent::Error { message } => {
                                flush_thinking(&trace_collector, session_id, &mut thinking_buffer);
                                error!(session_id, error = %message, "agent stream error");
                                trace_collector.emit(TraceEvent {
                                    session_id: session_id.to_string(),
                                    kind: TraceEventKind::Error {
                                        message: message.clone(),
                                    },
                                    timestamp: chrono::Utc::now().to_rfc3339(),
                                });
                                had_error = true;
                                break;
                            }
                            _ => {}
                        }
                    },
                    Some(Err(e)) => {
                        error!(session_id, error = %e, "agent stream provider error");
                        had_error = true;
                        sse_manager.broadcast(
                            session_id,
                            SseWireEvent::Error { message: e.to_string() },
                        );
                        break;
                    }
                    None => {
                        // Stream ended without a Done event
                        break;
                    }
                }
            }
        }
    }

    // Drain orphaned pending tool calls (ToolUseStart without ToolResult)
    drain_pending_tools(&trace_collector, session_id, &mut pending_tools);

    // Final flush of the thinking buffer. Covers the stream-ended-without-
    // Done paths (provider error, channel close, cancel token fire) where
    // the per-event flush never ran.
    flush_thinking(&trace_collector, session_id, &mut thinking_buffer);

    // Flush remaining trace events
    drop(trace_collector);
    let _ = flush_handle.await;

    // Re-check cancellation after loop (race between Done and Cancel). If
    // the agent finished normally but the user clicked Cancel at almost
    // the same instant, we treat the turn as cancelled — the user
    // expressed intent before the result was visible to them.
    if cancel_token.is_cancelled() {
        cancelled = true;
    }

    // Decide the stop_reason and final_status for this turn. The order
    // matters: cancellation wins over error wins over the agent's natural
    // stop_reason, because user intent outranks both transport failure
    // and the model's own end-of-stream signal.
    let stop_reason = if cancelled {
        agent::StopReason::Cancelled
    } else if had_error {
        // There is no StopReason::Error variant; reuse EndTurn and let
        // the metadata JSON record the "errored" tag so the frontend
        // can render an Error badge.
        agent::StopReason::EndTurn
    } else {
        pending_stop_reason.unwrap_or(agent::StopReason::EndTurn)
    };

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
    let metadata_json = build_message_metadata(&stop_reason, had_error);
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

/// Build the `messages.metadata` JSON string for a persisted assistant
/// message. Returns `None` for the common `EndTurn` case so we don't
/// write empty `{}` rows to the DB. The frontend parses this JSON to
/// render a "Cancelled" / "Error" / "MaxTokens" badge on the persisted
/// bubble.
fn build_message_metadata(stop_reason: &agent::StopReason, had_error: bool) -> Option<String> {
    let tag: &str = if had_error {
        "error"
    } else {
        match stop_reason {
            agent::StopReason::Cancelled => "cancelled",
            agent::StopReason::MaxTokens => "max_tokens",
            agent::StopReason::EndTurn | agent::StopReason::ToolUse => return None,
        }
    };
    Some(serde_json::json!({ "stop_reason": tag }).to_string())
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
        }
    };
    Some(s.to_string())
}

/// Drain orphaned pending tool calls (ToolUseStart without matching ToolResult).
///
/// Emits a trace event for each incomplete tool call so that the trace
/// contains a record of tools that were invoked but never completed.
fn drain_pending_tools(
    trace_collector: &trace::TraceCollector,
    session_id: &str,
    pending: &mut HashMap<String, (String, serde_json::Value, std::time::Instant)>,
) {
    for (_id, (name, input, start)) in pending.drain() {
        let duration_ms = start.elapsed().as_millis() as u64;
        trace_collector.emit(TraceEvent {
            session_id: session_id.to_string(),
            kind: TraceEventKind::ToolCall {
                tool_name: name,
                input_json: input.to_string(),
                output_json: r#"{"error":"incomplete"}"#.to_string(),
                duration_ms,
            },
            timestamp: chrono::Utc::now().to_rfc3339(),
        });
    }
}

/// Flush accumulated `StreamEvent::Thinking` text into a single `Decision`
/// trace event. Called at the boundary between a thinking block and the
/// next text/tool event, and at terminal events (`Done`/`Error`).
///
/// Whitespace-only or empty buffers are dropped without emitting — a
/// row with no readable text would be just as useless as the fragmented
/// rows this helper replaces.
///
/// Defined as a free function (not a `FnMut` closure) so the call sites
/// can hold their own `&mut` borrow of the buffer for the per-event
/// `push_str` write. A closure that captures `&mut thinking_buffer`
/// would lock the buffer for the closure's entire lifetime, blocking
/// the direct `push_str` in the `Thinking` arm.
fn flush_thinking(trace_collector: &trace::TraceCollector, session_id: &str, buf: &mut String) {
    if buf.trim().is_empty() {
        // Whitespace-only or empty — drop without emitting.
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
    use crate::tools::ToolRegistry;
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
        tool_registry.register(Arc::new(MockTool::new("kanban")));
        tool_registry.register(Arc::new(MockTool::new("notes")));
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
                "get_task",
                "kanban",
                "list_tasks",
                "notes",
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

    #[tokio::test]
    async fn test_thinking_deltas_coalesce_into_single_decision() {
        // The Anthropic provider streams extended thinking as many small
        // chunks (2-3 words each). The session loop must coalesce them
        // into one Decision trace event per contiguous thinking block,
        // so the Journey sidebar shows one readable row per reasoning
        // pass rather than dozens of fragments. The boundary between
        // the thinking block and the next text block is the natural
        // flush point.
        use async_trait::async_trait;
        use futures_core::Stream;
        use std::pin::Pin;

        struct ThinkingAgent;

        #[async_trait]
        impl crate::agent::CodingAgent for ThinkingAgent {
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
                    // 5 small deltas — exactly the kind of fragmentation
                    // Anthropic's extended thinking stream produces.
                    for chunk in ["Hmm,", " the user", " said ", "\"hi\"", " — a greeting"] {
                        let _ = tx
                            .send(Ok(agent::StreamEvent::Thinking {
                                text: chunk.to_string(),
                            }))
                            .await;
                    }
                    // Boundary: a text delta marks the end of the
                    // thinking block. The buffer must flush here.
                    let _ = tx
                        .send(Ok(agent::StreamEvent::TextDelta {
                            text: "Hello!".into(),
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
        registry.add_agent(&provider.id, Arc::new(ThinkingAgent));

        let session = crate::store::sessions::SessionStore::create(
            &db,
            &ws.id,
            &provider.id,
            None,
            None,
            None,
            None,
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
            "hello",
        )
        .await
        .unwrap();

        // Wait for completion. `run_prompt_task` drops the trace
        // collector and awaits the flush task before removing the
        // session from the active set, so traces are durable by the
        // time `active.contains` is false.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while active.contains(&session.id) && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(!active.contains(&session.id));

        // Read traces back. Exactly one Decision row, with the
        // concatenated text.
        let traces = crate::store::traces::TraceStore::list_by_session(&db, &session.id).unwrap();
        let decisions: Vec<_> = traces
            .iter()
            .filter(|t| t.event_type == "decision")
            .collect();
        assert_eq!(
            decisions.len(),
            1,
            "5 thinking deltas should coalesce into 1 decision row, got {decisions:?}"
        );
        let expected = "Hmm, the user said \"hi\" — a greeting";
        assert_eq!(decisions[0].summary, expected);
        let data: serde_json::Value = serde_json::from_str(
            decisions[0]
                .data_json
                .as_deref()
                .expect("decision has data_json"),
        )
        .unwrap();
        assert_eq!(data["text"], expected);
    }

    // --- Session resume tests (feat-018) ---

    /// Helper: create a session via the service layer.
    fn create_session_via_service(
        db: &Db,
        ws_id: &str,
        provider_id: &str,
        parent_session_id: Option<&str>,
    ) -> crate::store::sessions::Session {
        SessionService::create_session(db, ws_id, provider_id, None, None, None, parent_session_id)
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
        );

        match result {
            Err(AppError::Validation(msg)) => {
                assert!(msg.contains("connecting"), "unexpected message: {}", msg);
            }
            other => panic!("expected Validation for active parent, got: {:?}", other),
        }
    }
}
