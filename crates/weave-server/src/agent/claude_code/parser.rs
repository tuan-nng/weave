//! Claude Code `stream-json` line-stream parser (feat-045).
//!
//! Consumes the newline-delimited JSON output Claude Code produces in
//! `stream-json` mode and converts each line into zero or more
//! [`StreamEvent`]s. The parser is the only Claude-Code-specific piece
//! of the runtime; the rest is shared with Codex and OpenCode via
//! feat-057.
//!
//! ## State machine
//!
//! The parser tracks in-flight `tool_use` blocks by their CLI-assigned
//! ids. A `tool_use` line registers the id, name, and `input` field
//! (if non-empty) in an in-flight list — no `ToolUseStart` is emitted
//! at that point. Subsequent `input_json_delta` lines append to the
//! per-id input buffer and emit a `ToolUseDelta` for each. The
//! `ToolUseStart` is *deferred* until the matching `done` line — or,
//! if no `done` is seen, until the consumer calls
//! [`ClaudeCodeStreamParser::flush`] at end-of-stream. This mirrors
//! `agent::anthropic::streaming::EventConverter` (see `streaming.rs:217-252`
//! for the rationale: the consumer always sees a fully-assembled
//! `Value` rather than a stream of placeholders that have to be
//! glued together downstream).
//!
//! The in-flight list is a `Vec<(String, InFlightToolUse)>` rather
//! than a `HashMap`/`BTreeMap` to preserve **insertion order** of
//! the CLI's tool_use ids — Claude Code assigns opaque ids
//! (e.g. `toolu_01T7K…`) whose lex order does NOT match the order
//! the model announced the tools, and the consumer must see
//! `ToolUseStart`s in the order the CLI emitted them. Lookups are
//! O(n) but n is typically 1–2 in-flight tools per turn.
//!
//! ## Malformed and unknown lines
//!
//! The wire format is not a closed set today. Per the feat-045 spec,
//! malformed JSON and unknown event types are logged at WARN and
//! skipped — they are NEVER fatal. The parser returns `Ok(None)` for
//! skipped lines and continues consuming the stream.
//!
//! ## Resume id capture
//!
//! The first `session_id` line of every turn is captured and exposed
//! via [`ClaudeCodeStreamParser::session_id`] (passive getter) and
//! [`ClaudeCodeStreamParser::take_session_id`] (consuming). Feat-047
//! will wire a `Sender<String>` channel in the `ClaudeCodeCodingAgent`
//! impl and push the captured id into
//! `Session::runtime_metadata_json` at end-of-turn. The parser stays
//! synchronous and tokio-free so it can be unit-tested without a
//! runtime.

// Public surface for `ClaudeCodeCodingAgent` (feat-051) — see the
// cli_runner.rs:58 precedent for the rationale.
#![allow(dead_code)]

use serde_json::Value;
use tracing::warn;

use crate::agent::{StopReason, StreamEvent};
use crate::error::ProviderError;

/// Stateful line-stream parser for Claude Code's `stream-json` output.
///
/// Construct with [`ClaudeCodeStreamParser::new`], feed lines via
/// [`ClaudeCodeStreamParser::feed_line`], and at end-of-stream call
/// [`ClaudeCodeStreamParser::flush`] to drain any pending deferred
/// `ToolUseStart` events. The captured session id is available via
/// [`ClaudeCodeStreamParser::session_id`] /
/// [`ClaudeCodeStreamParser::take_session_id`].
#[derive(Debug, Default)]
pub struct ClaudeCodeStreamParser {
    /// Captured from the first `session_id` line; immutable after.
    session_id: Option<String>,
    /// In-flight tool_use blocks, in the order the CLI announced
    /// them. A `Vec` rather than a map to preserve that order — see
    /// the module-level docs for the rationale.
    in_flight: Vec<(String, InFlightToolUse)>,
}

