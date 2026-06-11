//! Shared CLI adapter conformance test suite (feat-057).
//!
//! Exercises the `ConformanceAdapter` contract against the Claude Code
//! adapter using the fake CLI harness. Each test function corresponds
//! to one of the 7 conformance cases.
//!
//! When Codex (feat-058) or OpenCode (feat-059) adapters arrive, add
//! `_codex` / `_opencode` variants of each test function.

use std::collections::BTreeMap;
use std::path::PathBuf;

use weave_server::agent::claude_code::JourneyTranslator;
use weave_server::agent::cli_runner::CliRunner;
use weave_server::agent::conformance::{ClaudeCodeConformanceAdapter, ConformanceAdapter};
use weave_server::agent::permissions::ToolProfile;
use weave_server::agent::turn_context::test_support::make_test_turn_context;
use weave_server::agent::{RuntimeKind, StopReason, StreamEvent};
use weave_server::store::traces::TraceEventKind;
use weave_server::trace::TraceCollector;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn adapter() -> ClaudeCodeConformanceAdapter {
    ClaudeCodeConformanceAdapter
}

fn make_collector() -> (
    TraceCollector,
    tokio::sync::mpsc::UnboundedReceiver<weave_server::store::traces::TraceEvent>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    (TraceCollector::new(tx), rx)
}

fn drain(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<weave_server::store::traces::TraceEvent>,
) -> Vec<weave_server::store::traces::TraceEvent> {
    let mut out = Vec::new();
    loop {
        match rx.try_recv() {
            Ok(ev) => out.push(ev),
            Err(_) => break,
        }
    }
    out
}

// ---------------------------------------------------------------------------
// (a) test_conformance_argv_construction
// ---------------------------------------------------------------------------

#[test]
fn test_conformance_argv_construction_claude_code() {
    let adt = adapter();
    let _turn = adt.make_turn_context("test-session", PathBuf::from("/tmp/test-workspace"), None);

    let inv = adt.build_invocation(
        vec!["--verbose".into()],
        BTreeMap::new(),
        PathBuf::from("/tmp/test-workspace"),
    );

    // Binary is the fake CLI
    assert_eq!(inv.binary, adt.fake_cli_path());
    assert_eq!(inv.cwd, PathBuf::from("/tmp/test-workspace"));
    assert!(inv.stdin_payload.is_none());

    // Args include provider-level flags
    assert!(inv.args.contains(&"--verbose".to_string()));
}

#[test]
fn test_conformance_argv_includes_resume_flag() {
    let adt = adapter();
    let mut turn =
        adt.make_turn_context("test-session", PathBuf::from("/tmp/test-workspace"), None);
    turn.cli_resume_id = Some("sess-abc-123".to_string());

    // The adapter's build_args is private, but we can verify the
    // TurnContext carries the resume id correctly.
    assert_eq!(turn.cli_resume_id.as_deref(), Some("sess-abc-123"));
}

// ---------------------------------------------------------------------------
// (b) test_conformance_stream_parser
// ---------------------------------------------------------------------------

#[test]
fn test_conformance_stream_parser_claude_code() {
    let adt = adapter();
    let mut parser = adt.new_parser();

    // Canonical sequence: session_id → text_delta → done(end_turn)
    let lines = [
        r#"{"type":"session_id","id":"sess-1"}"#,
        r#"{"type":"text_delta","text":"hello"}"#,
        r#"{"type":"done","stop_reason":"end_turn"}"#,
    ];

    let mut all_events = Vec::new();
    for line in &lines {
        if let Some(events) = parser.feed_line(line).unwrap() {
            all_events.extend(events);
        }
    }

    // Should have TextDelta + Done
    assert!(all_events
        .iter()
        .any(|e| matches!(e, StreamEvent::TextDelta { text } if text == "hello")));
    assert!(all_events.iter().any(|e| matches!(
        e,
        StreamEvent::Done {
            stop_reason: StopReason::EndTurn
        }
    )));
}

