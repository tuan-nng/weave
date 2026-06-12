//! `ClaudeCodeCodingAgent` — the first CLI Runtime Tool end-to-end (feat-051).
//!
//! Wires the per-turn CLI runtime together:
//!
//! - **Process** — `CliRunner` (feat-043) for the per-turn subprocess.
//! - **Wire** — Claude Code's `stream-json` line format, parsed by
//!   `ClaudeCodeStreamParser` (feat-045) into the universal
//!   [`StreamEvent`] contract.
//! - **Permissions** — `ClaudeCodePermissionMapper` (feat-046) injects
//!   `--permission-mode <profile>` and the `WEAVE_TOOL_ALLOWLIST`
//!   metadata env var.
//! - **Journey** — `JourneyTranslator` (feat-048) maps `StreamEvent`s
//!   to `TraceEvent`s, with the CLI as the source of truth for tool
//!   results (no re-execution on the Weave side).
//! - **Resume** — the parser captures the CLI's session id
//!   (`session_id` line) and the runner reports `did_reject` from
//!   `detect_resume_rejection` (feat-047). The post-loop
//!   `run_prompt_task` writes the captured id into
//!   `Session::runtime_metadata_json` and decides the
//!   `ResumeState` (`None | Native | Replayed`).
//!
//! The `send_message` async method returns a pinned
//! `Stream<Item = Result<StreamEvent, ProviderError>>` per the
//! [`CodingAgent`] trait — the loop driver in
//! `service::sessions::agent_loop` consumes it like any other
//! agent's stream. The side-channel metadata fields
//! `captured_cli_resume_id` and `did_reject` are recorded into the
//! `ProviderRegistry`'s per-session map keyed by session id; the
//! `run_prompt_task` reads it after the loop returns and clears the
//! entry. That keeps the trait surface narrow (no second return value)
//! while still feeding the feat-047 state machine.
//!
//! ## Stream shape
//!
//! The per-turn stream emits, in order:
//!
//! 1. Zero or more `TextDelta` / `Thinking` / `ToolUseStart` /
//!    `ToolUseDelta` events as the CLI streams them.
//! 2. CLI-emitted `ToolResult` events when applicable (real Claude
//!    Code never re-emits these — it executes the tool — so the
//!    `fake_cli` harness skips them too; the `JourneyTranslator` is
//!    robust to either shape).
//! 3. A single `Done { stop_reason }` at end-of-stream, **or** a
//!    single `Error { message }` on runner failure.
//!
//!
//! In the failure paths (`runner.run` returns `Err`, the child
//! crashed mid-stream, the cancel token fired) the agent surfaces
//! a `Done { Cancelled }` (or `Error`) before dropping the stream.
//! `agent_loop` treats either as terminal.
//!
//! ## Health check
//!
//! `health_check` runs `<binary> --version` (or the configured
//! `args + --version` if `args` is non-empty) with a short cancel
//! timeout. A successful 0-exit + stdout line means the binary is
//! invocable. The actual API surface is validated by the first real
//! turn; the health check only catches "binary missing / not
//! executable" cheaply.
//!
//! ## Per-provider construction
//!
//! `ClaudeCodeCodingAgent::new` is the single chokepoint builder
//! the `api/providers` handler calls. It stores the runner with
//! the SHARED `ActiveChildProcesses` registry so the HTTP cancel
//! handler and the cold-start reaper (feat-049) see the same
//! table the runner writes to.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures_core::Stream;
use std::task::{Context, Poll};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::parser::ClaudeCodeStreamParser;
use super::translator::JourneyTranslator;
use crate::agent::cli_runner::{CliInvocation, CliRunResult, CliRunner};
use crate::agent::permissions::{
    ClaudeCodePermissionMapper, PermissionMapper, PermissionSnapshot, ToolProfile,
};
use crate::agent::registry::TurnOutcome;
use crate::agent::turn_context::TurnContext;
use crate::agent::{
    CodingAgent, MessageRequest, ModelInfo, ProviderError, ProviderHealth, RuntimeKind, StreamEvent,
};
use crate::error::AppError;
use crate::service::ActiveChildProcesses;