/// One in-flight `tool_use` block awaiting assembly.
#[derive(Debug)]
struct InFlightToolUse {
    /// Tool name from the `tool_use` line. Empty string if the CLI
    /// emitted `input_json_delta` without a preceding `tool_use` for
    /// this id (a wire-protocol anomaly — see the warn-log in
    /// [`ClaudeCodeStreamParser::handle_input_json_delta`]).
    name: String,
    /// Initial input from the `tool_use` line's `input` field, when
    /// non-empty. Used at flush time only when no deltas were
    /// received. `None` when the `tool_use` line carried `null`, an
    /// empty object `{}`, or no `input` field at all — the parser
    /// treats these three cases identically as "deltas will provide
    /// the complete input".
    initial_input: Option<Value>,
    /// Concatenated delta text from `input_json_delta` lines. Parsed
    /// to `serde_json::Value` on flush; takes precedence over
    /// `initial_input` when non-empty.
    input_text: String,
}

impl ClaudeCodeStreamParser {
    /// Build a new parser with no captured state.
    pub fn new() -> Self {
        Self::default()
    }

    /// The captured session id from the first `session_id` line, if
    /// any. Returns `None` before the first `session_id` line is seen
    /// or after [`Self::take_session_id`] has consumed it.
    pub fn session_id(&self) -> Option<&str> {
        self.session_id.as_deref()
    }

    /// Consume and return the captured session id (one-shot). Equivalent
    /// to `Option::take` on [`Self::session_id`].
    pub fn take_session_id(&mut self) -> Option<String> {
        self.session_id.take()
    }