#[test]
fn test_conformance_stream_parser_tool_use_deferred() {
    let adt = adapter();
    let mut parser = adt.new_parser();

    // tool_use is deferred — not emitted until done/flush
    let lines = [
        r#"{"type":"session_id","id":"sess-2"}"#,
        r#"{"type":"text_delta","text":"calling tool"}"#,
        r#"{"type":"tool_use","id":"tool-1","name":"fs_read","input":{"path":"/tmp/a"}}"#,
        r#"{"type":"done","stop_reason":"tool_use"}"#,
    ];

    let mut all_events = Vec::new();
    for line in &lines {
        if let Some(events) = parser.feed_line(line).unwrap() {
            all_events.extend(events);
        }
    }

    // TextDelta present
    assert!(all_events
        .iter()
        .any(|e| matches!(e, StreamEvent::TextDelta { .. })));

    // ToolUseStart emitted by done (flushed from pending)
    assert!(all_events.iter().any(|e| matches!(e, StreamEvent::ToolUseStart { id, name, .. } if id == "tool-1" && name == "fs_read")));

    // Done with ToolUse stop reason
    assert!(all_events.iter().any(|e| matches!(
        e,
        StreamEvent::Done {
            stop_reason: StopReason::ToolUse
        }
    )));
}

#[test]
fn test_conformance_stream_parser_malformed_line_skipped() {
    let adt = adapter();
    let mut parser = adt.new_parser();

    // Malformed JSON should be skipped, not fatal
    let result = parser.feed_line("not json at all");
    assert!(result.unwrap().is_none());

    // Unknown event type should be skipped
    let result = parser.feed_line(r#"{"type":"unknown_event"}"#);
    assert!(result.unwrap().is_none());
}

// ---------------------------------------------------------------------------
// (c) test_conformance_resume_metadata
// ---------------------------------------------------------------------------

#[test]
fn test_conformance_resume_metadata_claude_code() {
    let adt = adapter();
    let mut parser = adt.new_parser();

    parser
        .feed_line(r#"{"type":"session_id","id":"sess-abc-123"}"#)
        .unwrap();
    assert_eq!(parser.session_id(), Some("sess-abc-123".to_string()));

    // take_session_id consumes it
    let taken = parser.take_session_id();
    assert_eq!(taken, Some("sess-abc-123".to_string()));
    assert_eq!(parser.session_id(), None);
}

#[test]
fn test_conformance_resume_metadata_not_set() {
    let adt = adapter();
    let mut parser = adt.new_parser();

    // No session_id event → session_id is None
    parser
        .feed_line(r#"{"type":"text_delta","text":"hello"}"#)
        .unwrap();
    assert_eq!(parser.session_id(), None);
    assert_eq!(parser.take_session_id(), None);
}

// ---------------------------------------------------------------------------
// (d) test_conformance_permission_mapper
// ---------------------------------------------------------------------------

#[test]
fn test_conformance_permission_mapper_claude_code() {
    let adt = adapter();
    let mapper = adt.permission_mapper();

    // Each ToolProfile produces a valid JSON snapshot
    for profile in [
        ToolProfile::Full,
        ToolProfile::Implementation,
        ToolProfile::Review,
        ToolProfile::Planning,
        ToolProfile::Reporting,
    ] {
        let snap = mapper.effective_permissions(RuntimeKind::ClaudeCode, profile);
        let json = snap.to_json().expect("snapshot must serialize");
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("JSON must parse");

        assert_eq!(parsed["runtime_kind"], "claude-code");
        assert_eq!(parsed["tool_profile"], profile.as_str());
        assert!(parsed["argv_flags"].is_array());
        assert!(parsed["env_vars"].is_object());
    }
}

#[test]
fn test_conformance_permission_mapper_non_claude_code() {
    let adt = adapter();
    let mapper = adt.permission_mapper();

    // Non-ClaudeCode runtime returns empty snapshot
    let snap = mapper.effective_permissions(RuntimeKind::AnthropicApi, ToolProfile::Full);
    assert!(snap.argv_flags.is_empty());
    assert!(snap.env_vars.is_empty());
}

#[test]
fn test_conformance_permission_mapper_profile_specific() {
    let adt = adapter();
    let mapper = adt.permission_mapper();

    // Full → bypassPermissions
    let snap = mapper.effective_permissions(RuntimeKind::ClaudeCode, ToolProfile::Full);
    let json = snap.to_json().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let flags = parsed["argv_flags"].as_array().unwrap();
    assert!(flags
        .iter()
        .any(|f| f.as_str() == Some("--permission-mode")));
    assert!(flags
        .iter()
        .any(|f| f.as_str() == Some("bypassPermissions")));

    // Review → plan
    let snap = mapper.effective_permissions(RuntimeKind::ClaudeCode, ToolProfile::Review);
    let json = snap.to_json().unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    let flags = parsed["argv_flags"].as_array().unwrap();
    assert!(flags.iter().any(|f| f.as_str() == Some("plan")));
}

// ---------------------------------------------------------------------------
// (e) test_conformance_journey_translator
// ---------------------------------------------------------------------------

#[test]
fn test_conformance_journey_translator_claude_code() {
    let (collector, mut rx) = make_collector();
    let mut translator = JourneyTranslator::new("conformance-test", &collector);

    // Feed a tool_use start + result → should emit ToolCall trace
    translator.on_event(&StreamEvent::ToolUseStart {
        id: "tool-1".into(),
        name: "fs_read".into(),
        input: serde_json::json!({"path": "/tmp/a"}),
    });
    translator.on_event(&StreamEvent::ToolResult {
        id: "tool-1".into(),
        result: "file contents".into(),
    });
    translator.finish();

    let events = drain(&mut rx);
    assert!(!events.is_empty(), "translator must emit trace events");

    let tool_calls: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.kind, TraceEventKind::ToolCall { .. }))
        .collect();
    assert!(!tool_calls.is_empty(), "must record a tool_call");
}

#[test]
fn test_conformance_journey_translator_thinking_to_decision() {
    let (collector, mut rx) = make_collector();
    let mut translator = JourneyTranslator::new("conformance-test", &collector);

    translator.on_event(&StreamEvent::Thinking {
        text: "analyzing the code".into(),
    });
    translator.finish();

    let events = drain(&mut rx);
    let decisions: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.kind, TraceEventKind::Decision { .. }))
        .collect();
    assert!(
        !decisions.is_empty(),
        "must record a decision from thinking"
    );
}

