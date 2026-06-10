//! Test-only fake CLI binary (feat-044).
//!
//! Emulates a Claude-Code-style CLI for conformance tests. The behavior
//! is selected by the `FAKE_CLI_SCRIPT` env var; each script emits a
//! deterministic sequence of events on stdout as newline-delimited
//! JSON — the same wire shape Claude Code's `stream-json` mode
//! produces (and the `ClaudeCodeStreamParser` in feat-045 will consume).
//!
//! Auto-discovered as a `[[bin]]` from the conventional
//! `src/bin/fake_cli.rs` path. Located from tests via
//! `env!("CARGO_BIN_EXE_fake_cli")` (integration tests) or by walking
//! up from `current_exe()` (in-crate unit tests). Zero production
//! callers.
//!
//! ## Wire format
//!
//! Every script emits one JSON object per line on stdout:
//!
//! | Event               | Shape                                                                  |
//! | ------------------- | ---------------------------------------------------------------------- |
//! | `session_id`        | `{"type": "session_id", "id": "<uuid-or-echoed-resume-id>"}`           |
//! | `text_delta`        | `{"type": "text_delta", "text": "<text>"}`                             |
//! | `tool_use`          | `{"type": "tool_use", "id": "<id>", "name": "<name>", "input": {...}}` |
//! | `input_json_delta`  | `{"type": "input_json_delta", "id": "<id>", "delta": "<chunk>"}`       |
//! | `tool_result`       | `{"type": "tool_result", "tool_use_id": "<id>", "content": "<text>"}`  |
//! | `thinking`          | `{"type": "thinking", "text": "<text>"}`                               |
//! | `error`             | `{"type": "error", "code": "<code>", "message": "<text>"}`             |
//! | `done`              | `{"type": "done", "stop_reason": "end_turn"|"max_tokens"|"tool_use"}`  |
//!
//! The `session_id` event is ALWAYS the first line of every script. When
//! `--resume <id>` is passed, the fake echoes that value back as the
//! session id; otherwise a fresh UUID is generated.
//!
//! ## Inputs
//!
//! - `--resume <id>`   — resume id; echoed back as the first
//!   `session_id` event (matches real Claude Code behavior of
//!   always emitting a session id)
//!
//! ## Env vars
//!
//! - `FAKE_CLI_SCRIPT`     — one of:
//!     - `text-only`              (default)
//!     - `text+tool+done`
//!     - `permission-denied`
//!     - `crash`
//!     - `resume-unknown-session`
//!     - `echo-resume-id`
//! - `FAKE_CLI_INPUT_MODE` — `argv` (default) | `stdin`. When `stdin`,
//!   the fake reads (and discards) stdin to exercise the runner's
//!   stdin-write path. The prompt is never echoed in any event
//!   (the canonical event shape is fixed; extra fields would
//!   diverge from real Claude Code).
//! - `FAKE_CLI_DELAY_MS`   — optional sleep before each event
//!   (default 0; useful for cancel-path tests in later phases).
//!
//! ## Exit codes
//!
//! - `0`   — success
//! - `2`   — `permission-denied` script
//! - `3`   — `resume-unknown-session` script
//! - `139` — `crash` script (128 + SIGSEGV)
//! - `1`   — defensive: unknown `FAKE_CLI_SCRIPT`
//!
//! ## Scripts
//!
//! | Script                  | Events (in order, after `session_id`)                            | Exit |
//! | ----------------------- | ---------------------------------------------------------------- | ---- |
//! | `text-only`             | `text_delta` → `done(end_turn)`                                  | 0    |
//! | `text+tool+done`        | `text_delta` → `tool_use` → `done(tool_use)` (no `tool_result`)  | 0    |
//! | `permission-denied`     | `error{code:permission_denied}` (+ stderr diagnostic)            | 2    |
//! | `crash`                 | (no further events)                                              | 139  |
//! | `resume-unknown-session`| `error{code:resume_unknown_session}` (+ stderr diagnostic)       | 3    |
//! | `echo-resume-id`        | `done(end_turn)` (the `session_id` was already emitted)          | 0    |
//!
//! `text+tool+done` does NOT emit a `tool_result`. Real Claude Code
//! never re-emits a tool result it did not execute; the tool is run
//! server-side by the runtime. The journey translator (feat-048)
//! records the missing result as a `tool_call` with
//! `status='orphaned'`.

#![forbid(unsafe_code)]

use std::env;
use std::io::{self, Read, Write};
use std::process::ExitCode;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Argv + env parsing
// ---------------------------------------------------------------------------

/// Hand-rolled argv parser. Recognized flag is `--resume`; unknown
/// flags are silently ignored (real CLIs accept many we don't model
/// — `--output-format`, `--verbose`, etc.).
struct Args {
    resume: Option<String>,
}

fn parse_args() -> Args {
    let mut args = Args { resume: None };
    let mut iter = env::args().skip(1);
    while let Some(flag) = iter.next() {
        if flag == "--resume" {
            args.resume = iter.next();
        }
    }
    args
}

// ---------------------------------------------------------------------------
// Event emission
// ---------------------------------------------------------------------------