    /// Feed one line of stdout from the CLI. Returns zero or more
    /// [`StreamEvent`]s to forward downstream; typically empty or a
    /// singleton. Returns `vec![ToolUseStart, Done]` (in that order)
    /// on `done` when one or more `tool_use`s were in flight.
    ///
    /// Malformed JSON and unknown event types are logged at WARN and
    /// produce an empty result — the parser NEVER aborts on bad input.
    /// The `Result::Err` variant is reserved for symmetry with
    /// `agent::anthropic::streaming::EventConverter`; in normal
    /// operation it is never returned.
    pub fn feed_line(&mut self, line: &str) -> Result<Option<Vec<StreamEvent>>, ProviderError> {
        let v: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    line = %line,
                    error = %e,
                    "claude_code: malformed JSON line; skipping (never fatal)"
                );
                return Ok(None);
            }
        };

        let ty = v.get("type").and_then(Value::as_str);
        match ty {
            Some("session_id") => {
                // Capture but emit nothing.
                if self.session_id.is_none() {
                    if let Some(id) = v.get("id").and_then(Value::as_str) {
                        self.session_id = Some(id.to_string());
                    } else {
                        warn!(
                            line = %line,
                            "claude_code: session_id line missing `id` field; capturing nothing"
                        );
                    }
                }
                Ok(None)
            }
            Some("text_delta") => Ok(v.get("text").and_then(Value::as_str).map(|text| {
                vec![StreamEvent::TextDelta {
                    text: text.to_string(),
                }]
            })),
            Some("tool_use") => {
                // Deferred: register the in-flight block, emit nothing now.
                self.register_tool_use(&v);
                Ok(None)
            }
            Some("input_json_delta") => Ok(self.handle_input_json_delta(&v)),
            Some("tool_result") => {
                // Field rename: tool_use_id -> id, content -> result.
                let id = v.get("tool_use_id").and_then(Value::as_str);
                let content = v.get("content").and_then(Value::as_str);
                match (id, content) {
                    (Some(id), Some(content)) => Ok(Some(vec![StreamEvent::ToolResult {
                        id: id.to_string(),
                        result: content.to_string(),
                    }])),
                    _ => {
                        warn!(
                            line = %line,
                            "claude_code: tool_result line missing `tool_use_id` or `content`; skipping"
                        );
                        Ok(None)
                    }
                }
            }
            Some("thinking") => Ok(v.get("text").and_then(Value::as_str).map(|text| {
                vec![StreamEvent::Thinking {
                    text: text.to_string(),
                }]
            })),
            Some("error") => {
                // Prefer `message`; fall back to `code` for events
                // that carry only the code (e.g., `permission_denied`).
                let message = v
                    .get("message")
                    .and_then(Value::as_str)
                    .map(String::from)
                    .or_else(|| {
                        v.get("code")
                            .and_then(Value::as_str)
                            .map(|code| format!("error code: {code}"))
                    });
                match message {
                    Some(message) => Ok(Some(vec![StreamEvent::Error { message }])),
                    None => {
                        warn!(
                            line = %line,
                            "claude_code: error line missing `message` and `code`; skipping"
                        );
                        Ok(None)
                    }
                }
            }
            Some("done") => {
                // Flush any pending deferred tool_uses first, then the Done.
                let mut events = self.flush_pending();
                let stop_reason = v
                    .get("stop_reason")
                    .and_then(Value::as_str)
                    .map(map_stop_reason)
                    .unwrap_or(StopReason::EndTurn);
                events.push(StreamEvent::Done { stop_reason });
                Ok(Some(events))
            }
            Some(other) => {
                warn!(
                    event_type = %other,
                    "claude_code: unknown event type; skipping (forward-compat point)"
                );
                Ok(None)
            }
            None => {
                warn!(
                    line = %line,
                    "claude_code: line missing `type` field; skipping"
                );
                Ok(None)
            }
        }
    }

    /// Drain any pending deferred `ToolUseStart` events at end-of-stream.
    /// Returns the first one if any are pending, or `None` if no
    /// `tool_use` was in flight. Repeated calls drain in order; the
    /// in-flight map is empty after the last pending block is emitted.
    ///
    /// Callers should invoke this after the `LineStream` returns `None`
    /// so that a CLI that exits without emitting `done` (e.g., it
    /// crashed or was cancelled mid-stream) still surfaces its
    /// in-flight `tool_use` blocks.
    pub fn flush(&mut self) -> Option<StreamEvent> {
        self.flush_pending().into_iter().next()
    }

    // --- private helpers ---

    /// Register a `tool_use` block in the in-flight list. Captures
    /// the tool name and the `input` field if non-empty (`null`,
    /// `{}`, and missing `input` are all treated as "no initial
    /// input — deltas will provide the complete value"; see
    /// [`is_empty_input_value`]). If a `tool_use` for this id was
    /// already registered (e.g., an `input_json_delta` arrived
    /// first), the existing entry's name and initial_input are
    /// updated in place — the position in the list is preserved so
    /// the deferred `ToolUseStart` still flushes in the order the
    /// CLI first announced the id.
    fn register_tool_use(&mut self, v: &Value) {
        let id = match v.get("id").and_then(Value::as_str) {
            Some(id) => id.to_string(),
            None => {
                warn!("claude_code: tool_use line missing `id`; not registering");
                return;
            }
        };
        let name = v
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let initial_input = v
            .get("input")
            .filter(|input| !is_empty_input_value(input))
            .cloned();
        if let Some((_, entry)) = self.in_flight.iter_mut().find(|(k, _)| k == &id) {
            entry.name = name;
            if initial_input.is_some() {
                entry.initial_input = initial_input;
            }
        } else {
            self.in_flight.push((
                id,
                InFlightToolUse {
                    name,
                    initial_input,
                    input_text: String::new(),
                },
            ));
        }
    }

    /// Append an `input_json_delta` to the in-flight buffer for `id`
    /// and return a `ToolUseDelta` event. If `id` is unknown (CLI sent
    /// a delta without a preceding `tool_use`), register a stub entry
    /// (empty name) so the deferred `ToolUseStart` at flush time has
    /// something to attach to; warn-log the anomaly.
    fn handle_input_json_delta(&mut self, v: &Value) -> Option<Vec<StreamEvent>> {
        let id = match v.get("id").and_then(Value::as_str) {
            Some(id) => id.to_string(),
            None => {
                warn!("claude_code: input_json_delta line missing `id`; skipping");
                return None;
            }
        };
        let delta = match v.get("delta").and_then(Value::as_str) {
            Some(delta) => delta.to_string(),
            None => {
                warn!(
                    tool_use_id = %id,
                    "claude_code: input_json_delta line missing `delta`; skipping"
                );
                return None;
            }
        };
        if let Some((_, entry)) = self.in_flight.iter_mut().find(|(k, _)| k == &id) {
            entry.input_text.push_str(&delta);
        } else {
            // Stub: real Claude Code always emits tool_use first. The
            // empty name will be visible at flush time as a
            // wire-protocol anomaly.
            self.in_flight.push((
                id.clone(),
                InFlightToolUse {
                    name: String::new(),
                    initial_input: None,
                    input_text: delta.clone(),
                },
            ));
        }
        Some(vec![StreamEvent::ToolUseDelta { id, delta }])
    }

    /// Flush all in-flight tool_uses as `ToolUseStart` events in
    /// insertion order. Each input is assembled from the deltas (if
    /// any) and the initial `input` value (if any); on parse failure
    /// the buffer falls back to the initial input, then to `{}`
    /// (matches the `EventConverter::convert_block_stop` defensive
    /// fallback at `agent::anthropic::streaming.rs:242-250`).
    fn flush_pending(&mut self) -> Vec<StreamEvent> {
        let empty_input = || serde_json::json!({});
        let entries = std::mem::take(&mut self.in_flight);
        entries
            .into_iter()
            .map(|(id, entry)| {
                let input: Value = if entry.input_text.is_empty() {
                    entry.initial_input.unwrap_or_else(empty_input)
                } else {
                    serde_json::from_str(&entry.input_text).unwrap_or_else(|e| {
                        warn!(
                            tool_use_id = %id,
                            raw = %entry.input_text,
                            error = %e,
                            "claude_code: tool_use assembled input failed to parse; \
                             emitting empty object so the consumer can still surface \
                             a validation error"
                        );
                        entry.initial_input.unwrap_or_else(empty_input)
                    })
                };
                StreamEvent::ToolUseStart {
                    id,
                    name: entry.name,
                    input,
                }
            })
            .collect()
    }
}

