//! End-to-end tests for the wrapped (CLI) session runtime (feat-051).
//!
//! These tests cover the full path the `ClaudeCodeCodingAgent` exercises
//! in production: registry → `CodingAgent::send_message` → `CliRunner` →
//! `fake_cli` binary → `ProviderRegistry::turn_outcomes` side-channel.
//!
//! Each test stands up a real in-memory `ProviderRegistry` (with the
//! same `ActiveChildProcesses` and `turn_outcomes` side-channel the
//! production HTTP path uses) and a `fake_cli` binary configured via
//! `FAKE_CLI_SCRIPT`. The agent-under-test is the real
//! `ClaudeCodeCodingAgent` — no mocking of the per-turn subprocess.
//!
//! ## Why this lives next to `sessions.rs` (not in `agent/claude_code`)
//!
//! The 7 e2e flows verify `service::sessions` end-to-end — the side
//! channel, the resume-state decision (feat-047), and the SSE event
//! shape are all part of the contract. The agent-level tests for
//! the parser, translator, and runner are already in
//! `agent::claude_code::*_test`; this file is the cross-layer wiring.

#![cfg(test)]

use std::path::PathBuf;
use std::sync::Arc;

use futures_core::Stream;
use futures_util::stream::StreamExt;

use crate::agent::claude_code::ClaudeCodeCodingAgent;
use crate::agent::registry::{ProviderRegistry, TurnOutcome};
use crate::agent::turn_context::TurnContext;
use crate::agent::{CodingAgent, MessageRequest, StreamEvent};
use crate::service::ActiveChildProcesses;

// --- helpers ---------------------------------------------------------------

fn fake_cli_path() -> PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_fake_cli") {
        return PathBuf::from(p);
    }
    let exe = std::env::current_exe().expect("current_exe is set");
    exe.parent()
        .and_then(|p| p.parent())
        .expect("target/debug/")
        .join("fake_cli")
}

/// Build a `ClaudeCodeCodingAgent` whose CLI is `fake_cli`. The
/// `script` is forwarded as `FAKE_CLI_SCRIPT` to control the wire
/// output. The `permission_mode` is set to a noop sentinel — the
/// tests below don't assert on argv; they assert on the
/// `StreamEvent`s the agent emits.
fn build_agent(script: &str, registry: &Arc<ProviderRegistry>) -> Arc<ClaudeCodeCodingAgent> {
    let path = fake_cli_path();
    let mut env = std::collections::BTreeMap::new();
    env.insert("FAKE_CLI_SCRIPT".to_string(), script.to_string());
    let outcomes = registry.turn_outcomes_arc();
    let procs = registry.active_child_processes();
    Arc::new(ClaudeCodeCodingAgent::new(
        path,
        vec![],
        env,
        "claude-sonnet-4-20250514".to_string(),
        "acceptEdits".to_string(),
        procs,
        outcomes,
    ))
}

fn new_registry() -> Arc<ProviderRegistry> {
    Arc::new(ProviderRegistry::with_shared_process_registry(Arc::new(
        ActiveChildProcesses::new(),
    )))
}

fn make_turn(session_id: &str) -> TurnContext {
    TurnContext {
        session_id: session_id.to_string(),
        workspace_id: "w-1".to_string(),
        cwd: PathBuf::from("."),
        runtime_kind: crate::agent::RuntimeKind::ClaudeCode,
        effective_permissions: crate::agent::permissions::PermissionSnapshot {
            runtime_kind: crate::agent::RuntimeKind::ClaudeCode,
            tool_profile: crate::agent::permissions::ToolProfile::Full,
            argv_flags: Vec::new(),
            env_vars: std::collections::BTreeMap::new(),
        },
        codebase_root: None,
        cli_resume_id: None,
        cancellation_token: tokio_util::sync::CancellationToken::new(),
    }
}

fn text_request(prompt: &str) -> MessageRequest {
    MessageRequest {
        messages: vec![crate::agent::Message {
            role: crate::agent::Role::User,
            content: crate::agent::Content::Text(prompt.to_string()),
        }],
        system: None,
        max_tokens: 1024,
        model: "claude-sonnet-4-20250514".to_string(),
        tools: None,
    }
}

