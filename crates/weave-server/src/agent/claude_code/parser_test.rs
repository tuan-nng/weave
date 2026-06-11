//! Tests for the Claude Code `stream-json` parser (feat-045).
//!
//! The 7 spec-named tests are listed first in the order they appear in
//! `feature_list.json`; the trailing 8 are robustness / coverage
//! tests that don't add scope but pin down edge cases the spec hints
//! at (unknown event types, no-pending-on-done, multiple in-flight
//! blocks, end-of-stream flush, error-event shapes, runner→parser
//! integration).
//!
//! 14 of the 15 tests are pure-parser: they feed synthetic JSON lines
//! directly to `ClaudeCodeStreamParser::feed_line` and assert on the
//! returned `Vec<StreamEvent>`. The 15th (`…_through_runner`) drives
//! the parser from `CliRunner::run`'s `LineStream` to verify the
//! parser sits correctly in the `runner → LineStream → feed_line`
//! chain that the `ClaudeCodeCodingAgent` (feat-051) will own.
//! Until the shared test-helper module lands (see TODO in the
//! runner-driven test), it re-implements the `fake_cli` path
//! resolution from `agent::fake_cli_test::fake_cli_path`.

#![cfg(test)]

use super::ClaudeCodeStreamParser;
use crate::agent::{StopReason, StreamEvent};

