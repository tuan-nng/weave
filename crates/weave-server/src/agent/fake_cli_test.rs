//! Integration tests for the `fake_cli` test harness (feat-044).
//!
//! Each test spawns the `fake_cli` binary (auto-discovered as a
//! `[[bin]]` from `src/bin/fake_cli.rs`) via `CliRunner::run`,
//! configures it with a `FAKE_CLI_SCRIPT` env var, and asserts on
//! the resulting `CliRunResult` and stdout/stderr payloads. The
//! runner from feat-043 is exercised end-to-end: spawn, line
//! stream, bounded stderr, non-zero exit mapping.
//!
//! Wire-format reference: `crates/weave-server/src/bin/fake_cli.rs`
//! (the binary's module-level doc).

#![cfg(test)]

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::Value;

use super::cli_runner::{
    test_support::run_with_timeout, CliInvocation, CliRunResult, CliRunner, LineStream,
};
use super::turn_context::test_support::make_test_turn_context;
use crate::error::AppError;

/// Path to the built `fake_cli` binary. `CARGO_BIN_EXE_fake_cli` is
/// set for integration tests in `tests/`, not for in-crate tests, so
/// we fall back to walking up from `current_exe()`.
fn fake_cli_path() -> PathBuf {
    if let Ok(p) = std::env::var("CARGO_BIN_EXE_fake_cli") {
        return PathBuf::from(p);
    }
    // exe = target/debug/deps/<test-name>-<hash>; go up to target/debug/.
    let exe = std::env::current_exe().expect("current_exe is set");
    exe.parent()
        .and_then(|p| p.parent())
        .expect("target/debug/ is the binary output dir")
        .join("fake_cli")
}

/// Build a `CliInvocation` that runs `fake_cli` with the given
/// `FAKE_CLI_SCRIPT` and extra args. The fake is run in the
/// workspace's `target/` tree, so `cwd = "."` is fine.
fn fake_invocation(script: &str, extra_args: Vec<String>) -> CliInvocation {
    let env = BTreeMap::from([("FAKE_CLI_SCRIPT".into(), script.into())]);
    CliInvocation {
        binary: fake_cli_path(),
        args: extra_args,
        env,
        cwd: PathBuf::from("."),
        stdin_payload: None,
    }
}

/// Drive the runner with a default turn context. The fake's
/// scripts all return in milliseconds; anything slower is a
/// regression in the runner, the fake, or both.
async fn run(runner: &CliRunner, inv: CliInvocation) -> Result<CliRunResult, AppError> {
    run_with_timeout(runner, inv, make_test_turn_context()).await
}

/// Unwrap a `CliRunResult::Success` or panic with the actual variant.
/// The `Success { stdout, stderr, exit_code }` is the only variant
/// the success-path tests expect to see; any other variant (e.g.
/// the runner incorrectly mapping a 0 exit to `ExitError`) is a
/// regression worth a self-explaining panic.
fn expect_success(r: CliRunResult) -> (LineStream, Vec<u8>, i32) {
    match r {
        CliRunResult::Success {
            stdout,
            stderr,
            exit_code,
        } => (stdout, stderr, exit_code),
        other => panic!("expected Success, got: {other:?}"),
    }
}

/// Unwrap a `CliRunResult::ExitError` or panic with the actual
/// variant. Symmetric with [`expect_success`].
fn expect_exit_error(r: CliRunResult) -> (i32, Vec<u8>) {
    match r {
        CliRunResult::ExitError { exit_code, stderr } => (exit_code, stderr),
        other => panic!("expected ExitError, got: {other:?}"),
    }
}

/// Collect all stdout lines from a `Success` result into a `Vec`.
/// Returns the lines as parsed `serde_json::Value`s so the test
/// assertions can drill into the event shape.
async fn collect_lines(stdout: &mut LineStream) -> Vec<Value> {
    let mut out = Vec::new();
    while let Some(line) = stdout.next().await {
        let v: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("stdout line is not JSON: {line:?}: {e}"));
        out.push(v);
    }
    out
}

// ---------------------------------------------------------------------------
// 1. `test_fake_cli_emits_text_delta` — `text-only` script emits a
//    `text_delta` then a `done`. The runner delivers both as
//    `Success` with exit code 0.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_fake_cli_emits_text_delta() {
    let runner = CliRunner::new();
    let inv = fake_invocation("text-only", vec![]);

    let result = run(&runner, inv).await.expect("run must succeed");
    let (mut stdout, stderr, exit_code) = expect_success(result);

    assert_eq!(exit_code, 0);
    assert!(stderr.is_empty(), "stderr should be empty: {stderr:?}");
    let events = collect_lines(&mut stdout).await;
    assert_eq!(events.len(), 3, "session_id + text_delta + done");

    assert_eq!(events[0]["type"], "session_id");
    assert!(
        events[0]["id"].is_string(),
        "session_id must have a string id"
    );

    assert_eq!(events[1]["type"], "text_delta");
    assert_eq!(events[1]["text"], "hello from fake_cli");

    assert_eq!(events[2]["type"], "done");
    assert_eq!(events[2]["stop_reason"], "end_turn");
}

