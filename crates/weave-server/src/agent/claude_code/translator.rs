//! Claude Code journey translator (feat-048).
//!
//! Maps parsed [`StreamEvent`]s from [`super::ClaudeCodeStreamParser`]
//! into Weave [`TraceEvent`]s, with the CLI as the source of truth for
//! tool results. The translator NEVER re-executes CLI tools — it only
//! records what the CLI did.
//!
//! ## What is translated
//!
//! | Stream event    | Trace event(s) emitted                                                |
//! |-----------------|-----------------------------------------------------------------------|
//! | `TextDelta`     | (none — pass-through to SSE)                                          |
//! | `ToolUseStart`  | (registers in-flight tracker; emits on result)                        |
//! | `ToolUseDelta`  | (none — input assembly lives in the parser)                           |
//! | `ToolResult`    | `ToolCall { status: None }` + (maybe) `FileChange`                    |
//! | `Thinking`      | `Decision` (coalesced, flushed on non-thinking event or end-of-turn)  |
//! | `Done`          | flushes pending `Decision` text + orphans                              |
//! | `Error`         | `Error`                                                               |
//!
//! ## Orphan detection
//!
//! A `ToolUseStart` whose matching `ToolResult` never arrives before
//! `Done` (or `finish()` at end-of-stream) is recorded as a
//! `ToolCall` with `status = Some("orphaned")` — the spec's
//! `tool_call + status="orphaned"` path. The agent (feat-051) drains
//! `parser.flush()` in a loop and feeds the deferred events to
//! [`JourneyTranslator::on_event`]; any in-flight tracker that has
//! not been closed by a `ToolResult` is orphaned at the final
//! `finish()`.
//!
//! ## File changes
//!
//! The CLI does not emit a dedicated `file_change` event in its
//! `stream-json` wire format. We synthesize them from
//! `ToolUseStart` input via [`cli_tool_to_file_change`] — a
//! Claude-Code-specific name → `FileAction` map. The map lives
//! here (not in `trace::extract_file_changes`) to keep CLI-specific
//! knowledge in the adapter.
//!
//! ## Thinking coalescing
//!
//! Mirrors the native `flush_thinking` pattern at
//! `service::sessions.rs:1090-1101`: multiple `Thinking` events in a
//! row accumulate into a single `Decision` trace event, emitted
//! when a non-thinking event arrives or the turn ends.
//!
//! ## Why a `BTreeMap` (not a `Vec`)
//!
//! The parser uses a `Vec<(String, InFlightToolUse)>` to preserve
//! *insertion* order of the CLI's opaque tool_use ids (their lex
//! order is unrelated to the model's announcement order). The
//! translator does NOT need to preserve that order — the test
//! suite asserts on the SET of trace events, not on order. A
//! `BTreeMap` gives deterministic iteration for stable assertions
//! and matches the native `pending_tool_requests` precedent at
//! `service::sessions.rs:1181`.

// Public surface for `ClaudeCodeCodingAgent` (feat-051) — see the
// `parser.rs:52` precedent for the rationale.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::time::Instant;

use serde_json::Value;
use tracing::warn;

use crate::agent::StreamEvent;
use crate::store::traces::{FileAction, TraceEvent, TraceEventKind};
use crate::trace::TraceCollector;

// ---------------------------------------------------------------------------
// JourneyTranslator
// ---------------------------------------------------------------------------

/// Stateful translator from parsed Claude Code `StreamEvent`s to
/// Weave `TraceEvent`s.
///
/// Construct with [`JourneyTranslator::new`], feed events via
/// [`JourneyTranslator::on_event`], and call
/// [`JourneyTranslator::finish`] at end-of-stream to surface any
/// orphans when the CLI exits without a `done` line.
pub struct JourneyTranslator<'a> {
    session_id: &'a str,
    collector: &'a TraceCollector,
    /// In-flight tool_use blocks, keyed by `tool_use_id` (opaque,
    /// assigned by the CLI). Entries persist past `ToolResult` so
    /// that (a) defensive dedup of duplicate `tool_result` deltas for
    /// the same id emits `FileChange` only once, and (b) `finish()`
    /// can distinguish completed entries from orphans.
    in_flight: BTreeMap<String, ToolTracker>,
    /// Accumulated thinking text since the last flush.
    pending_thinking: String,
}