/// Collect every `StreamEvent` from the agent's stream into a `Vec`.
async fn drain<S>(mut s: S) -> Vec<StreamEvent>
where
    S: Stream<Item = Result<StreamEvent, crate::error::ProviderError>> + Unpin,
{
    let mut out = Vec::new();
    while let Some(ev) = s.next().await {
        match ev {
            Ok(ev) => out.push(ev),
            Err(e) => panic!("agent stream returned error: {e}"),
        }
    }
    out
}

// --- tests -----------------------------------------------------------------

/// 1. Creating a `ClaudeCodeCodingAgent` and calling `provider_type`
/// and `display_name` returns the runtime-tool shape. This is the
/// minimum "did the wiring build" gate.
#[tokio::test]
async fn test_claude_code_wrapped_session_create() {
    let registry = new_registry();
    let agent = build_agent("text-only", &registry);

    assert_eq!(agent.provider_type(), "claude-code");
    assert_eq!(agent.display_name(), "Claude Code");
    // list_models is empty for CLI runtimes — the binary enumerates
    // models itself (the wire format doesn't list them).
    let models = agent.list_models().await.expect("list_models");
    assert!(models.is_empty());
}

/// 2. The agent's `send_message` stream emits a `TextDelta` for the
/// `text-only` fake_cli script, followed by a `Done`. The 7-test suite
/// locks in this end-to-end shape.
#[tokio::test]
async fn test_claude_code_wrapped_streams_via_sse() {
    let registry = new_registry();
    let agent = build_agent("text-only", &registry);

    let turn = make_turn("s-stream");
    let stream = agent
        .send_message(text_request("hello"), &turn)
        .await
        .expect("send_message");
    let events = drain(stream).await;

    // At least one TextDelta.
    let text_count = events
        .iter()
        .filter(|e| matches!(e, StreamEvent::TextDelta { .. }))
        .count();
    assert!(text_count >= 1, "expected ≥1 TextDelta, got {events:?}");

    // Last event must be Done (EndTurn).
    match events.last() {
        Some(StreamEvent::Done {
            stop_reason: crate::agent::StopReason::EndTurn,
        }) => {}
        other => panic!("expected Done(EndTurn) at end, got: {other:?}"),
    }

    // The side-channel map has the captured `cli_resume_id`.
    let outcome = registry
        .turn_outcomes_arc()
        .lock()
        .unwrap()
        .get("s-stream")
        .cloned();
    assert!(outcome.is_some(), "turn_outcome should be stashed");
}

/// 3. Turn 1 writes a captured id; turn 2 reads it and the agent sees
/// it via the `cli_resume_id` field on the TurnContext. The CLI
/// accepts the `--resume <id>` and emits the same `session_id` back
/// (real Claude Code's behavior; the fake mirrors it).
#[tokio::test]
async fn test_claude_code_wrapped_resume_first_turn_native_second() {
    let registry = new_registry();
    let agent = build_agent("echo-resume-id", &registry);

    // Turn 1: no cli_resume_id — the CLI generates a fresh id and
    // emits it as the first `session_id` line.
    let turn1 = make_turn("s-resume");
    let events1 = drain(
        agent
            .send_message(text_request("hi"), &turn1)
            .await
            .unwrap(),
    )
    .await;
    assert!(matches!(events1.last(), Some(StreamEvent::Done { .. })));
    let outcome1 = registry.take_turn_outcome("s-resume");
    assert!(!outcome1.did_reject);
    let captured = outcome1
        .captured_cli_resume_id
        .expect("first turn must capture a resume id");

    // Turn 2: cli_resume_id is in the TurnContext — the agent appends
    // `--resume <id>` to the CLI's argv; the fake echoes the same id
    // back in the `session_id` line.
    let mut turn2 = make_turn("s-resume");
    turn2.cli_resume_id = Some(captured.clone());
    let events2 = drain(
        agent
            .send_message(text_request("hi again"), &turn2)
            .await
            .unwrap(),
    )
    .await;
    assert!(matches!(events2.last(), Some(StreamEvent::Done { .. })));
    // The second turn must NOT reject — the CLI accepted the resume.
    let outcome2 = registry.take_turn_outcome("s-resume");
    assert!(!outcome2.did_reject);
    // The CLI's emitted session_id (captured by the parser) matches
    // the resume id we passed in — the resume was actually forwarded.
    assert_eq!(
        outcome2.captured_cli_resume_id.as_deref(),
        Some(captured.as_str())
    );
}