/// Per-provider agent instance. Holds the resolved binary, args, env,
/// and the `CliRunner` shared with `AppState::active_child_processes`.
pub struct ClaudeCodeCodingAgent {
    /// Absolute path to the CLI binary (validated upstream — the
    /// `create_cli_provider` API handler rejects relative paths).
    binary_path: PathBuf,
    /// Extra argv flags appended to every invocation (per-provider
    /// configuration). The runner concatenates
    /// `permission_snapshot.argv_flags` AFTER these, so provider-level
    /// args come first in argv order.
    args: Vec<String>,
    /// Env vars merged into every invocation (per-provider
    /// configuration). The runner merges `permission_snapshot.env_vars`
    /// on top.
    env: BTreeMap<String, String>,
    /// Default model for this provider. Recorded for
    /// `list_models`'s wire shape (the v1 CLI stub returns an empty
    /// list — the real Claude Code list is feat-042's
    /// `list_cli_models_via_shell` path; we keep the field so the
    /// eventual list can filter by the provider's default).
    #[allow(dead_code)]
    default_model: String,
    /// Permission mode the operator chose (e.g. `"accept-edits"`,
    /// `"plan"`, `"default"`). Recorded for the future
    /// `PermissionMapper::effective_permissions` to feed the
    /// `--permission-mode` value as an override of the session's
    /// `ToolProfile` (the spec's "CLI default wins" rule).
    /// Today the mapper reads the session's `ToolProfile` from the
    /// `TurnContext`; this field is reserved for the provider-level
    /// override path that lands in feat-053.
    #[allow(dead_code)]
    permission_mode: String,
    /// Per-turn runner. Shares the `ActiveChildProcesses` registry
    /// with `AppState` so the cancel handler / reaper reach the
    /// same pid table the runner writes to.
    runner: CliRunner,
    /// Side-channel cell for per-turn metadata
    /// (`captured_cli_resume_id`, `did_reject`). The loop driver
    /// reads this after the stream is consumed. Keyed by session
    /// id so concurrent turns (different sessions) never collide.
    /// The `TurnOutcome` shape is shared with the registry side
    /// (`agent::registry::TurnOutcome`) so the same `Arc<Mutex<…>>`
    /// is read by both the spawned task and `run_prompt_task`.
    turn_outcomes: Arc<std::sync::Mutex<std::collections::HashMap<String, TurnOutcome>>>,
}

impl ClaudeCodeCodingAgent {
    /// Build a new agent.
    ///
    /// `registry` is the shared `ActiveChildProcesses` table; the
    /// `CliRunner` writes to it on every spawn so the HTTP cancel
    /// handler and the cold-start reaper (feat-049) can reach the
    /// child by `session_id` even when the per-task token is in
    /// flight.
    pub fn new(
        binary_path: PathBuf,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        default_model: String,
        permission_mode: String,
        registry: Arc<ActiveChildProcesses>,
        turn_outcomes: Arc<std::sync::Mutex<std::collections::HashMap<String, TurnOutcome>>>,
    ) -> Self {
        Self {
            binary_path,
            args,
            env,
            default_model,
            permission_mode,
            runner: CliRunner::with_registry(registry),
            turn_outcomes,
        }
    }