/// One in-flight or recently-completed `tool_use` block.
struct ToolTracker {
    tool_name: String,
    /// Parsed `Value` so we can re-serialize for the `ToolCall` event
    /// AND drive [`cli_tool_to_file_change`] synthesis.
    input: Value,
    /// Recorded at `ToolUseStart` so `duration_ms` at `ToolResult` is
    /// the honest "time between CLI announcing the tool and the CLI
    /// emitting the result" — i.e. CLI-side tool execution time.
    started_at: Instant,
    /// Defensive flag: Claude Code's stream-json spec does not
    /// currently emit multiple `tool_result` deltas for the same id,
    /// but the test suite exercises that shape. Tracks whether
    /// `FileChange` has been emitted for this tool_use_id so we
    /// dedup.
    file_change_emitted: bool,
    /// `false` until a matching `ToolResult` arrives. The orphan
    /// flush at `Done` / `finish()` filters on this flag.
    completed: bool,
}

impl<'a> JourneyTranslator<'a> {
    /// Build a translator that emits to `collector` for `session_id`.
    pub fn new(session_id: &'a str, collector: &'a TraceCollector) -> Self {
        Self {
            session_id,
            collector,
            in_flight: BTreeMap::new(),
            pending_thinking: String::new(),
        }
    }

    /// Handle one parsed stream event.
    ///
    /// Emits zero or more [`TraceEvent`]s to the collector. Returns
    /// the same events for test inspection; production callers can
    /// ignore the return value (the collector is the source of
    /// truth for what was emitted).
    pub fn on_event(&mut self, event: &StreamEvent) -> Vec<TraceEvent> {
        match event {
            StreamEvent::TextDelta { .. } => {
                // Pass-through: text deltas go to SSE, not traces —
                // but a non-thinking event breaks the coalescing
                // window, so flush any pending `Decision` first
                // (mirrors the native `flush_thinking` call at
                // `service::sessions.rs:1273`).
                let _ = self.flush_thinking();
                Vec::new()
            }
            StreamEvent::ToolUseStart { id, name, input } => {
                // A non-thinking event breaks the coalescing window.
                let events = self.flush_thinking();
                // Defer the `ToolCall` emit until the matching
                // `ToolResult` arrives. The CLI is the source of
                // truth for the result text.
                self.in_flight.insert(
                    id.clone(),
                    ToolTracker {
                        tool_name: name.clone(),
                        input: input.clone(),
                        started_at: Instant::now(),
                        file_change_emitted: false,
                        completed: false,
                    },
                );
                events
            }
            StreamEvent::ToolUseDelta { .. } => {
                // No-op for traces — input assembly happens inside
                // the parser. The `ToolUseStart` that already fired
                // carries the final input; deltas are SSE-only.
                // Also: ToolUseDelta is a continuation of the same
                // tool_use's input, not a new "thought", so it
                // should NOT break the thinking-coalescing window.
                Vec::new()
            }
            StreamEvent::ToolResult { id, result } => {
                // A non-thinking event breaks the coalescing window.
                let mut events = self.flush_thinking();

                let tracker = match self.in_flight.get_mut(id) {
                    Some(t) => t,
                    None => {
                        // Wire-protocol anomaly: the CLI emitted a
                        // `tool_result` for a `tool_use_id` that was
                        // never announced. Log and skip — the spec
                        // does not define a recovery path.
                        warn!(
                            tool_use_id = %id,
                            "claude_code: tool_result for unknown tool_use_id; \
                             recording as anonymous tool_call with empty input"
                        );
                        let ev = TraceEvent {
                            session_id: self.session_id.to_string(),
                            kind: TraceEventKind::ToolCall {
                                tool_name: "<unknown>".to_string(),
                                input_json: "{}".to_string(),
                                output_json: result.clone(),
                                duration_ms: 0,
                                status: None,
                            },
                            timestamp: chrono::Utc::now().to_rfc3339(),
                        };
                        self.collector.emit(ev.clone());
                        events.push(ev);
                        return events;
                    }
                };

                let duration_ms = tracker.started_at.elapsed().as_millis() as u64;
                let input_json =
                    serde_json::to_string(&tracker.input).unwrap_or_else(|_| "{}".to_string());

                // Always emit the `ToolCall` so the trace stream
                // records the latest result text (matters for
                // duplicate-delta shape — keep the most recent
                // result rather than dropping it on the floor).
                let tool_call = TraceEvent {
                    session_id: self.session_id.to_string(),
                    kind: TraceEventKind::ToolCall {
                        tool_name: tracker.tool_name.clone(),
                        input_json,
                        output_json: result.clone(),
                        duration_ms,
                        status: None,
                    },
                    timestamp: chrono::Utc::now().to_rfc3339(),
                };
                self.collector.emit(tool_call.clone());
                events.push(tool_call);

                // Synthesize `FileChange` once per tool_use_id.
                if !tracker.file_change_emitted {
                    if let Some((path, action)) =
                        cli_tool_to_file_change(&tracker.tool_name, &tracker.input)
                    {
                        let fc = TraceEvent {
                            session_id: self.session_id.to_string(),
                            kind: TraceEventKind::FileChange { path, action },
                            timestamp: chrono::Utc::now().to_rfc3339(),
                        };
                        self.collector.emit(fc.clone());
                        events.push(fc);
                        tracker.file_change_emitted = true;
                    }
                }

                // Mark completed but keep the entry — orphan
                // detection on `Done` filters via `completed`.
                tracker.completed = true;
                events
            }
            StreamEvent::Thinking { text } => {
                // Coalesce into pending buffer; flush on the next
                // non-thinking event or at end-of-turn.
                if !self.pending_thinking.is_empty() {
                    self.pending_thinking.push('\n');
                }
                self.pending_thinking.push_str(text);
                Vec::new()
            }
            StreamEvent::Done { .. } => {
                // Flush thinking first, then orphans.
                let mut events = self.flush_thinking();
                events.extend(self.flush_orphans());
                events
            }
            StreamEvent::Error { message } => {
                // A non-thinking event breaks the coalescing window.
                let mut events = self.flush_thinking();
                let ev = TraceEvent {
                    session_id: self.session_id.to_string(),
                    kind: TraceEventKind::Error {
                        message: message.clone(),
                    },
                    timestamp: chrono::Utc::now().to_rfc3339(),
                };
                self.collector.emit(ev.clone());
                events.push(ev);
                events
            }
        }
    }