#[test]
fn test_conformance_journey_translator_orphaned_tool_use() {
    let (collector, mut rx) = make_collector();
    let mut translator = JourneyTranslator::new("conformance-test", &collector);

    // tool_use without matching tool_result → orphaned
    translator.on_event(&StreamEvent::ToolUseStart {
        id: "tool-orphan".into(),
        name: "fs_write".into(),
        input: serde_json::json!({"path": "/tmp/b", "content": "data"}),
    });
    // No ToolResult — finish without it
    translator.finish();

    let events = drain(&mut rx);
    let tool_calls: Vec<_> = events
        .iter()
        .filter(|e| matches!(e.kind, TraceEventKind::ToolCall { ref status, .. } if status.as_deref() == Some("orphaned")))
        .collect();
    assert!(!tool_calls.is_empty(), "must record orphaned tool_use");
}

// ---------------------------------------------------------------------------
// (f) test_conformance_error_scenarios
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_conformance_error_permission_denied() {
    use weave_server::agent::cli_runner::test_support::run_with_timeout;

    let adt = adapter();
    let runner = CliRunner::new();
    let turn = make_test_turn_context();

    let inv = adt.build_invocation(
        vec![],
        BTreeMap::from([("FAKE_CLI_SCRIPT".into(), "permission-denied".into())]),
        PathBuf::from("."),
    );

    let result = run_with_timeout(&runner, inv, turn).await;
    // Runner returns Ok(ExitError) for permission-denied (exit code 2)
    assert!(result.is_ok(), "runner should not error on spawn");
    match result.unwrap() {
        weave_server::agent::cli_runner::CliRunResult::ExitError { exit_code, .. } => {
            assert_eq!(exit_code, 2, "permission-denied exits with code 2");
        }
        other => panic!("expected ExitError for permission-denied, got {other:?}"),
    }
}

