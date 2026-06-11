//! Tests for the Claude Code journey translator (feat-048).
//!
//! The 8 spec-named tests are listed first in the order they appear
//! in `feature_list.json`; each one is a focused black-box test
//! against `JourneyTranslator::on_event` / `JourneyTranslator::finish`.
//!
//! All tests build a real `TraceCollector` over an `mpsc` channel and
//! read the emitted events from the receiver. This is the same
//! pattern used in `trace::mod.rs` tests and avoids needing a mock
//! clock or a fake collector.
//!
//! Timestamp values are checked for "non-empty" rather than
//! exact-equality, since `chrono::Utc::now()` is the source.

#![cfg(test)]

use crate::agent::claude_code::JourneyTranslator;
use crate::agent::StreamEvent;
use crate::store::traces::{FileAction, TraceEvent, TraceEventKind};
use crate::trace::TraceCollector;

/// Build a `TraceCollector` over a fresh mpsc channel and return
/// both halves so the test can read what the translator emitted.
fn make_collector() -> (
    TraceCollector,
    tokio::sync::mpsc::UnboundedReceiver<TraceEvent>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (TraceCollector::new(tx), rx)
}

/// Drain all currently-pending events from the receiver. Blocks
/// (synchronously) only on the first `recv`; subsequent reads return
/// `None` once the channel is closed (the collector is dropped at
/// end of the test function) OR if the buffer is empty.
///
/// Helper used by every test below to assert on the SET of emitted
/// events without depending on the precise channel-close ordering.
fn drain(rx: &mut tokio::sync::mpsc::UnboundedReceiver<TraceEvent>) -> Vec<TraceEvent> {
    let mut out = Vec::new();
    // Try to read everything currently buffered. We use a
    // non-blocking poll via the channel's `try_recv`; the channel
    // may not be closed yet (we don't drop the collector mid-test).
    loop {
        match rx.try_recv() {
            Ok(ev) => out.push(ev),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
        }
    }
    out
}

fn assert_session_id(ev: &TraceEvent, expected: &str) {
    assert_eq!(ev.session_id, expected);
    assert!(
        !ev.timestamp.is_empty(),
        "trace event should carry a non-empty timestamp"
    );
}

// ---------------------------------------------------------------------------
// 1. test_journey_translator_text_passthrough — text deltas go to SSE only,
//    never to traces. The collector stays empty across TextDelta events.
// ---------------------------------------------------------------------------
#[test]
fn test_journey_translator_text_passthrough() {
    let (collector, mut rx) = make_collector();
    let mut t = JourneyTranslator::new("sess-1", &collector);

    t.on_event(&StreamEvent::TextDelta {
        text: "hello".into(),
    });
    t.on_event(&StreamEvent::TextDelta {
        text: " world".into(),
    });

    let events = drain(&mut rx);
    assert!(
        events.is_empty(),
        "text deltas must not produce trace events; got {events:?}"
    );
}