/// 4. The cancel token is plumbed through the agent's
/// `TurnContext` and reaches the runner's `wait_or_cancel`. When
/// the token is pre-cancelled before the runner enters the wait
/// phase, the runner takes the cancel branch and the agent
/// emits `Done { Cancelled }`. We use the `text-only` script with
/// a long delay so the runner is still in `wait_or_cancel` when
/// the cancel fires.
#[tokio::test]
async fn test_claude_code_wrapped_cancel_mid_stream() {
    let registry = new_registry();
    // 5s delay so the fake_cli process is still running when we
    // cancel. The runner is then forced through `wait_or_cancel`'s
    // cancel branch and reports `Cancelled` to the agent.
    let path = fake_cli_path();
    let mut env = std::collections::BTreeMap::new();
    env.insert("FAKE_CLI_SCRIPT".to_string(), "text-only".to_string());
    env.insert("FAKE_CLI_DELAY_MS".to_string(), "5000".to_string());
    let agent: Arc<ClaudeCodeCodingAgent> = Arc::new(ClaudeCodeCodingAgent::new(
        path,
        vec![],
        env,
        "claude-sonnet-4-20250514".to_string(),
        "acceptEdits".to_string(),
        registry.active_child_processes(),
        registry.turn_outcomes_arc(),
    ));

    let cancel = tokio_util::sync::CancellationToken::new();
    // Pre-cancel BEFORE calling send_message. The agent's spawned
    // task calls runner.run with the token; runner.run races
    // child.wait() against token.cancelled() — the cancel branch
    // wins because of the `biased` selector.
    cancel.cancel();
    let mut turn = make_turn("s-cancel");
    turn.cancellation_token = cancel.clone();
    let stream = agent
        .send_message(text_request("do stuff"), &turn)
        .await
        .unwrap();
    let rest = drain(stream).await;

    // The final event must be Done(Cancelled) OR Error.
    let terminal = rest.last();
    assert!(
        matches!(
            terminal,
            Some(StreamEvent::Done {
                stop_reason: crate::agent::StopReason::Cancelled
            }) | Some(StreamEvent::Error { .. })
        ),
        "expected terminal Cancelled/Error, got: {terminal:?}"
    );
}

/// 5. The CLI's rejection of a `--resume <id>` invocation is detected
/// by `did_reject` and the side-channel carries it through. The
/// `resume-unknown-session` fake script exits non-zero with a stderr
/// substring the rejection detector recognizes.
#[tokio::test]
async fn test_claude_code_wrapped_falls_back_to_replay() {
    let registry = new_registry();
    let agent = build_agent("resume-unknown-session", &registry);

    let mut turn = make_turn("s-reject");
    turn.cli_resume_id = Some("ghost-session".to_string());
    let events = drain(agent.send_message(text_request("hi"), &turn).await.unwrap()).await;
    // The runner exits non-zero → agent emits an Error OR Done(Cancelled).
    assert!(matches!(
        events.last(),
        Some(StreamEvent::Error { .. })
            | Some(StreamEvent::Done {
                stop_reason: crate::agent::StopReason::Cancelled
            })
    ));
    let outcome = registry.take_turn_outcome("s-reject");
    assert!(
        outcome.did_reject,
        "expected did_reject=true on rejected resume, got: {outcome:?}"
    );
}

/// 6. The agent's translator compiles the per-turn `StreamEvent`s
/// into a sequence of `TraceEvent`s. The translator is reached from
/// the `agent_loop` driver in `service::sessions`; here we test it
/// directly to lock in the cross-layer wiring. We use the
/// `text+tool+done` fake script so the translator sees a `ToolUseStart`
/// and emits a `ToolCall` trace.
#[tokio::test]
async fn test_claude_code_wrapped_records_journey() {
    use crate::agent::claude_code::JourneyTranslator;
    use crate::store::traces::TraceEventKind;
    use crate::trace::TraceCollector;
    use tokio::sync::mpsc;

    let registry = new_registry();
    let agent = build_agent("text+tool+done", &registry);

    let session_id = "s-journey";
    let turn = make_turn(session_id);
    let events = drain(agent.send_message(text_request("hi"), &turn).await.unwrap()).await;

    // Drive a real JourneyTranslator over the events. The collector
    // uses an mpsc channel — we capture everything via the receiver.
    let (tx, mut rx) = mpsc::unbounded_channel();
    let collector = TraceCollector::new(tx);
    let mut translator = JourneyTranslator::new(session_id, &collector);
    for ev in &events {
        translator.on_event(ev);
    }
    drop(translator);
    // Close the sender (drop collector) so rx drains.
    drop(collector);

    let mut trace_events = Vec::new();
    while let Some(ev) = rx.recv().await {
        trace_events.push(ev);
    }
    assert!(
        !trace_events.is_empty(),
        "JourneyTranslator must produce ≥1 TraceEvent for {events:?}"
    );
    // Sanity: every event is tagged with our session id.
    for ev in &trace_events {
        assert_eq!(ev.session_id, session_id);
    }
    // The text+tool+done script emits a `tool_use` — the translator
    // should produce a `ToolCall` trace (orphan or completed, depending
    // on whether the result arrived).
    let has_tool_call = trace_events
        .iter()
        .any(|e| matches!(e.kind, TraceEventKind::ToolCall { .. }));
    assert!(
        has_tool_call,
        "expected at least one ToolCall trace for text+tool+done events: {trace_events:?}"
    );
}