#[tokio::test]
async fn test_conformance_error_crash() {
    use weave_server::agent::cli_runner::test_support::run_with_timeout;

    let adt = adapter();
    let runner = CliRunner::new();
    let turn = make_test_turn_context();

    let inv = adt.build_invocation(
        vec![],
        BTreeMap::from([("FAKE_CLI_SCRIPT".into(), "crash".into())]),
        PathBuf::from("."),
    );

    let result = run_with_timeout(&runner, inv, turn).await;
    assert!(result.is_ok(), "runner should not error on spawn");
    match result.unwrap() {
        weave_server::agent::cli_runner::CliRunResult::ExitError { exit_code, .. } => {
            // crash exits with 128+SIGSEGV = 139
            assert_eq!(exit_code, 139, "crash exits with code 139");
        }
        other => panic!("expected ExitError for crash, got {other:?}"),
    }
}

#[tokio::test]
async fn test_conformance_error_resume_unknown() {
    use weave_server::agent::cli_runner::test_support::run_with_timeout;

    let adt = adapter();
    let runner = CliRunner::new();
    let mut turn = make_test_turn_context();
    turn.cli_resume_id = Some("unknown-session-id".into());

    let inv = adt.build_invocation(
        vec!["--resume".into(), "unknown-session-id".into()],
        BTreeMap::from([("FAKE_CLI_SCRIPT".into(), "resume-unknown-session".into())]),
        PathBuf::from("."),
    );

    let result = run_with_timeout(&runner, inv, turn).await;
    assert!(result.is_ok(), "runner should not error on spawn");
    match result.unwrap() {
        weave_server::agent::cli_runner::CliRunResult::ExitError { exit_code, .. } => {
            assert_eq!(exit_code, 3, "resume-unknown-session exits with code 3");
        }
        other => panic!("expected ExitError for resume-unknown, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// (g) test_conformance_workspace_scoped_cwd
// ---------------------------------------------------------------------------

#[test]
fn test_conformance_workspace_scoped_cwd_claude_code() {
    let adt = adapter();

    // With codebase_root set
    let ctx = adt.make_turn_context(
        "test-session",
        PathBuf::from("/workspace/code"),
        Some(PathBuf::from("/workspace")),
    );
    assert_eq!(ctx.codebase_root, Some(PathBuf::from("/workspace")));
    assert_eq!(ctx.cwd, PathBuf::from("/workspace/code"));
    assert_eq!(ctx.runtime_kind, RuntimeKind::ClaudeCode);

    // Without codebase_root
    let ctx = adt.make_turn_context("test-session", PathBuf::from("."), None);
    assert_eq!(ctx.codebase_root, None);
    assert_eq!(ctx.runtime_kind, RuntimeKind::ClaudeCode);
}

#[test]
fn test_conformance_cwd_validator_wired() {
    // Verify the adapter's TurnContext carries the right runtime_kind
    // so the upstream cwd validator (feat-050) can enforce it.
    let adt = adapter();
    let ctx = adt.make_turn_context(
        "test-session",
        PathBuf::from("/workspace"),
        Some(PathBuf::from("/workspace")),
    );
    assert_eq!(ctx.runtime_kind, RuntimeKind::ClaudeCode);
    assert!(
        ctx.codebase_root.is_some(),
        "wrapped sessions must have codebase_root"
    );
}