// ---------------------------------------------------------------------------
// 2. test_journey_translator_tool_call_recorded — a complete
//    ToolUseStart → ToolResult pair produces exactly one ToolCall
//    trace event with the CLI-provided name/input/output.
// ---------------------------------------------------------------------------
#[test]
fn test_journey_translator_tool_call_recorded() {
    let (collector, mut rx) = make_collector();
    let mut t = JourneyTranslator::new("sess-1", &collector);

    t.on_event(&StreamEvent::ToolUseStart {
        id: "tu_1".into(),
        name: "Read".into(),
        input: serde_json::json!({"file_path": "/etc/hostname"}),
    });
    t.on_event(&StreamEvent::ToolResult {
        id: "tu_1".into(),
        result: "host1\n".into(),
    });

    let events = drain(&mut rx);
    assert_eq!(events.len(), 1, "exactly one ToolCall trace event");
    let ev = &events[0];
    assert_session_id(ev, "sess-1");
    match &ev.kind {
        TraceEventKind::ToolCall {
            tool_name,
            input_json,
            output_json,
            status,
            ..
        } => {
            assert_eq!(tool_name, "Read");
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(input_json).unwrap(),
                serde_json::json!({"file_path": "/etc/hostname"})
            );
            assert_eq!(output_json, "host1\n");
            assert!(
                status.is_none(),
                "completed tool calls must have status=None; got {status:?}"
            );
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 3. test_journey_translator_tool_not_re_executed — the translator only
//    records what the CLI did. The Read tool's `result` came from the
//    CLI's tool_result line; the translator never reads the file
//    itself. Pin this with a clearly-fake path that no real
//    translator code would ever touch.
// ---------------------------------------------------------------------------
#[test]
fn test_journey_translator_tool_not_re_executed() {
    let (collector, mut rx) = make_collector();
    let mut t = JourneyTranslator::new("sess-1", &collector);

    let fake_path = "/nonexistent/__translator_must_not_read__";
    t.on_event(&StreamEvent::ToolUseStart {
        id: "tu_x".into(),
        name: "Read".into(),
        input: serde_json::json!({"file_path": fake_path}),
    });
    t.on_event(&StreamEvent::ToolResult {
        id: "tu_x".into(),
        result: "value-from-cli".into(),
    });

    let events = drain(&mut rx);
    assert_eq!(events.len(), 1);
    match &events[0].kind {
        TraceEventKind::ToolCall { output_json, .. } => {
            assert_eq!(
                output_json, "value-from-cli",
                "translator must use the CLI's tool_result, not its own read"
            );
        }
        other => panic!("expected ToolCall, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 4. test_journey_translator_file_change_recorded — a Write/Edit
//    tool_use synthesizes a sibling FileChange event with the
//    inferred path and FileAction::Write.
// ---------------------------------------------------------------------------
#[test]
fn test_journey_translator_file_change_recorded() {
    let (collector, mut rx) = make_collector();
    let mut t = JourneyTranslator::new("sess-1", &collector);

    t.on_event(&StreamEvent::ToolUseStart {
        id: "tu_w".into(),
        name: "Write".into(),
        input: serde_json::json!({"file_path": "/tmp/x.rs", "content": "y"}),
    });
    t.on_event(&StreamEvent::ToolResult {
        id: "tu_w".into(),
        result: "ok".into(),
    });

    let events = drain(&mut rx);
    assert_eq!(events.len(), 2, "expected ToolCall + FileChange");

    let mut tool_call = None;
    let mut file_change = None;
    for ev in &events {
        assert_session_id(ev, "sess-1");
        match &ev.kind {
            TraceEventKind::ToolCall { tool_name, .. } => {
                assert_eq!(tool_name, "Write");
                tool_call = Some(());
            }
            TraceEventKind::FileChange { path, action } => {
                assert_eq!(path, "/tmp/x.rs");
                assert_eq!(*action, FileAction::Write);
                file_change = Some(());
            }
            other => panic!("unexpected trace kind: {other:?}"),
        }
    }
    assert!(tool_call.is_some());
    assert!(file_change.is_some());
}

// ---------------------------------------------------------------------------
// 5. test_journey_translator_thinking_to_decision — multiple
//    consecutive Thinking events coalesce into one Decision trace
//    event, emitted on the next non-thinking event.
// ---------------------------------------------------------------------------
#[test]
fn test_journey_translator_thinking_to_decision() {
    let (collector, mut rx) = make_collector();
    let mut t = JourneyTranslator::new("sess-1", &collector);

    t.on_event(&StreamEvent::Thinking {
        text: "first thought".into(),
    });
    t.on_event(&StreamEvent::Thinking {
        text: "second thought".into(),
    });
    // A non-thinking event breaks the coalescing window — the
    // pending Decision should flush here.
    t.on_event(&StreamEvent::TextDelta {
        text: "answer".into(),
    });

    let events = drain(&mut rx);
    assert_eq!(events.len(), 1, "expected one coalesced Decision event");
    match &events[0].kind {
        TraceEventKind::Decision { text } => {
            assert_eq!(text, "first thought\nsecond thought");
        }
        other => panic!("expected Decision, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 6. test_journey_translator_error_to_error — a StreamEvent::Error
//    surfaces as a TraceEventKind::Error with the original message.
// ---------------------------------------------------------------------------
#[test]
fn test_journey_translator_error_to_error() {
    let (collector, mut rx) = make_collector();
    let mut t = JourneyTranslator::new("sess-1", &collector);

    t.on_event(&StreamEvent::Error {
        message: "permission denied".into(),
    });

    let events = drain(&mut rx);
    assert_eq!(events.len(), 1);
    match &events[0].kind {
        TraceEventKind::Error { message } => {
            assert_eq!(message, "permission denied");
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 7. test_journey_translator_orphaned_tool_use — a tool_use that
//    never receives a matching tool_result before the turn ends is
//    recorded as a ToolCall with status="orphaned" at the next Done
//    (or finish()).
// ---------------------------------------------------------------------------
#[test]
fn test_journey_translator_orphaned_tool_use() {
    let (collector, mut rx) = make_collector();
    let mut t = JourneyTranslator::new("sess-1", &collector);

    t.on_event(&StreamEvent::ToolUseStart {
        id: "tu_orphan".into(),
        name: "Bash".into(),
        input: serde_json::json!({"command": "ls"}),
    });
    // CLI exits or skips the matching tool_result. The Done event
    // signals "no more events coming" — orphan flush fires here.
    t.on_event(&StreamEvent::Done {
        stop_reason: crate::agent::StopReason::EndTurn,
    });

    let events = drain(&mut rx);
    assert_eq!(events.len(), 1, "expected one orphaned ToolCall");
    match &events[0].kind {
        TraceEventKind::ToolCall {
            tool_name,
            status,
            output_json,
            ..
        } => {
            assert_eq!(tool_name, "Bash");
            assert_eq!(status.as_deref(), Some("orphaned"));
            assert!(
                output_json.is_empty(),
                "orphaned tool_use has no result text"
            );
        }
        other => panic!("expected ToolCall with status=orphaned, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 8. test_journey_translator_dedupes_file_changes — if the CLI emits
//    multiple tool_result deltas for the same tool_use_id (a
//    wire-protocol anomaly), the translator records one FileChange
//    per tool_use_id, not one per result delta.
// ---------------------------------------------------------------------------
#[test]
fn test_journey_translator_dedupes_file_changes() {
    let (collector, mut rx) = make_collector();
    let mut t = JourneyTranslator::new("sess-1", &collector);

    t.on_event(&StreamEvent::ToolUseStart {
        id: "tu_dup".into(),
        name: "Write".into(),
        input: serde_json::json!({"file_path": "/tmp/dup.rs", "content": "x"}),
    });
    // First result delta.
    t.on_event(&StreamEvent::ToolResult {
        id: "tu_dup".into(),
        result: "partial".into(),
    });
    // Second result delta for the same id — wire anomaly, but
    // translator must dedup the FileChange.
    t.on_event(&StreamEvent::ToolResult {
        id: "tu_dup".into(),
        result: "final".into(),
    });

    let events = drain(&mut rx);
    // Two ToolCall events (one per result delta — the trace stream
    // records the latest result text) but exactly ONE FileChange.
    let tool_calls = events
        .iter()
        .filter(|e| matches!(e.kind, TraceEventKind::ToolCall { .. }))
        .count();
    let file_changes = events
        .iter()
        .filter(|e| matches!(e.kind, TraceEventKind::FileChange { .. }))
        .count();
    assert_eq!(tool_calls, 2, "one ToolCall per result delta");
    assert_eq!(file_changes, 1, "exactly one FileChange per tool_use_id");
}