/// Emit a single JSON event on stdout, flushed immediately so the
/// runner's `LineStream` can consume it line-by-line. A non-zero
/// `delay` sleeps BEFORE the emit (lets cancel-path tests in later
/// phases exercise the runner's cancel branch with realistic timing).
fn emit(event: &Value, delay: Duration) {
    if !delay.is_zero() {
        sleep(delay);
    }
    let stdout = io::stdout();
    let mut handle = stdout.lock();
    let _ = writeln!(handle, "{event}");
    let _ = handle.flush();
}

// ---------------------------------------------------------------------------
// Scripts
// ---------------------------------------------------------------------------
//
// Every script emits a `session_id` event as its first line (already
// done in `main`, before dispatch). The scripts below add their
// script-specific events on top.

/// `text-only` — emits one `text_delta` then a `done` event.
fn script_text_only(delay: Duration) -> ExitCode {
    emit(
        &json!({"type": "text_delta", "text": "hello from fake_cli"}),
        delay,
    );
    emit(&json!({"type": "done", "stop_reason": "end_turn"}), delay);
    ExitCode::SUCCESS
}

/// `text+tool+done` — emits a `text_delta`, a `tool_use` (start), and
/// a `done` with `stop_reason: "tool_use"`. The fake does NOT emit a
/// `tool_result` — in real Claude Code, the tool is executed
/// server-side by the runtime, not the CLI; the CLI just announces
/// the tool_use and waits for the next user turn.
fn script_text_plus_tool(delay: Duration) -> ExitCode {
    emit(
        &json!({"type": "text_delta", "text": "calling read_file"}),
        delay,
    );
    emit(
        &json!({
            "type": "tool_use",
            "id": "tool_use_1",
            "name": "read_file",
            "input": {"path": "/etc/hostname"}
        }),
        delay,
    );
    emit(&json!({"type": "done", "stop_reason": "tool_use"}), delay);
    ExitCode::SUCCESS
}

/// `permission-denied` — emits an `error` event with code
/// `permission_denied` on stdout, writes a short diagnostic to
/// stderr (so the runner's bounded stderr buffer carries a
/// human-readable signal), and exits with code 2.
fn script_permission_denied(delay: Duration) -> ExitCode {
    emit(
        &json!({
            "type": "error",
            "code": "permission_denied",
            "message": "Permission denied: cannot write to /etc/passwd"
        }),
        delay,
    );
    eprintln!("fake_cli: permission_denied: cannot write to /etc/passwd");
    ExitCode::from(2)
}

/// `crash` — emits nothing after the `session_id` and exits with
/// code 139 (128 + SIGSEGV(11)). Used to verify the runner maps
/// non-zero exits to `ExitError { exit_code }`.
fn script_crash() -> ExitCode {
    ExitCode::from(139)
}

/// `resume-unknown-session` — emits an `error` event with code
/// `resume_unknown_session` and exits with code 3. Feat-047 will
/// catch this in the runner and fall back to message-history
/// replay.
fn script_resume_unknown_session(delay: Duration) -> ExitCode {
    emit(
        &json!({
            "type": "error",
            "code": "resume_unknown_session",
            "message": "No session found for the supplied resume id"
        }),
        delay,
    );
    eprintln!("fake_cli: resume_unknown_session: no session for resume id");
    ExitCode::from(3)
}

/// `echo-resume-id` — only the `session_id` event (already emitted
/// in `main` with the echoed resume id) followed by `done`. Lets
/// tests assert that the runner forwarded the `--resume` value
/// through to the CLI process.
fn script_echo_resume_id(delay: Duration) -> ExitCode {
    emit(&json!({"type": "done", "stop_reason": "end_turn"}), delay);
    ExitCode::SUCCESS
}

// ---------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------

fn main() -> ExitCode {
    let args = parse_args();
    let script = env::var("FAKE_CLI_SCRIPT").unwrap_or_else(|_| "text-only".into());
    let delay_ms: u64 = env::var("FAKE_CLI_DELAY_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let stdin_mode = env::var("FAKE_CLI_INPUT_MODE").as_deref() == Ok("stdin");

    // Exercise the runner's stdin-write path when FAKE_CLI_INPUT_MODE=stdin.
    // The prompt is not echoed in any event (the canonical event shape is
    // fixed; extra fields would diverge from real Claude Code). The runner
    // (feat-043) writes the prompt to stdin then closes the pipe; we read
    // until EOF and discard.
    if stdin_mode {
        let mut buf = String::new();
        let _ = io::stdin().read_to_string(&mut buf);
    }

    let delay = Duration::from_millis(delay_ms);

    // 1. Always emit `session_id` first. Use the --resume value if
    //    provided; otherwise generate a fresh UUID. Matches real
    //    Claude Code, which always emits a session id on the first
    //    event of every turn.
    let session_id = args.resume.unwrap_or_else(|| Uuid::new_v4().to_string());
    emit(&json!({"type": "session_id", "id": session_id}), delay);

    // 2. Dispatch to the requested script.
    match script.as_str() {
        "text-only" => script_text_only(delay),
        "text+tool+done" => script_text_plus_tool(delay),
        "permission-denied" => script_permission_denied(delay),
        "crash" => script_crash(),
        "resume-unknown-session" => script_resume_unknown_session(delay),
        "echo-resume-id" => script_echo_resume_id(delay),
        other => {
            eprintln!("fake_cli: unknown FAKE_CLI_SCRIPT: {other:?}");
            ExitCode::from(1)
        }
    }
}