// ---------------------------------------------------------------------------
// 2. `test_fake_cli_emits_tool_use_and_result` — `text+tool+done`
//    emits `text_delta` then `tool_use` then `done(tool_use)`. No
//    `tool_result` (matches real Claude Code; the runtime executes
//    tools, not the CLI).
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_fake_cli_emits_tool_use_and_result() {
    let runner = CliRunner::new();
    let inv = fake_invocation("text+tool+done", vec![]);

    let result = run(&runner, inv).await.expect("run must succeed");
    let (mut stdout, _stderr, exit_code) = expect_success(result);

    assert_eq!(exit_code, 0);
    let events = collect_lines(&mut stdout).await;
    assert_eq!(events.len(), 4, "session_id + text_delta + tool_use + done");

    assert_eq!(events[0]["type"], "session_id");
    assert_eq!(events[1]["type"], "text_delta");
    assert_eq!(events[2]["type"], "tool_use");
    assert_eq!(events[2]["id"], "tool_use_1");
    assert_eq!(events[2]["name"], "read_file");
    assert_eq!(events[2]["input"]["path"], "/etc/hostname");

    assert_eq!(events[3]["type"], "done");
    assert_eq!(events[3]["stop_reason"], "tool_use");

    // Load-bearing: real Claude Code never re-emits a tool_result
    // (the runtime executes tools). If the fake starts emitting one,
    // this assertion fires with a self-explaining message.
    assert!(
        !events.iter().any(|e| e["type"] == "tool_result"),
        "fake must not emit tool_result; runtime executes tools: events={events:?}"
    );
}

// ---------------------------------------------------------------------------
// 3. `test_fake_cli_permission_denied_scenario` — emits an
//    `error{code:permission_denied}` event on stdout, writes a
//    diagnostic to stderr, exits 2. The runner maps this to
//    `ExitError { exit_code: 2, stderr }`.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_fake_cli_permission_denied_scenario() {
    let runner = CliRunner::new();
    let inv = fake_invocation("permission-denied", vec![]);

    let result = run(&runner, inv).await.expect("run must succeed");
    let (exit_code, stderr) = expect_exit_error(result);

    assert_eq!(exit_code, 2, "permission-denied exits 2");
    let stderr_s = String::from_utf8_lossy(&stderr);
    assert!(
        stderr_s.contains("permission_denied"),
        "stderr should mention the code, got: {stderr_s}"
    );
}

// ---------------------------------------------------------------------------
// 4. `test_fake_cli_crash_scenario` — `crash` script exits 139
//    (128 + SIGSEGV) without writing further events. The runner
//    maps this to `ExitError { exit_code: 139 }`.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_fake_cli_crash_scenario() {
    let runner = CliRunner::new();
    let inv = fake_invocation("crash", vec![]);

    let result = run(&runner, inv).await.expect("run must succeed");
    let (exit_code, stderr) = expect_exit_error(result);

    assert_eq!(exit_code, 139, "crash exits 139 (128 + SIGSEGV)");
    assert!(
        stderr.is_empty(),
        "crash script writes nothing to stderr, got: {stderr:?}"
    );
}

// ---------------------------------------------------------------------------
// 5. `test_fake_cli_resume_unknown_session` — emits
//    `error{code:resume_unknown_session}` and exits 3. Feat-047
//    will catch this and fall back to message-history replay.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_fake_cli_resume_unknown_session() {
    let runner = CliRunner::new();
    let inv = fake_invocation(
        "resume-unknown-session",
        vec!["--resume".into(), "stale-id".into()],
    );

    let result = run(&runner, inv).await.expect("run must succeed");
    let (exit_code, stderr) = expect_exit_error(result);

    assert_eq!(exit_code, 3, "resume-unknown-session exits 3");
    let stderr_s = String::from_utf8_lossy(&stderr);
    assert!(
        stderr_s.contains("resume_unknown_session"),
        "stderr should mention the code, got: {stderr_s}"
    );
}

// ---------------------------------------------------------------------------
// 6. `test_fake_cli_echoes_resume_id` — with `--resume abc-123`,
//    the fake's first event is `session_id{ id: "abc-123" }`. The
//    test asserts the runner forwards the `--resume` value to the
//    child, and the parser (in feat-045) would capture it from the
//    first stdout line.
// ---------------------------------------------------------------------------
#[tokio::test]
async fn test_fake_cli_echoes_resume_id() {
    let runner = CliRunner::new();
    let inv = fake_invocation("echo-resume-id", vec!["--resume".into(), "abc-123".into()]);

    let result = run(&runner, inv).await.expect("run must succeed");
    let (mut stdout, _stderr, exit_code) = expect_success(result);

    assert_eq!(exit_code, 0);
    let first = stdout.next().await.expect("at least the session_id");
    let v: Value = serde_json::from_str(&first)
        .unwrap_or_else(|e| panic!("session_id line is not JSON: {first:?}: {e}"));
    assert_eq!(v["type"], "session_id");
    assert_eq!(v["id"], "abc-123", "the echoed resume id must round-trip");
}