/// 7. A native (HTTP) session still works through the registry — the
/// runtime-aware branch added in feat-051 must not regress the
/// non-CLI path. We use a mock `CodingAgent` registered under a
/// non-CLI provider to confirm the registry's get/add/remove
/// discipline survives the new fields.
#[tokio::test]
async fn test_native_anthropic_still_passes_through_loop() {
    use async_trait::async_trait;
    use std::pin::Pin;
    use std::sync::Mutex;

    /// A trivial mock that emits Done(EndTurn). The `provider_type`
    /// "mock-anthropic" mirrors what the `create_provider` HTTP
    /// handler would persist for an Anthropic row.
    struct MockAnthropicAgent {
        emit: Arc<Mutex<bool>>,
    }

    #[async_trait]
    impl CodingAgent for MockAnthropicAgent {
        fn provider_type(&self) -> &str {
            "mock-anthropic"
        }
        fn display_name(&self) -> &str {
            "Mock Anthropic"
        }
        async fn list_models(
            &self,
        ) -> Result<Vec<crate::agent::ModelInfo>, crate::error::ProviderError> {
            Ok(vec![])
        }
        async fn send_message(
            &self,
            _request: MessageRequest,
            _turn: &TurnContext,
        ) -> Result<
            Pin<Box<dyn Stream<Item = Result<StreamEvent, crate::error::ProviderError>> + Send>>,
            crate::error::ProviderError,
        > {
            *self.emit.lock().unwrap() = true;
            let (tx, rx) = tokio::sync::mpsc::channel(4);
            tokio::spawn(async move {
                let _ = tx
                    .send(Ok(StreamEvent::Done {
                        stop_reason: crate::agent::StopReason::EndTurn,
                    }))
                    .await;
            });
            // Local ReceiverStream wrapper.
            struct R<T> {
                rx: tokio::sync::mpsc::Receiver<T>,
            }
            impl<T> Stream for R<T> {
                type Item = T;
                fn poll_next(
                    mut self: Pin<&mut Self>,
                    cx: &mut std::task::Context<'_>,
                ) -> std::task::Poll<Option<Self::Item>> {
                    self.rx.poll_recv(cx)
                }
            }
            Ok(Box::pin(R { rx }))
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

    let registry = new_registry();
    let emit = Arc::new(Mutex::new(false));
    let mock: Arc<dyn CodingAgent> = Arc::new(MockAnthropicAgent { emit: emit.clone() });
    registry.add_agent("p-anthropic", mock);

    // Lookup works.
    let resolved = registry.get_agent("p-anthropic").expect("agent");
    assert_eq!(resolved.provider_type(), "mock-anthropic");

    // send_message still works (cross-checks the registry's storage).
    let turn = make_turn("s-http");
    let events = drain(
        resolved
            .send_message(text_request("hi"), &turn)
            .await
            .unwrap(),
    )
    .await;
    assert!(matches!(
        events.last(),
        Some(StreamEvent::Done {
            stop_reason: crate::agent::StopReason::EndTurn
        })
    ));
    assert!(*emit.lock().unwrap(), "send_message was called");
}

// --- TurnOutcome side-channel sanity check ---------------------------------

/// The side-channel accessor on the registry returns the empty
/// default if no turn has been recorded for the given session id.
#[test]
fn test_take_turn_outcome_empty_default() {
    let registry = new_registry();
    let outcome: TurnOutcome = registry.take_turn_outcome("never-streamed");
    assert!(!outcome.did_reject);
    assert!(outcome.captured_cli_resume_id.is_none());
}