    /// Construct the argv for one turn. The order is deterministic:
    ///
    /// 1. `self.args` (per-provider flags, e.g. `--verbose`).
    /// 2. `permission_snapshot.argv_flags` (e.g. `--permission-mode plan`).
    /// 3. `--resume <cli_resume_id>` when the turn is a CLI-resume attempt.
    /// 4. `--model <request.model>`.
    /// 5. `--verbose --output-format stream-json` (force the parser's
    ///    expected wire format on stdout).
    /// 6. `--print` + the prompt as the last positional arg (argv
    ///    mode is the v1 default; real Claude Code's `--input-format
    ///    stream-json` stdin path is a feat-053 follow-up).
    ///
    /// `--print` enables the real `claude` CLI's non-interactive
    /// mode (without it, the binary is interactive by default and
    /// blocks waiting for stdin). The prompt is delivered as a
    /// positional argument per the real CLI's usage line
    /// `claude [options] [command] [prompt]`. `--verbose` plus
    /// `--output-format stream-json` are required for the CLI to
    /// emit `{"type":"text_delta",...}` JSON events on stdout, which
    /// `ClaudeCodeStreamParser` consumes; without them, the CLI
    /// writes plain text the parser can't read and the assistant
    /// turn is dropped on the floor.
    ///
    /// (fix-072: previously this pushed `--prompt <text>`, which
    /// the real CLI rejects with `error: unknown option '--prompt'`,
    /// causing every wrapped session to transition to `error` status
    /// within ~200ms.)
    fn build_args(
        &self,
        request: &MessageRequest,
        turn: &TurnContext,
        permission: &PermissionSnapshot,
    ) -> Vec<String> {
        let mut out = Vec::with_capacity(self.args.len() + permission.argv_flags.len() + 5);
        out.extend(self.args.iter().cloned());
        out.extend(permission.argv_flags.iter().cloned());
        if let Some(ref resume_id) = turn.cli_resume_id {
            out.push("--resume".to_string());
            out.push(resume_id.clone());
        }
        // v1 argv-mode prompt. The CLI reads the user prompt as the
        // last positional; the model + tool definitions are encoded
        // into the prompt body by `service::sessions::build_message_history`
        // before the agent is called, so we pass the model + history
        // concatenated as a single argv string. The `fake_cli` harness
        // does not parse it (its scripts are environment-driven), so
        // the real Claude Code gets the full text and ignores what
        // it doesn't recognize.
        out.push("--model".to_string());
        out.push(request.model.clone());
        // Force stream-json output (the parser's wire format). The real
        // `claude` CLI requires `--verbose` whenever `--output-format
        // stream-json` is set; without it the CLI errors with
        // "When using --print, --output-format=stream-json requires
        // --verbose". `--print` is added below only when a user
        // message is present (so the trailing positional isn't an
        // empty arg), but `--verbose --output-format stream-json` is
        // always set so a tool-result-only turn still produces
        // parseable events.
        out.push("--verbose".to_string());
        out.push("--output-format".to_string());
        out.push("stream-json".to_string());
        if !request.messages.is_empty() {
            // Best-effort: serialize the last user message as the
            // prompt. Real Claude Code uses `--input-format stream-json`
            // and reads from stdin in a structured way — feat-053
            // lands that. For v1 the last user message is the prompt,
            // delivered as the last positional arg under `--print`
            // non-interactive mode.
            if let Some(last_user) = request
                .messages
                .iter()
                .rev()
                .find(|m| matches!(m.role, crate::agent::Role::User))
            {
                if let Some(text) = extract_text(&last_user.content) {
                    out.push("--print".to_string());
                    out.push(text);
                }
            }
        }
        out
    }

    /// Build the env for one turn. Provider-level env first, then
    /// the permission snapshot's env vars (the snapshot's values
    /// override provider-level on key collision — matches the
    /// `CliRunner` merge order).
    fn build_env(&self, permission: &PermissionSnapshot) -> BTreeMap<String, String> {
        let mut out = self.env.clone();
        for (k, v) in &permission.env_vars {
            out.insert(k.clone(), v.clone());
        }
        out
    }
}

#[async_trait]
impl CodingAgent for ClaudeCodeCodingAgent {
    fn provider_type(&self) -> &str {
        "claude-code"
    }

    fn display_name(&self) -> &str {
        "Claude Code"
    }

    async fn list_models(&self) -> Result<Vec<ModelInfo>, ProviderError> {
        // The model list for CLI providers is sourced by
        // `model_cache::list_cli_models_via_shell` (a `--list-models`
        // shell-out), not by the agent. Returning `Ok(vec![])` here
        // is the correct stub — `list_provider_models` checks the
        // kind and short-circuits to the shell-out for `kind="cli"`.
        // This method is only called for `kind="http"` rows.
        let _ = &self.default_model; // suppress the field's `dead_code`
        Ok(Vec::new())
    }