/// Returns `true` for "no meaningful initial input" values: `null`,
/// empty objects `{}`, and empty arrays `[]`. Used by
/// [`ClaudeCodeStreamParser::register_tool_use`] to decide whether
/// to treat the `tool_use` line's `input` field as the complete
/// initial value (non-empty) or as a placeholder for upcoming
/// deltas (empty / null / missing).
fn is_empty_input_value(v: &Value) -> bool {
    match v {
        Value::Null => true,
        Value::Object(map) => map.is_empty(),
        Value::Array(arr) => arr.is_empty(),
        _ => false,
    }
}

/// Map a `stop_reason` string from the `done` line to a [`StopReason`].
/// Unknown values default to `EndTurn` (defensive, matches the
/// Anthropic `map_stop_reason` at
/// `agent::anthropic::streaming.rs:268-275`).
fn map_stop_reason(reason: &str) -> StopReason {
    match reason {
        "end_turn" => StopReason::EndTurn,
        "max_tokens" => StopReason::MaxTokens,
        "tool_use" => StopReason::ToolUse,
        // Real Claude Code does not currently emit `cancelled` on
        // `done` — cancel is surfaced by the runner as
        // `CliRunResult::Cancelled` (see `agent::cli_runner.rs`). But
        // if a future version does emit it, map it through.
        "cancelled" => StopReason::Cancelled,
        other => {
            warn!(
                stop_reason = %other,
                "claude_code: unknown done.stop_reason; defaulting to EndTurn"
            );
            StopReason::EndTurn
        }
    }
}