// ---------------------------------------------------------------------------
// 1. test_claude_code_parser_text_delta — text_delta line maps to TextDelta.
// ---------------------------------------------------------------------------
#[test]
fn test_claude_code_parser_text_delta() {
    let mut p = ClaudeCodeStreamParser::new();

    // session_id line: no event, capture.
    let r = p
        .feed_line(r#"{"type":"session_id","id":"sess-1"}"#)
        .unwrap();
    assert!(r.is_none(), "session_id produces no event");
    assert_eq!(p.session_id(), Some("sess-1"));

    // text_delta line: TextDelta.
    let r = p
        .feed_line(r#"{"type":"text_delta","text":"hello"}"#)
        .unwrap()
        .expect("text_delta emits one event");
    assert_eq!(
        r,
        vec![StreamEvent::TextDelta {
            text: "hello".into()
        }]
    );

    // done line: Done (no pending tool_use).
    let r = p
        .feed_line(r#"{"type":"done","stop_reason":"end_turn"}"#)
        .unwrap()
        .expect("done emits one event");
    assert_eq!(
        r,
        vec![StreamEvent::Done {
            stop_reason: StopReason::EndTurn
        }]
    );
}

// ---------------------------------------------------------------------------
// 2. test_claude_code_parser_tool_use_start_delta — deferred emission with
//    the incremental `input_json_delta` path. Synthetic; the fake's
//    `text+tool+done` script never exercises deltas.
// ---------------------------------------------------------------------------
#[test]
fn test_claude_code_parser_tool_use_start_delta() {
    let mut p = ClaudeCodeStreamParser::new();

    p.feed_line(r#"{"type":"session_id","id":"sess-1"}"#)
        .unwrap();

    let r = p
        .feed_line(r#"{"type":"text_delta","text":"calling"}"#)
        .unwrap()
        .expect("text_delta emits");
    assert_eq!(
        r,
        vec![StreamEvent::TextDelta {
            text: "calling".into()
        }]
    );

    // tool_use with empty input: deferred, no emit.
    let r = p
        .feed_line(r#"{"type":"tool_use","id":"t1","name":"read_file","input":{}}"#)
        .unwrap();
    assert!(r.is_none(), "tool_use is deferred; emits nothing");

    // first input_json_delta: ToolUseDelta + accumulate.
    let r = p
        .feed_line(r#"{"type":"input_json_delta","id":"t1","delta":"{\"path\":"}"#)
        .unwrap()
        .expect("delta emits");
    assert_eq!(
        r,
        vec![StreamEvent::ToolUseDelta {
            id: "t1".into(),
            delta: "{\"path\":".into()
        }]
    );

    // second input_json_delta: ToolUseDelta + accumulate.
    let r = p
        .feed_line(r#"{"type":"input_json_delta","id":"t1","delta":"\"/x\"}"}"#)
        .unwrap()
        .expect("delta emits");
    assert_eq!(
        r,
        vec![StreamEvent::ToolUseDelta {
            id: "t1".into(),
            delta: "\"/x\"}".into()
        }]
    );

    // done: flushes the pending ToolUseStart, then emits Done.
    let r = p
        .feed_line(r#"{"type":"done","stop_reason":"tool_use"}"#)
        .unwrap()
        .expect("done emits");
    assert_eq!(r.len(), 2, "expected ToolUseStart + Done");
    assert_eq!(
        r[0],
        StreamEvent::ToolUseStart {
            id: "t1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/x"})
        }
    );
    assert_eq!(
        r[1],
        StreamEvent::Done {
            stop_reason: StopReason::ToolUse
        }
    );
}

// ---------------------------------------------------------------------------
// 3. test_claude_code_parser_tool_result — field rename
//    `tool_use_id` -> `id`, `content` -> `result`.
// ---------------------------------------------------------------------------
#[test]
fn test_claude_code_parser_tool_result() {
    let mut p = ClaudeCodeStreamParser::new();

    p.feed_line(r#"{"type":"session_id","id":"sess-1"}"#)
        .unwrap();

    let r = p
        .feed_line(r#"{"type":"tool_result","tool_use_id":"t1","content":"hostname\n"}"#)
        .unwrap()
        .expect("tool_result emits");
    assert_eq!(
        r,
        vec![StreamEvent::ToolResult {
            id: "t1".into(),
            result: "hostname\n".into()
        }]
    );

    let r = p
        .feed_line(r#"{"type":"done","stop_reason":"end_turn"}"#)
        .unwrap()
        .expect("done emits");
    assert_eq!(
        r,
        vec![StreamEvent::Done {
            stop_reason: StopReason::EndTurn
        }]
    );
}

// ---------------------------------------------------------------------------
// 4. test_claude_code_parser_thinking — thinking line maps to Thinking.
// ---------------------------------------------------------------------------
#[test]
fn test_claude_code_parser_thinking() {
    let mut p = ClaudeCodeStreamParser::new();

    p.feed_line(r#"{"type":"session_id","id":"sess-1"}"#)
        .unwrap();

    let r = p
        .feed_line(r#"{"type":"thinking","text":"deep thought"}"#)
        .unwrap()
        .expect("thinking emits");
    assert_eq!(
        r,
        vec![StreamEvent::Thinking {
            text: "deep thought".into()
        }]
    );

    let r = p
        .feed_line(r#"{"type":"done","stop_reason":"end_turn"}"#)
        .unwrap()
        .expect("done emits");
    assert_eq!(r.len(), 1, "no pending tool_use; done emits alone");
}

// ---------------------------------------------------------------------------
// 5. test_claude_code_parser_session_id_capture — session_id getter + take.
// ---------------------------------------------------------------------------
#[test]
fn test_claude_code_parser_session_id_capture() {
    let mut p = ClaudeCodeStreamParser::new();

    // Before any line: None.
    assert_eq!(p.session_id(), None);
    assert_eq!(p.take_session_id(), None);

    p.feed_line(r#"{"type":"session_id","id":"sess-abc-123"}"#)
        .unwrap();

    // After session_id: captured.
    assert_eq!(p.session_id(), Some("sess-abc-123"));

    // Scenario 1: a second `session_id` line, while one is already
    // set, is ignored. Only the first captures. This matches real
    // Claude Code, which always emits a session id on the first
    // event of every turn but the parser should not re-capture on
    // mid-stream re-emission.
    p.feed_line(r#"{"type":"session_id","id":"sess-late"}"#)
        .unwrap();
    assert_eq!(
        p.session_id(),
        Some("sess-abc-123"),
        "second session_id (while one is set) is ignored"
    );

    // Scenario 2: take_session_id is one-shot.
    assert_eq!(p.take_session_id(), Some("sess-abc-123".into()));
    assert_eq!(p.session_id(), None);
    assert_eq!(p.take_session_id(), None);

    // Scenario 3: after take, a new `session_id` line captures
    // normally. This is the pattern feat-051 will use: the adapter
    // takes the captured id, writes it to
    // `Session::runtime_metadata_json`, and the next turn's parser
    // is fresh.
    p.feed_line(r#"{"type":"session_id","id":"sess-fresh"}"#)
        .unwrap();
    assert_eq!(
        p.session_id(),
        Some("sess-fresh"),
        "after take, a new session_id line captures again"
    );
}

// ---------------------------------------------------------------------------
// 6. test_claude_code_parser_done_stop_reason — table-driven over the three
//    real Claude Code stop_reasons + one defensive default.
// ---------------------------------------------------------------------------
#[test]
fn test_claude_code_parser_done_stop_reason() {
    let cases = [
        (r#""end_turn""#, StopReason::EndTurn),
        (r#""max_tokens""#, StopReason::MaxTokens),
        (r#""tool_use""#, StopReason::ToolUse),
        // Defensive default: unknown stop_reason -> EndTurn.
        (r#""refusal""#, StopReason::EndTurn),
    ];

    for (wire, expected) in cases {
        let mut p = ClaudeCodeStreamParser::new();
        p.feed_line(r#"{"type":"session_id","id":"sess-1"}"#)
            .unwrap();
        let line = format!(r#"{{"type":"done","stop_reason":{wire}}}"#);
        let r = p.feed_line(&line).unwrap().expect("done emits");
        assert_eq!(
            r,
            vec![StreamEvent::Done {
                stop_reason: expected.clone()
            }],
            "wire stop_reason={wire} should map to {:?}",
            expected
        );
    }
}

// ---------------------------------------------------------------------------
// 7. test_claude_code_parser_malformed_line_skipped — parser never aborts on
//    bad input; it keeps consuming subsequent valid lines.
// ---------------------------------------------------------------------------
#[test]
fn test_claude_code_parser_malformed_line_skipped() {
    let mut p = ClaudeCodeStreamParser::new();

    // Garbage: not JSON at all.
    let r = p.feed_line("this is not json at all").unwrap();
    assert!(r.is_none(), "garbage line: no event");

    // Truncated JSON: missing closing brace.
    let r = p.feed_line(r#"{"type":"text_delta","text":"hi""#).unwrap();
    assert!(r.is_none(), "truncated JSON: no event");

    // JSON without `type` field: warn-skipped.
    let r = p.feed_line(r#"{"id":"foo"}"#).unwrap();
    assert!(r.is_none(), "missing type field: no event");

    // After all the bad input, the parser is still alive and
    // correctly handles a valid line.
    let r = p
        .feed_line(r#"{"type":"text_delta","text":"after malformed"}"#)
        .unwrap()
        .expect("valid line after malformed emits");
    assert_eq!(
        r,
        vec![StreamEvent::TextDelta {
            text: "after malformed".into()
        }]
    );

    // And `done` still works.
    let r = p
        .feed_line(r#"{"type":"done","stop_reason":"end_turn"}"#)
        .unwrap()
        .expect("done emits");
    assert_eq!(
        r,
        vec![StreamEvent::Done {
            stop_reason: StopReason::EndTurn
        }]
    );
}

// ---------------------------------------------------------------------------
// 8. test_claude_code_parser_unknown_event_skipped — forward-compat: a
//    future event type is warn-logged and skipped, parser stays alive.
// ---------------------------------------------------------------------------
#[test]
fn test_claude_code_parser_unknown_event_skipped() {
    let mut p = ClaudeCodeStreamParser::new();
    p.feed_line(r#"{"type":"session_id","id":"sess-1"}"#)
        .unwrap();

    // Future Claude Code event: unknown to today's parser.
    let r = p
        .feed_line(r#"{"type":"future_event","data":"unknown"}"#)
        .unwrap();
    assert!(r.is_none(), "unknown event type: no event");

    // Parser is still alive.
    let r = p
        .feed_line(r#"{"type":"text_delta","text":"alive"}"#)
        .unwrap()
        .expect("valid line after unknown event");
    assert_eq!(
        r,
        vec![StreamEvent::TextDelta {
            text: "alive".into()
        }]
    );
}

// ---------------------------------------------------------------------------
// 9. test_claude_code_parser_done_with_no_pending_emits_only_done — when
//    no tool_use is in flight, `done` returns just the Done event.
// ---------------------------------------------------------------------------
#[test]
fn test_claude_code_parser_done_with_no_pending_emits_only_done() {
    let mut p = ClaudeCodeStreamParser::new();
    p.feed_line(r#"{"type":"session_id","id":"sess-1"}"#)
        .unwrap();
    p.feed_line(r#"{"type":"text_delta","text":"hi"}"#).unwrap();

    let r = p
        .feed_line(r#"{"type":"done","stop_reason":"end_turn"}"#)
        .unwrap()
        .expect("done emits");
    assert_eq!(
        r,
        vec![StreamEvent::Done {
            stop_reason: StopReason::EndTurn
        }],
        "no tool_use was in flight; done emits alone"
    );
}

// ---------------------------------------------------------------------------
// 10. test_claude_code_parser_multiple_pending_tool_uses_flush_in_order —
//     tool_use ids that are NOT lex-ordered relative to insertion
//     order, so the test actually verifies insertion order (a
//     BTreeMap would silently flush in lex order and the test
//     would pass by accident with `t1, t2`). Real Claude Code uses
//     opaque ids like `toolu_01T7K...` whose lex order is
//     unrelated to the order the model announced the tools.
// ---------------------------------------------------------------------------
#[test]
fn test_claude_code_parser_multiple_pending_tool_uses_flush_in_order() {
    let mut p = ClaudeCodeStreamParser::new();
    p.feed_line(r#"{"type":"session_id","id":"sess-1"}"#)
        .unwrap();

    // Insertion order: toolu_b first, then toolu_a.
    // Lex order:     toolu_a < toolu_b.
    // The flush must follow insertion order, NOT lex order.

    // First tool_use (inserted first; lex-larger id).
    let r = p
        .feed_line(r#"{"type":"tool_use","id":"toolu_b","name":"read_file","input":{"path":"/b"}}"#)
        .unwrap();
    assert!(r.is_none(), "first tool_use deferred");

    // Second tool_use (inserted second; lex-smaller id).
    let r = p
        .feed_line(
            r#"{"type":"tool_use","id":"toolu_a","name":"write_file","input":{"path":"/a","content":"y"}}"#,
        )
        .unwrap();
    assert!(r.is_none(), "second tool_use deferred");

    // done: flushes both in INSERTION order (toolu_b before toolu_a).
    let r = p
        .feed_line(r#"{"type":"done","stop_reason":"tool_use"}"#)
        .unwrap()
        .expect("done emits");
    assert_eq!(r.len(), 3, "expected 2 ToolUseStarts + 1 Done");
    assert_eq!(
        r[0],
        StreamEvent::ToolUseStart {
            id: "toolu_b".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/b"})
        }
    );
    assert_eq!(
        r[1],
        StreamEvent::ToolUseStart {
            id: "toolu_a".into(),
            name: "write_file".into(),
            input: serde_json::json!({"path": "/a", "content": "y"})
        }
    );
    assert_eq!(
        r[2],
        StreamEvent::Done {
            stop_reason: StopReason::ToolUse
        }
    );
}

// ---------------------------------------------------------------------------
// 11. test_claude_code_parser_flush_drains_pending_at_end_of_stream — when
//     the CLI exits without a `done` line (crash, cancel), the consumer
//     calls `flush` to recover the in-flight tool_use blocks.
// ---------------------------------------------------------------------------
#[test]
fn test_claude_code_parser_flush_drains_pending_at_end_of_stream() {
    let mut p = ClaudeCodeStreamParser::new();
    p.feed_line(r#"{"type":"session_id","id":"sess-1"}"#)
        .unwrap();
    p.feed_line(r#"{"type":"tool_use","id":"t1","name":"read_file","input":{"path":"/x"}}"#)
        .unwrap();
    // No `done` line; CLI exited or was cancelled.
    // flush should drain the pending tool_use.
    let r = p.flush();
    assert_eq!(
        r,
        Some(StreamEvent::ToolUseStart {
            id: "t1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/x"})
        })
    );
    // Second flush returns None.
    assert_eq!(p.flush(), None);
}

// ---------------------------------------------------------------------------
// 11b. test_claude_code_parser_drain_pending_returns_all_in_flight —
//      the `drain_pending` method (used by the agent's truncated-stream
//      recovery path) must return EVERY in-flight tool_use in
//      registration order, not just the first. A truncated stream
//      (CLI crash, cancel mid-`tool_use`) leaves the parser with
//      multiple pending blocks; dropping all but the first would lose
//      the model's tool calls on the SSE wire.
//
//      Triggered by the code-review feedback on feat-051 — the
//      single-event `flush` shim silently dropped events when the
//      agent called it in a `while let Some(...)` loop. This test
//      locks in the corrected `drain_pending` contract.
// ---------------------------------------------------------------------------
#[test]
fn test_claude_code_parser_drain_pending_returns_all_in_flight() {
    let mut p = ClaudeCodeStreamParser::new();
    p.feed_line(r#"{"type":"session_id","id":"sess-1"}"#)
        .unwrap();
    p.feed_line(r#"{"type":"tool_use","id":"toolu_a","name":"read_file","input":{"path":"/a"}}"#)
        .unwrap();
    p.feed_line(r#"{"type":"tool_use","id":"toolu_b","name":"write_file","input":{"path":"/b"}}"#)
        .unwrap();
    // No `done` line — CLI exited non-zero mid-stream.
    let r = p.drain_pending();
    assert_eq!(
        r.len(),
        2,
        "drain_pending must return every in-flight tool_use, got {r:?}"
    );
    assert_eq!(
        r[0],
        StreamEvent::ToolUseStart {
            id: "toolu_a".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/a"})
        }
    );
    assert_eq!(
        r[1],
        StreamEvent::ToolUseStart {
            id: "toolu_b".into(),
            name: "write_file".into(),
            input: serde_json::json!({"path": "/b"})
        }
    );
    // A second drain is a no-op — the in-flight map is empty.
    assert!(p.drain_pending().is_empty());
    // `flush` (single-event back-compat shim) also returns None now.
    assert_eq!(p.flush(), None);
}

// ---------------------------------------------------------------------------
// 12. test_claude_code_parser_tool_use_with_complete_input — the fake's
//     `text+tool+done` shape: `tool_use` carries a complete `input` AND
//     no subsequent `input_json_delta`. The parser must still defer to
//     `done` and emit the assembled `Value` at that point.
// ---------------------------------------------------------------------------
#[test]
fn test_claude_code_parser_tool_use_with_complete_input() {
    let mut p = ClaudeCodeStreamParser::new();
    p.feed_line(r#"{"type":"session_id","id":"sess-1"}"#)
        .unwrap();

    // The fake's exact shape (see src/bin/fake_cli.rs:script_text_plus_tool).
    let r = p
        .feed_line(
            r#"{"type":"tool_use","id":"tool_use_1","name":"read_file","input":{"path":"/etc/hostname"}}"#,
        )
        .unwrap();
    assert!(r.is_none(), "deferred; no emit on tool_use");

    let r = p
        .feed_line(r#"{"type":"done","stop_reason":"tool_use"}"#)
        .unwrap()
        .expect("done emits");
    assert_eq!(r.len(), 2);
    assert_eq!(
        r[0],
        StreamEvent::ToolUseStart {
            id: "tool_use_1".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "/etc/hostname"})
        }
    );
    assert_eq!(
        r[1],
        StreamEvent::Done {
            stop_reason: StopReason::ToolUse
        }
    );
}

// ---------------------------------------------------------------------------
// 13. test_claude_code_parser_error_with_message — in-band error events
//     surface as StreamEvent::Error.
// ---------------------------------------------------------------------------
#[test]
fn test_claude_code_parser_error_with_message() {
    let mut p = ClaudeCodeStreamParser::new();
    p.feed_line(r#"{"type":"session_id","id":"sess-1"}"#)
        .unwrap();

    let r = p
        .feed_line(
            r#"{"type":"error","code":"permission_denied","message":"Permission denied: cannot write to /etc/passwd"}"#,
        )
        .unwrap()
        .expect("error emits");
    assert_eq!(
        r,
        vec![StreamEvent::Error {
            message: "Permission denied: cannot write to /etc/passwd".into()
        }]
    );
}

// ---------------------------------------------------------------------------
// 14. test_claude_code_parser_error_with_code_only — error events that
//     carry only `code` (no `message`) still surface as StreamEvent::Error
//     with a synthesized message.
// ---------------------------------------------------------------------------
#[test]
fn test_claude_code_parser_error_with_code_only() {
    let mut p = ClaudeCodeStreamParser::new();
    p.feed_line(r#"{"type":"session_id","id":"sess-1"}"#)
        .unwrap();

    let r = p
        .feed_line(r#"{"type":"error","code":"resume_unknown_session"}"#)
        .unwrap()
        .expect("error emits");
    assert_eq!(
        r,
        vec![StreamEvent::Error {
            message: "error code: resume_unknown_session".into()
        }]
    );
}

// ---------------------------------------------------------------------------
// 15. test_claude_code_parser_session_id_capture_through_runner — end-to-end:
//     the parser's `session_id` getter receives the value the runner
//     delivered from the fake's first stdout line. This is the
//     runner→parser chain that the `ClaudeCodeCodingAgent` (feat-051)
//     will own. Mirrors `test_fake_cli_echoes_resume_id` in
//     `agent::fake_cli_test.rs` but from the parser's perspective.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_claude_code_parser_session_id_capture_through_runner() {
    use crate::agent::cli_runner::{
        test_support::run_with_timeout, CliInvocation, CliRunResult, CliRunner, LineStream,
    };
    use crate::agent::turn_context::test_support::make_test_turn_context;
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    // Locate the `fake_cli` binary. Same strategy as
    // `agent::fake_cli_test::fake_cli_path`; duplicated here because
    // that helper is private to its module. TODO: hoist both
    // `fake_cli_test` and `parser_test` helpers into a shared
    // `agent::test_support` module when feat-051 (ClaudeCode adapter)
    // and feat-057 (conformance suite) need them — at that point
    // three call sites will justify the extraction.
    let fake_cli_path = if let Ok(p) = std::env::var("CARGO_BIN_EXE_fake_cli") {
        PathBuf::from(p)
    } else {
        std::env::current_exe()
            .expect("current_exe is set")
            .parent()
            .and_then(|p| p.parent())
            .expect("target/debug/ is the binary output dir")
            .join("fake_cli")
    };

    let env = BTreeMap::from([("FAKE_CLI_SCRIPT".into(), "echo-resume-id".into())]);
    let inv = CliInvocation {
        binary: fake_cli_path,
        args: vec!["--resume".into(), "captured-id-789".into()],
        env,
        cwd: PathBuf::from("."),
        stdin_payload: None,
    };

    let runner = CliRunner::new();
    let result = run_with_timeout(&runner, inv, make_test_turn_context())
        .await
        .expect("run must succeed");

    let mut stdout: LineStream = match result {
        CliRunResult::Success { stdout, .. } => stdout,
        other => panic!("expected Success, got: {other:?}"),
    };

    // Drive the parser from the runner's LineStream.
    let mut parser = ClaudeCodeStreamParser::new();
    while let Some(line) = stdout.next().await {
        let _ = parser
            .feed_line(&line)
            .expect("feed_line never returns Err in normal operation");
    }
    // At end-of-stream, flush any pending tool_uses (none in this script).
    let _ = parser.flush();

    // The parser captured the session_id from the fake's first event,
    // which itself echoed the `--resume captured-id-789` value. This is
    // the value feat-047 will persist into
    // `Session::runtime_metadata_json['cli_resume_id']`.
    assert_eq!(
        parser.session_id(),
        Some("captured-id-789"),
        "parser must capture the session_id emitted by the runner's first stdout line"
    );
}