    async fn send_message(
        &self,
        request: MessageRequest,
        turn: &TurnContext,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent, ProviderError>> + Send>>, ProviderError>
    {
        // Build the per-turn invocation.
        let permission = ClaudeCodePermissionMapper::new()
            .effective_permissions(turn.runtime_kind, ToolProfile::Full);
        let args = self.build_args(&request, turn, &permission);
        let env = self.build_env(&permission);

        debug!(
            session_id = %turn.session_id,
            binary = %self.binary_path.display(),
            arg_count = args.len(),
            env_key_count = env.len(),
            "claude_code: spawning per-turn subprocess"
        );

        let invocation = CliInvocation {
            binary: self.binary_path.clone(),
            args,
            env,
            cwd: turn.cwd.clone(),
            stdin_payload: None,
        };

        // Run the subprocess. The runner owns the pid-table registration,
        // the cancel token observation, and the line-stream production.
        // We do NOT observe the cancel token here — the runner does, and
        // it returns `Cancelled` on cancel; the loop driver in
        // `agent_loop` observes the same token separately.
        let run_result = self
            .runner
            .run(invocation, turn)
            .await
            .map_err(|e: AppError| ProviderError::Unreachable(e.to_string()))?;

        // Spawn the consumer task. It owns the parser, the journey
        // translator (if a `TraceCollector` is reachable — for the v1
        // path the collector lives on `ToolContext`, NOT on `TurnContext`,
        // so the journey translator is best-effort: a future
        // `TurnContext::trace_collector` field lands the strict
        // wiring), and the producer channel.
        let (tx, rx) = mpsc::channel::<Result<StreamEvent, ProviderError>>(32);
        let session_id = turn.session_id.clone();
        let turn_outcomes = self.turn_outcomes.clone();

        tokio::spawn(async move {
            let mut parser = ClaudeCodeStreamParser::new();
            // NOTE: the `TraceCollector` is held on `ToolContext` (the
            // loop driver's side), not on `TurnContext` (the agent's
            // side). The agent therefore does NOT have direct access
            // to the collector today — the loop driver itself runs
            // the journey translator on its side, and the agent
            // produces the source `StreamEvent`s. See
            // `service::sessions::agent_loop` for the
            // runtime-aware branch that decides which side runs the
            // tool executor. The translator is wired here for the
            // future feat-053 path that adds `trace_collector` to
            // `TurnContext` directly.
            let mut translator: Option<JourneyTranslator> = None;
            let mut outcome = TurnOutcome::default();

            match run_result {
                CliRunResult::Success {
                    mut stdout,
                    stderr,
                    exit_code,
                } => {
                    debug!(
                        session_id = %session_id,
                        exit_code,
                        "claude_code: subprocess exited 0; consuming line stream"
                    );
                    // Read lines until the stream closes.
                    while let Some(line) = stdout.next().await {
                        match parser.feed_line(&line) {
                            Ok(Some(events)) => {
                                for event in events {
                                    if let Some(ref mut t) = translator {
                                        t.on_event(&event);
                                    }
                                    if tx.send(Ok(event)).await.is_err() {
                                        return; // consumer dropped
                                    }
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                warn!(
                                    session_id = %session_id,
                                    error = %e,
                                    "claude_code: parser error; emitting as Error event"
                                );
                                if tx
                                    .send(Ok(StreamEvent::Error {
                                        message: e.to_string(),
                                    }))
                                    .await
                                    .is_err()
                                {
                                    return;
                                }
                            }
                        }
                    }
                    // Drain every pending deferred `ToolUseStart` event
                    // the CLI never explicitly closed with `done`. Use
                    // `drain_pending` (not the single-event `flush`
                    // shim) so a truncated stream with multiple
                    // in-flight tool_uses emits them all in order
                    // instead of dropping all but the first.
                    for event in parser.drain_pending() {
                        if let Some(ref mut t) = translator {
                            t.on_event(&event);
                        }
                        if tx.send(Ok(event)).await.is_err() {
                            return;
                        }
                    }
                    // The `Done` event is emitted by the parser's
                    // `done` handler; if the CLI never emitted `done`
                    // (e.g. `text+tool+done` does, but a truncated
                    // stream doesn't), the drain above ends without a
                    // Done. The agent surfaces a synthetic EndTurn
                    // Done so the loop driver always sees a terminal
                    // event — the consumer-side `agent_loop` also
                    // tolerates a stream that ends without a `Done`
                    // (falls back to `StopReason::EndTurn`), so this
                    // is defense-in-depth rather than the only line
                    // of defense.
                    if tx
                        .send(Ok(StreamEvent::Done {
                            stop_reason: crate::agent::StopReason::EndTurn,
                        }))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    // Capture resume id + rejection status. The parser
                    // already consumed the `session_id` line — take
                    // it now (None after a prior `take_session_id`,
                    // but the parser is fresh this turn so it's
                    // still set when the CLI emitted it).
                    outcome.captured_cli_resume_id = parser.take_session_id();
                    outcome.did_reject =
                        super::detect_resume_rejection(None, &String::from_utf8_lossy(&stderr));
                }
                CliRunResult::ExitError { exit_code, stderr } => {
                    warn!(
                        session_id = %session_id,
                        exit_code,
                        stderr = %String::from_utf8_lossy(&stderr),
                        "claude_code: subprocess exited non-zero; emitting Error + Cancelled Done"
                    );
                    // Non-zero exit: surface as Error, then Done(Cancelled)
                    // so the loop driver can persist the partial state.
                    if tx
                        .send(Ok(StreamEvent::Error {
                            message: format!(
                                "claude_code: subprocess exited with code {exit_code}: {}",
                                String::from_utf8_lossy(&stderr)
                            ),
                        }))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    if tx
                        .send(Ok(StreamEvent::Done {
                            stop_reason: crate::agent::StopReason::Cancelled,
                        }))
                        .await
                        .is_err()
                    {
                        return;
                    }
                    // Drain any in-flight parser state. The CLI may have
                    // emitted events before exiting non-zero — use
                    // `drain_pending` so multiple in-flight tool_uses
                    // are all surfaced (single-event `flush` would drop
                    // all but the first).
                    for event in parser.drain_pending() {
                        if let Some(ref mut t) = translator {
                            t.on_event(&event);
                        }
                        if tx.send(Ok(event)).await.is_err() {
                            return;
                        }
                    }
                    outcome.captured_cli_resume_id = parser.take_session_id();
                    // Detect resume rejection from the stderr buffer
                    // (the structured `error{code:resume_unknown_session}`
                    // event would have been captured on stdout and
                    // emitted as an Error stream event already — for
                    // `did_reject` we look at the stderr substring
                    // signal as the canonical secondary check).
                    outcome.did_reject =
                        super::detect_resume_rejection(None, &String::from_utf8_lossy(&stderr));
                }
                CliRunResult::Cancelled => {
                    info!(
                        session_id = %session_id,
                        "claude_code: subprocess cancelled; emitting Done(Cancelled)"
                    );
                    if tx
                        .send(Ok(StreamEvent::Done {
                            stop_reason: crate::agent::StopReason::Cancelled,
                        }))
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
            }

            // Finish the translator (drain pending thinking + orphans).
            if let Some(ref mut t) = translator {
                t.finish();
            }

            // Stash the per-turn outcome for the loop driver to read.
            if let Ok(mut map) = turn_outcomes.lock() {
                map.insert(session_id, outcome);
            }
        });

        // Wrap the mpsc receiver in a stream.
        let stream = ReceiverStream::new(rx);
        Ok(Box::pin(stream))
    }

    async fn health_check(&self) -> Result<ProviderHealth, ProviderError> {
        // Probe the binary with `<binary> --version` and a short cancel
        // timeout. The fake_cli harness exits 0 on `--version` (the
        // script machinery ignores unknown flags), so a healthy binary
        // returns a 0 exit and at least one stdout line.
        let mut args = self.args.clone();
        args.push("--version".to_string());
        let invocation = CliInvocation {
            binary: self.binary_path.clone(),
            args,
            env: self.env.clone(),
            cwd: std::path::PathBuf::from("."),
            stdin_payload: None,
        };
        let probe_turn = TurnContext {
            session_id: "health-check".to_string(),
            workspace_id: String::new(),
            cwd: std::path::PathBuf::from("."),
            codebase_root: None,
            cli_resume_id: None,
            runtime_kind: RuntimeKind::ClaudeCode,
            effective_permissions: PermissionSnapshot::empty(
                RuntimeKind::ClaudeCode,
                ToolProfile::Full,
            ),
            cancellation_token: tokio_util::sync::CancellationToken::new(),
        };
        let started = std::time::Instant::now();
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            self.runner.run(invocation, &probe_turn),
        )
        .await
        .map_err(|_| {
            ProviderError::Unreachable("claude_code health check timed out after 5s".into())
        })?;
        let latency_ms = started.elapsed().as_millis() as u64;
        match result {
            Ok(CliRunResult::Success { exit_code, .. }) => {
                if exit_code == 0 {
                    Ok(ProviderHealth {
                        healthy: true,
                        latency_ms,
                        error: None,
                    })
                } else {
                    Ok(ProviderHealth {
                        healthy: false,
                        latency_ms,
                        error: Some(format!("exit code {exit_code}")),
                    })
                }
            }
            Ok(other) => Ok(ProviderHealth {
                healthy: false,
                latency_ms,
                error: Some(format!("non-success result: {other:?}")),
            }),
            Err(e) => Ok(ProviderHealth {
                healthy: false,
                latency_ms,
                error: Some(e.to_string()),
            }),
        }
    }
}

/// Best-effort text extraction from a `Content` enum. Returns the
/// `Text` variant directly, or joins the `Text` blocks of a `Blocks`
/// variant with `\n`. Returns `None` for purely structured content
/// (tool_use / tool_result blocks) — the agent then omits the
/// `--print <prompt>` positional argv and the CLI sees only the
/// structured turn.
fn extract_text(content: &crate::agent::Content) -> Option<String> {
    use crate::agent::Content;
    match content {
        Content::Text(s) => Some(s.clone()),
        Content::Blocks(blocks) => {
            let texts: Vec<String> = blocks
                .iter()
                .filter_map(|b| match b {
                    crate::agent::ContentBlock::Text { text } => Some(text.clone()),
                    _ => None,
                })
                .collect();
            if texts.is_empty() {
                None
            } else {
                Some(texts.join("\n"))
            }
        }
    }
}

// ---------------------------------------------------------------------------
// ReceiverStream wrapper (avoids tokio-stream dependency at the use site)
// ---------------------------------------------------------------------------

/// Wrapper around `tokio::sync::mpsc::Receiver` that implements `Stream`.
struct ReceiverStream<T> {
    rx: mpsc::Receiver<T>,
}

impl<T> ReceiverStream<T> {
    fn new(rx: mpsc::Receiver<T>) -> Self {
        Self { rx }
    }
}

impl<T> Stream for ReceiverStream<T> {
    type Item = T;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.rx.poll_recv(cx)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::turn_context::test_support::make_test_turn_context;
    use std::path::PathBuf;

    /// Re-export so external tests can construct a
    /// `ClaudeCodeCodingAgent` with a pre-populated `turn_outcomes`
    /// map they can read back.
    pub fn build_agent(
        binary_path: PathBuf,
        registry: Arc<ActiveChildProcesses>,
        turn_outcomes: Arc<std::sync::Mutex<std::collections::HashMap<String, TurnOutcome>>>,
    ) -> ClaudeCodeCodingAgent {
        ClaudeCodeCodingAgent::new(
            binary_path,
            Vec::new(),
            BTreeMap::new(),
            "claude-sonnet-4-5".to_string(),
            "default".to_string(),
            registry,
            turn_outcomes,
        )
    }

    /// 1. `provider_type` and `display_name` are stable wire forms
    /// (the frontend's runtime-aware UI affordances depend on them).
    #[test]
    fn test_claude_code_agent_metadata() {
        let registry = Arc::new(ActiveChildProcesses::new());
        let outcomes = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let agent = build_agent(PathBuf::from("/bin/true"), registry, outcomes);
        assert_eq!(agent.provider_type(), "claude-code");
        assert_eq!(agent.display_name(), "Claude Code");
    }

    /// 2. `list_models` returns an empty vec (the CLI path uses
    /// `model_cache::list_cli_models_via_shell` instead). Pinning
    /// the empty stub here so a future refactor that returns data
    /// surfaces the change in tests.
    #[tokio::test]
    async fn test_list_models_returns_empty() {
        let registry = Arc::new(ActiveChildProcesses::new());
        let outcomes = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let agent = build_agent(PathBuf::from("/bin/true"), registry, outcomes);
        let models = agent.list_models().await.expect("list_models");
        assert!(models.is_empty(), "v1 stub returns empty vec");
    }

    /// 3. `build_args` emits `--resume` when the turn carries a
    /// `cli_resume_id`, and never otherwise.
    #[test]
    fn test_build_args_resume_flag() {
        let registry = Arc::new(ActiveChildProcesses::new());
        let outcomes = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let agent = build_agent(PathBuf::from("/bin/true"), registry, outcomes);
        let perm = PermissionSnapshot::empty(RuntimeKind::ClaudeCode, ToolProfile::Full);

        // No resume id: no `--resume` flag.
        let mut turn = make_test_turn_context();
        turn.cli_resume_id = None;
        let req = MessageRequest {
            model: "m".into(),
            messages: vec![crate::agent::Message {
                role: crate::agent::Role::User,
                content: crate::agent::Content::Text("hi".into()),
            }],
            system: None,
            max_tokens: 1024,
            tools: None,
        };
        let args = agent.build_args(&req, &turn, &perm);
        assert!(
            !args.iter().any(|a| a == "--resume"),
            "no --resume when cli_resume_id is None, got: {args:?}"
        );

        // With resume id: `--resume <id>` present.
        turn.cli_resume_id = Some("stale-id".into());
        let args = agent.build_args(&req, &turn, &perm);
        let resume_idx = args
            .iter()
            .position(|a| a == "--resume")
            .expect("--resume present when cli_resume_id is Some");
        assert_eq!(args[resume_idx + 1], "stale-id");
    }

    /// 3a. `build_args` delivers the prompt via `--print` + positional
    /// arg, not via a `--prompt <text>` flag pair. The real
    /// `claude` CLI rejects `--prompt` (exit 1, `error: unknown option
    /// '--prompt'`), which previously caused every wrapped session to
    /// transition to `error` status within ~200ms. Regression guard
    /// for fix-072.
    #[test]
    fn test_build_args_prompt_uses_print_and_positional() {
        let registry = Arc::new(ActiveChildProcesses::new());
        let outcomes = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let agent = build_agent(PathBuf::from("/bin/true"), registry, outcomes);
        let perm = PermissionSnapshot::empty(RuntimeKind::ClaudeCode, ToolProfile::Full);
        let mut turn = make_test_turn_context();
        turn.cli_resume_id = None;
        let req = MessageRequest {
            model: "claude-sonnet-4-20250514".into(),
            messages: vec![crate::agent::Message {
                role: crate::agent::Role::User,
                content: crate::agent::Content::Text("hello".into()),
            }],
            system: None,
            max_tokens: 1024,
            tools: None,
        };

        let args = agent.build_args(&req, &turn, &perm);

        // The real `claude` CLI does not accept `--prompt`. The
        // previous argv-shape used `--prompt <text>`, which the binary
        // rejected with exit 1, leaving the session in `error`.
        assert!(
            !args.iter().any(|a| a == "--prompt"),
            "build_args must not emit --prompt (real claude CLI rejects it), got: {args:?}"
        );

        // Force the parser's expected wire format. `--verbose` is
        // REQUIRED by the real CLI whenever `--output-format
        // stream-json` is set; without `--verbose` the CLI errors
        // with "When using --print, --output-format=stream-json
        // requires --verbose". Without `--output-format stream-json`,
        // the CLI writes plain text that the parser can't read and
        // the assistant turn is silently dropped.
        assert!(
            args.iter().any(|a| a == "--verbose"),
            "build_args must emit --verbose (required by --output-format stream-json), got: {args:?}"
        );
        let of_idx = args
            .iter()
            .position(|a| a == "--output-format")
            .expect("--output-format must be present");
        assert_eq!(
            args.get(of_idx + 1).map(String::as_str),
            Some("stream-json"),
            "output format must be stream-json (the parser's wire format), got: {args:?}"
        );

        // Non-interactive mode: `--print` enables `claude`'s
        // print-and-exit behavior (otherwise the binary is interactive
        // by default and would block waiting for stdin).
        assert!(
            args.iter().any(|a| a == "--print"),
            "build_args must emit --print to enable non-interactive mode, got: {args:?}"
        );

        // The prompt is the last positional arg (no flag prefix). The
        // real `claude` CLI's usage is `claude [options] [command]
        // [prompt]`; the prompt is positional.
        let last = args
            .last()
            .expect("at least one positional arg (the prompt)");
        assert_eq!(
            last, "hello",
            "the last positional arg should be the user prompt, got: {last:?} in {args:?}"
        );
    }

    /// 3b. When the request has no user message, `build_args` does
    /// NOT emit `--print` or a positional prompt — but it DOES
    /// still emit `--verbose --output-format stream-json` so any
    /// tool-result events the CLI emits are parseable. (E.g. a
    /// tool-result-only turn.)
    #[test]
    fn test_build_args_omits_positional_prompt_when_no_user_message() {
        let registry = Arc::new(ActiveChildProcesses::new());
        let outcomes = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let agent = build_agent(PathBuf::from("/bin/true"), registry, outcomes);
        let perm = PermissionSnapshot::empty(RuntimeKind::ClaudeCode, ToolProfile::Full);
        let turn = make_test_turn_context();
        let req = MessageRequest {
            model: "m".into(),
            messages: vec![],
            system: None,
            max_tokens: 1024,
            tools: None,
        };

        let args = agent.build_args(&req, &turn, &perm);

        assert!(
            !args.iter().any(|a| a == "--prompt"),
            "no --prompt (regression guard) when no user message, got: {args:?}"
        );
        // No `--print` either, since the trigger to add the prompt
        // is the existence of a user message. (`--print` is
        // `--print <positional>`; without a positional it's a
        // no-op flag and would still trigger non-interactive mode,
        // which we don't want for tool-result-only turns.)
        assert!(
            !args.iter().any(|a| a == "--print"),
            "no --print when no user message, got: {args:?}"
        );
    }

    /// 4. `build_env` merges provider env with permission env (permission wins).
    #[test]
    fn test_build_env_merges_permission() {
        let registry = Arc::new(ActiveChildProcesses::new());
        let outcomes = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let mut agent = build_agent(PathBuf::from("/bin/true"), registry, outcomes);
        agent.env = BTreeMap::from([
            ("LOG_LEVEL".to_string(), "info".to_string()),
            (
                "WEAVE_TOOL_ALLOWLIST".to_string(),
                "provider-wins".to_string(),
            ),
        ]);
        let perm = PermissionSnapshot {
            runtime_kind: RuntimeKind::ClaudeCode,
            tool_profile: ToolProfile::Full,
            argv_flags: Vec::new(),
            env_vars: BTreeMap::from([(
                "WEAVE_TOOL_ALLOWLIST".to_string(),
                "permission-wins".to_string(),
            )]),
        };
        let env = agent.build_env(&perm);
        assert_eq!(env.get("LOG_LEVEL").map(String::as_str), Some("info"));
        assert_eq!(
            env.get("WEAVE_TOOL_ALLOWLIST").map(String::as_str),
            Some("permission-wins"),
            "permission env overrides provider env on key collision"
        );
    }
}