    /// End-of-stream flush.
    ///
    /// The agent (feat-051) calls this after `parser.flush()` to
    /// surface any in-flight state when the CLI exits without a
    /// `done` line (crash, cancel). Emits pending `Decision` text
    /// and any orphaned `ToolCall`s.
    pub fn finish(&mut self) -> Vec<TraceEvent> {
        let mut events = self.flush_thinking();
        events.extend(self.flush_orphans());
        events
    }

    // --- private helpers ---

    fn flush_thinking(&mut self) -> Vec<TraceEvent> {
        if self.pending_thinking.is_empty() {
            return Vec::new();
        }
        let text = std::mem::take(&mut self.pending_thinking);
        let ev = TraceEvent {
            session_id: self.session_id.to_string(),
            kind: TraceEventKind::Decision { text },
            timestamp: chrono::Utc::now().to_rfc3339(),
        };
        self.collector.emit(ev.clone());
        vec![ev]
    }

    fn flush_orphans(&mut self) -> Vec<TraceEvent> {
        if self.in_flight.is_empty() {
            return Vec::new();
        }
        let entries = std::mem::take(&mut self.in_flight);
        entries
            .into_iter()
            .filter(|(_, t)| !t.completed)
            .map(|(_, tracker)| {
                let input_json =
                    serde_json::to_string(&tracker.input).unwrap_or_else(|_| "{}".to_string());
                let ev = TraceEvent {
                    session_id: self.session_id.to_string(),
                    kind: TraceEventKind::ToolCall {
                        tool_name: tracker.tool_name,
                        input_json,
                        output_json: String::new(),
                        duration_ms: tracker.started_at.elapsed().as_millis() as u64,
                        status: Some("orphaned".to_string()),
                    },
                    timestamp: chrono::Utc::now().to_rfc3339(),
                };
                self.collector.emit(ev.clone());
                ev
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Free function: CLI tool name → (path, FileAction)
// ---------------------------------------------------------------------------

/// Map a Claude Code tool name + input to a `(path, FileAction)`
/// tuple, or `None` for tools that don't mutate files.
///
/// Claude Code's `stream-json` spec does NOT emit a dedicated
/// `file_change` event — we synthesize from the tool input at
/// `ToolResult` time. This map is Claude-Code-specific and lives in
/// the adapter to keep `trace::extract_file_changes` generic.
///
/// Read tools do NOT synthesize a `FileChange` — reads don't mutate
/// the file, and the Journey UI's file_changes view should show only
/// what actually changed.
pub(crate) fn cli_tool_to_file_change(
    tool_name: &str,
    input: &Value,
) -> Option<(String, FileAction)> {
    match tool_name {
        "Write" | "Edit" | "MultiEdit" => {
            // MultiEdit's top-level `file_path` is the single file
            // being edited (one file per call). Per-edit paths are
            // nested in `edits[].file_path`; we deliberately use the
            // top-level field to stay in sync with the tool's
            // single-file-per-call contract.
            let path = input.get("file_path").and_then(Value::as_str)?;
            Some((path.to_string(), FileAction::Write))
        }
        "NotebookEdit" => {
            // Notebook edits use `notebook_path`, not `file_path`.
            let path = input.get("notebook_path").and_then(Value::as_str)?;
            Some((path.to_string(), FileAction::Write))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::cli_tool_to_file_change;
    use crate::store::traces::FileAction;

    #[test]
    fn cli_tool_to_file_change_maps_known_tools() {
        let write = cli_tool_to_file_change(
            "Write",
            &serde_json::json!({"file_path": "/tmp/x.rs", "content": "y"}),
        );
        assert_eq!(write, Some(("/tmp/x.rs".into(), FileAction::Write)));

        let edit = cli_tool_to_file_change(
            "Edit",
            &serde_json::json!({"file_path": "/tmp/x.rs", "old": "a", "new": "b"}),
        );
        assert_eq!(edit, Some(("/tmp/x.rs".into(), FileAction::Write)));

        let multi = cli_tool_to_file_change(
            "MultiEdit",
            &serde_json::json!({"file_path": "/tmp/x.rs", "edits": []}),
        );
        assert_eq!(multi, Some(("/tmp/x.rs".into(), FileAction::Write)));

        let notebook = cli_tool_to_file_change(
            "NotebookEdit",
            &serde_json::json!({"notebook_path": "/tmp/nb.ipynb", "cell_id": "c1"}),
        );
        assert_eq!(notebook, Some(("/tmp/nb.ipynb".into(), FileAction::Write)));
    }

    #[test]
    fn cli_tool_to_file_change_returns_none_for_read_only_tools() {
        // Read is a read, not a mutation — no FileChange.
        assert!(cli_tool_to_file_change("Read", &serde_json::json!({"file_path": "/x"})).is_none());
        // Bash / Glob / Grep don't mutate files via this surface.
        assert!(cli_tool_to_file_change("Bash", &serde_json::json!({})).is_none());
        assert!(cli_tool_to_file_change("Glob", &serde_json::json!({})).is_none());
    }

    #[test]
    fn cli_tool_to_file_change_returns_none_for_missing_path() {
        // Write with no file_path: caller passed a malformed input.
        // We don't fabricate — return None and let the trace stream
        // record the tool_call without a file_change sidecar.
        assert!(cli_tool_to_file_change("Write", &serde_json::json!({"content": "x"})).is_none());
    }
}
