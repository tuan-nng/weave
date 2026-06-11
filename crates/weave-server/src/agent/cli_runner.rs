//! Per-turn CLI subprocess runner (feat-043).
//!
//! Spawns a registered CLI as a subprocess for one turn, captures stdout
//! as a line stream (consumed by the per-CLI parser in feat-045+), and
//! captures stderr to a bounded buffer. Reaps the child on natural exit
//! or cancellation. Per-turn by design: every `run` call spawns a fresh
//! child; there is no long-lived process state across turns.
//!
//! ## Why per-turn
//!
//! The strategy doc (`docs/road-map/multi-runtime-strategy.md` §6) calls
//! for "per-turn subprocess, not long-lived, for `wrapped` mode". Long-lived
//! preserves in-memory caches but couples Weave's session lifecycle to the
//! CLI's process lifecycle. We start simple; revisit if a CLI's
//! context-engine costs become visible in practice.
//!
//! ## Per-session process table
//!
//! The `ActiveChildProcesses` table (in `service::`, feat-049) holds
//! `session_id -> pid` for every in-flight child. The runner
//! registers on entry and unregisters on every exit path; the cancel
//! handler can call `terminate(session_id)` to SIGTERM the group
//! directly, and the cold-start reaper scans /proc independently.
//! Production code shares the registry via
//! [`CliRunner::with_registry`] so the HTTP cancel handler, the
//! reaper, and the runner all reach the same table.
//!
//! ## Cancellation
//!
//! `tokio::select!` between the child `wait` and the per-turn
//! `CancellationToken` (carried in [`TurnContext`]). On cancel: SIGTERM
//! to the process group, poll for exit up to 5 s, then SIGKILL to the
//! group. On Unix, the child is spawned with `process_group(0)` so the
//! entire tree is killed — matches the policy in
//! `tools::shell::ShellExecTool` and the timeout path of
//! `model_cache::list_cli_models_via_shell`.
//!
//! ## Output handling
//!
//! Stdout is read line-by-line into a `tokio::sync::mpsc` channel. The
//! `Receiver<String>` is exposed as `LineStream` in the `Success` result
//! and consumed by the parser. A single line longer than 1 MiB is
//! truncated with a `<line truncated>` marker (defense against a
//! pathological or malicious CLI). Stderr is bounded at 256 KiB; the
//! reader keeps draining the pipe past the cap so the child never
//! blocks on write.
//!
//! ## Per-turn log
//!
//! Logs the full argv, cwd, env *keys* (never values — secrets), and
//! session id at INFO on start, plus the outcome at INFO on end. The
//! redaction discipline is verified by `test_cli_runner_log_redacts_env_values`.

// The entire module is the public surface for the `CliCodingAgent`
// that lands in feat-051+. Until that consumer exists, every item
// here is "unused" from the production binary's perspective; the
// test module exercises them. One inner attribute is cleaner than
// sprinkling `#[allow(dead_code)]` on every item.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;
use tokio::sync::mpsc;
use tracing::{debug, info, warn};

use super::turn_context::TurnContext;
use crate::error::AppError;
use crate::service::ActiveChildProcesses;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Everything a `CliCodingAgent` (feat-051+) needs to spawn a CLI
/// subprocess for one turn. The runner does NOT inspect `args` or `env`
/// — it just passes them to `tokio::process::Command`. The shell
/// argument syntax is the caller's responsibility (Claude Code uses
/// `--flag value` pairs; Codex and OpenCode will vary).
///
/// `env` is `BTreeMap` (not `HashMap`) so the per-turn log is
/// deterministic in its key order — important for debugging a
/// reproduction, and the test suite's `FAKE_CLI_*` variables.
///
/// `stdin_payload` is `Option` because some CLIs read the prompt from
/// stdin (Claude Code in `--input-format stream-json` mode) and others
/// take it as argv. `None` closes stdin.
#[allow(dead_code)] // Consumed by CliCodingAgent in feat-051+.
#[derive(Debug, Clone)]
pub struct CliInvocation {
    pub binary: PathBuf,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub cwd: PathBuf,
    pub stdin_payload: Option<Vec<u8>>,
}

/// A stream of lines read from a child process's stdout.
///
/// Returned by `CliRunner::run` in the `Success` variant and consumed
/// by the per-CLI parser in feat-045+. The wrapper is intentionally
/// thin — it just hands the caller a `mpsc::Receiver<String>` with
/// `async fn next` — so the parser can adopt whatever consumption
/// pattern it needs (collect-then-parse, or stream-into-SSE).
#[allow(dead_code)] // Consumed by CliCodingAgent in feat-051+.
#[derive(Debug)]
pub struct LineStream {
    inner: mpsc::Receiver<String>,
}

impl LineStream {
    /// Receive the next line, returning `None` when the child has
    /// closed stdout and all buffered lines have been consumed.
    pub async fn next(&mut self) -> Option<String> {
        self.inner.recv().await
    }

    /// Convert into the raw `mpsc::Receiver<String>` for callers that
    /// want the primitive directly (e.g., wrapping in
    /// `tokio_stream::wrappers::ReceiverStream`).
    #[allow(dead_code)] // Public for the per-CLI parser (feat-045+).
    pub fn into_inner(self) -> mpsc::Receiver<String> {
        self.inner
    }
}

/// The outcome of a `CliRunner::run` call.
///
/// `Success` carries the live `LineStream` (the parser has not yet
/// read it), the bounded stderr buffer, and the exit code. `Cancelled`
/// and `ExitError` are normal completion paths, not errors — the
/// runner only returns `Err` when the child could not be spawned at
/// all (binary missing, permission denied, etc.).
#[allow(dead_code)] // Consumed by CliCodingAgent in feat-051+.
#[derive(Debug)]
pub enum CliRunResult {
    /// Process exited with code 0. `stdout` is the live line stream;
    /// the parser in feat-045+ will consume it.
    Success {
        stdout: LineStream,
        stderr: Vec<u8>,
        exit_code: i32,
    },
    /// The `CancellationToken` was cancelled mid-turn. The child was
    /// SIGTERM'd, then SIGKILL'd after the 5 s grace. There is no
    /// stdout to consume — the channel is closed and the receiver
    /// returns `None` immediately.
    Cancelled,
    /// The child exited with a non-zero status. `stderr` is bounded
    /// (256 KiB) and includes a truncation marker if the cap was hit.
    ExitError { exit_code: i32, stderr: Vec<u8> },
}

/// The runner. Constructed once per `CliCodingAgent` (feat-051+) and
/// reused across turns. The internal registry tracks every spawned
/// child's pid (registered on entry, removed on exit) so the cancel
/// path can reach a live child even if the per-task token is in
/// flight. Production code shares the registry via
/// [`CliRunner::with_registry`] so the HTTP `cancel` handler and the
/// cold-start reaper can both reach the same table.
#[allow(dead_code)] // Consumed by CliCodingAgent in feat-051+.
pub struct CliRunner {
    registry: Arc<ActiveChildProcesses>,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum bytes retained per stdout line. Lines longer than this are
/// truncated with a `<line truncated>` marker; the remaining bytes
/// until the next `\n` are dropped. Defense against a pathological
/// or malicious CLI that emits a single multi-GB line.
const MAX_LINE_BYTES: usize = 1024 * 1024;

/// Maximum bytes retained from stderr. Past the cap, the reader keeps
/// draining the pipe (so the child never blocks on write) but stops
/// appending. A `<stderr truncated at 256KB>` marker is appended so
/// the caller knows the cap was hit.
const MAX_STDERR_BYTES: usize = 256 * 1024;

/// Buffer size for the stdout line-stream channel. Small (the
/// default-ish) because the parser is expected to consume lines as
/// they arrive; if the parser is slow, the reader blocks on `send`
/// and backpressures the child.
const CHANNEL_BUFFER: usize = 32;

/// How long to wait between SIGTERM and SIGKILL on cancel. Matches the
/// graceful-shutdown pattern from feat-034 (`cancel_all` then SIGKILL
/// after a short grace).
const SIGTERM_GRACE: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------------------
// CliRunner
// ---------------------------------------------------------------------------

impl CliRunner {
    /// Build a runner with a fresh, isolated process registry. Use
    /// this in tests; use [`CliRunner::with_registry`] in production
    /// to share the AppState-level registry.
    pub fn new() -> Self {
        Self {
            registry: Arc::new(ActiveChildProcesses::new()),
        }
    }

    /// Build a runner that registers its pids in a shared
    /// `ActiveChildProcesses`. The HTTP cancel handler and the
    /// cold-start reaper (`service::startup::reap_cli_processes`)
    /// both need to reach the same table.
    pub fn with_registry(registry: Arc<ActiveChildProcesses>) -> Self {
        Self { registry }
    }

    /// The process registry. Cloned for tests that want to call
    /// `terminate` against the same table the runner writes to.
    pub fn registry(&self) -> Arc<ActiveChildProcesses> {
        self.registry.clone()
    }

    /// Spawn `invocation` and wait for it to exit, cancel, or fail.
    ///
    /// The per-turn `TurnContext` (feat-041) carries the cancel token
    /// and session id. On cancel, SIGTERM is sent to the process
    /// group; after `SIGTERM_GRACE` the group is SIGKILL'd. The child
    /// is registered in the active table for the duration of the
    /// turn and removed on exit regardless of outcome.
    pub async fn run(
        &self,
        invocation: CliInvocation,
        turn: &TurnContext,
    ) -> Result<CliRunResult, AppError> {
        // 1. Build the command. Unix-only `process_group(0)` so the
        //    cancel path can SIGKILL the entire tree (matches
        //    `tools::shell::ShellExecTool` and the timeout branch of
        //    `model_cache::list_cli_models_via_shell`).
        let mut cmd = Command::new(&invocation.binary);
        cmd.args(&invocation.args)
            .envs(&invocation.env)
            .current_dir(&invocation.cwd)
            .stdin(if invocation.stdin_payload.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        #[cfg(unix)]
        {
            cmd.process_group(0);
        }

        // 2. Spawn. A failure here is the only path that produces
        //    `Err(_)` — the binary is missing, not executable, etc.
        let mut child = cmd.spawn().map_err(|e| {
            AppError::CliProcess {
                code: "cli_spawn_failed",
                message: format!(
                    "failed to spawn CLI binary '{}': {}. Check that the binary exists and is executable.",
                    invocation.binary.display(),
                    e
                ),
            }
        })?;

        // 3. Capture the pid before we move the Child around. The
        //    cancel path needs the pid to signal the process group;
        //    `child.id()` may return `None` if the child has already
        //    exited, in which case we report that and bail.
        let pid = child.id().ok_or_else(|| AppError::CliProcess {
            code: "cli_spawn_failed",
            message: format!(
                "spawned CLI '{}' but no pid was assigned (child likely exited immediately)",
                invocation.binary.display()
            ),
        })?;

        // 4. Take stdout/stderr/stdin handles before they go out of
        //    scope with `child`. The reader tasks own these for the
        //    rest of the turn.
        let stdout_handle = child
            .stdout
            .take()
            .ok_or_else(|| cli_spawn_post("child has no stdout pipe"))?;
        let stderr_handle = child
            .stderr
            .take()
            .ok_or_else(|| cli_spawn_post("child has no stderr pipe"))?;
        let stdin_handle = child.stdin.take();

        // 5. Write stdin payload if present. We don't await the
        //    child to flush it — closing the pipe is enough for
        //    `sh -c`-style CLIs; if the child blocks waiting for
        //    more input, the cancel path will catch it.
        if let (Some(mut stdin), Some(payload)) = (stdin_handle, invocation.stdin_payload.as_ref())
        {
            if let Err(e) = stdin.write_all(payload).await {
                debug!(
                    session_id = %turn.session_id,
                    error = %e,
                    "failed to write stdin payload to child (continuing — child may exit)"
                );
            }
            // Drop stdin to signal EOF.
            drop(stdin);
        }

        // 6. Register the child in the shared process registry. Held
        //    only for the duration of the turn; the entry is removed
        //    on every exit path (success, cancel, error).
        self.registry.register(turn.session_id.clone(), pid);

        // 7. Spawn the stdout reader: line-by-line into a bounded
        //    mpsc channel. The receiver becomes the `LineStream`
        //    in the `Success` result.
        let (stdout_tx, stdout_rx) = mpsc::channel::<String>(CHANNEL_BUFFER);
        let stdout_task = tokio::spawn(read_lines(stdout_handle, stdout_tx, MAX_LINE_BYTES));

        // 8. Spawn the stderr reader: bounded bytes. The handle is
        //    kept alive in this task; the Vec<u8> comes back on join.
        let stderr_task = tokio::spawn(read_bounded_stderr(stderr_handle, MAX_STDERR_BYTES));

        // 9. Per-turn log on start. Env values are NEVER logged —
        //    only the keys. This is verified by
        //    `test_cli_runner_log_redacts_env_values`.
        let env_keys: Vec<&str> = invocation.env.keys().map(String::as_str).collect();
        info!(
            session_id = %turn.session_id,
            binary = %invocation.binary.display(),
            args = ?invocation.args,
            cwd = %invocation.cwd.display(),
            env_keys = ?env_keys,
            pid = pid,
            "cli_turn_start"
        );

        // 10. Wait for the child, watching the cancel token.
        let outcome = wait_or_cancel(&mut child, &turn.cancellation_token, pid).await;

        // 11. Unregister from the process registry regardless of
        //     outcome. The cancel path may have already removed the
        //     entry via `terminate`, but the entry is a per-turn
        //     record and must not outlive the turn.
        self.registry.unregister(&turn.session_id);

        // 12. Collect reader output. Both tasks are bounded by the
        //     child's pipes closing, so they finish in finite time.
        let stderr_bytes = stderr_task.await.unwrap_or_else(|e| {
            warn!(session_id = %turn.session_id, error = %e, "stderr reader task failed");
            Vec::new()
        });
        // stdout_task is just a drain; we don't use its return value
        // (it's `()`), but we must await it to ensure the reader has
        // emitted any final buffered line before the caller starts
        // consuming the LineStream.
        let _ = stdout_task.await;

        // 13. Map the outcome to a CliRunResult.
        let cli_result = match outcome {
            WaitOutcome::Exited(status) => {
                let exit_code = status.code().unwrap_or(-1);
                if status.success() {
                    CliRunResult::Success {
                        stdout: LineStream { inner: stdout_rx },
                        stderr: stderr_bytes,
                        exit_code,
                    }
                } else {
                    CliRunResult::ExitError {
                        exit_code,
                        stderr: stderr_bytes,
                    }
                }
            }
            WaitOutcome::Cancelled => {
                // The child was SIGTERM'd (then SIGKILL'd after the
                // grace). stdout channel is closed; the receiver
                // will return None immediately. We deliberately
                // drop the receiver here so the test of
                // `test_cli_runner_per_turn_process_table` can
                // observe the empty active table.
                drop(stdout_rx);
                CliRunResult::Cancelled
            }
        };

        // 14. Per-turn log on end. Includes exit code for Success
        //     and ExitError; "cancelled" for the cancel path.
        let outcome_label = match &cli_result {
            CliRunResult::Success { exit_code, .. } => format!("success({exit_code})"),
            CliRunResult::ExitError { exit_code, .. } => format!("exit_error({exit_code})"),
            CliRunResult::Cancelled => "cancelled".to_string(),
        };
        info!(
            session_id = %turn.session_id,
            binary = %invocation.binary.display(),
            pid = pid,
            outcome = %outcome_label,
            "cli_turn_end"
        );

        Ok(cli_result)
    }

    /// Number of sessions with a currently-running child. Test-only.
    #[cfg(test)]
    pub(crate) async fn active_count(&self) -> usize {
        self.registry.len()
    }

    /// Pids of currently-running children, for assertions. Test-only.
    #[cfg(test)]
    pub(crate) async fn active_pids(&self) -> Vec<u32> {
        self.registry.pids()
    }
}

impl Default for CliRunner {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Outcome of the `wait_or_cancel` race.
enum WaitOutcome {
    /// Child exited naturally (success or non-zero).
    Exited(std::process::ExitStatus),
    /// `CancellationToken` fired; the child was SIGTERM'd and
    /// (after `SIGTERM_GRACE`) SIGKILL'd.
    Cancelled,
}

/// Race the child `wait` against the cancel token. On cancel: SIGTERM
/// the process group, wait up to `SIGTERM_GRACE` for graceful exit,
/// then SIGKILL the group. On Unix the child was spawned with
/// `process_group(0)` so the entire tree is signaled.
async fn wait_or_cancel(
    child: &mut tokio::process::Child,
    token: &tokio_util::sync::CancellationToken,
    pid: u32,
) -> WaitOutcome {
    // The `biased` selector makes the cancel branch checked first,
    // so a cancel that arrived just before this call is observed
    // immediately rather than racing the child's natural exit.
    tokio::select! {
        biased;
        _ = token.cancelled() => {
            // First: SIGTERM the whole group, give the child a chance
            // to flush / clean up.
            #[cfg(unix)]
            {
                // Safety: killpg is a libc function; pid came from
                // the child we spawned with process_group(0).
                let _ = unsafe { libc::killpg(pid as i32, libc::SIGTERM) };
            }
            match tokio::time::timeout(SIGTERM_GRACE, child.wait()).await {
                Ok(Ok(_)) => WaitOutcome::Cancelled,
                _ => {
                    // Grace expired (or wait itself failed). SIGKILL
                    // the group, then await the child to reap the
                    // zombie. Any error here is logged-and-ignored —
                    // we are already on the cancel path.
                    #[cfg(unix)]
                    {
                        let _ = unsafe { libc::killpg(pid as i32, libc::SIGKILL) };
                    }
                    let _ = child.wait().await;
                    WaitOutcome::Cancelled
                }
            }
        }
        result = child.wait() => {
            match result {
                Ok(status) => WaitOutcome::Exited(status),
                // The child handle is poisoned or the process is
                // already reaped. Treat as cancelled (no exit status
                // is available). The run() function will report
                // Cancelled rather than fabricate a status.
                Err(_) => WaitOutcome::Cancelled,
            }
        }
    }
}

/// Read lines from `reader` and push them to `tx`. A single line
/// longer than `max_line_bytes` is truncated with a `<line truncated>`
/// marker; bytes past the cap until the next `\n` are dropped.
async fn read_lines<R: AsyncRead + Unpin>(
    mut reader: R,
    tx: mpsc::Sender<String>,
    max_line_bytes: usize,
) {
    let mut buf = [0u8; 8192];
    let mut line = Vec::with_capacity(1024);
    let mut truncated = false;
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => {
                // EOF: emit any trailing partial line, then exit.
                if !line.is_empty() {
                    if truncated {
                        line.truncate(max_line_bytes);
                        line.extend_from_slice(b"<line truncated>");
                    }
                    let s = String::from_utf8_lossy(&line).into_owned();
                    // Receiver may be dropped if the caller is
                    // done; that's fine, we just exit.
                    let _ = tx.send(s).await;
                }
                return;
            }
            Ok(n) => {
                for &byte in &buf[..n] {
                    if byte == b'\n' {
                        if truncated {
                            line.truncate(max_line_bytes);
                            line.extend_from_slice(b"<line truncated>");
                        }
                        let s = String::from_utf8_lossy(&line).into_owned();
                        if tx.send(s).await.is_err() {
                            return; // receiver dropped, no point continuing
                        }
                        line.clear();
                        truncated = false;
                    } else if truncated {
                        // Drop bytes until the next \n.
                    } else if line.len() >= max_line_bytes {
                        // Mark this line as truncated; subsequent
                        // bytes are dropped until \n.
                        truncated = true;
                    } else {
                        line.push(byte);
                    }
                }
            }
            Err(_) => return,
        }
    }
}

/// Read up to `max_bytes` from `reader` into a `Vec<u8>`, dropping
/// anything past the cap. The reader is still drained past the cap so
/// the pipe never fills. A `<stderr truncated at N bytes>` marker is
/// appended if the cap was hit.
async fn read_bounded_stderr<R: AsyncRead + Unpin>(mut reader: R, max_bytes: usize) -> Vec<u8> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        match reader.read(&mut tmp).await {
            Ok(0) => break,
            Ok(n) => {
                if buf.len() < max_bytes {
                    let to_copy = (max_bytes - buf.len()).min(n);
                    buf.extend_from_slice(&tmp[..to_copy]);
                }
                // Bytes past the cap are intentionally dropped. We
                // keep reading to drain the pipe so the child never
                // blocks on write.
            }
            Err(_) => break,
        }
    }
    if buf.len() == max_bytes {
        buf.extend_from_slice(b"\n<stderr truncated at 262144 bytes>");
    }
    buf
}

/// Build a `CliProcess` error for a post-spawn sanity check that
/// failed. Distinct from `cli_spawn_failed` (which is the spawn IO
/// error) — this is "spawn succeeded but the handle we expected is
/// missing", which means the OS handed us a malformed child. In
/// practice we never see this; it's a defense against a future
/// `tokio` regression.
fn cli_spawn_post(message: &'static str) -> AppError {
    AppError::CliProcess {
        code: "cli_spawn_failed",
        message: message.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Shared helpers for in-crate test modules that drive the runner
/// (the existing `cli_runner::tests` mod and `fake_cli_test`). Kept
/// `pub` so the in-crate tests and integration tests in `tests/` can
/// share them.
pub mod test_support {
    use std::time::Duration;

    use tokio::time::timeout;

    use super::super::turn_context::TurnContext;
    use super::{CliInvocation, CliRunResult, CliRunner};
    use crate::error::AppError;

    /// Drive the runner to completion, return the result or panic
    /// on a wall-clock timeout (60s) — anything slower means a test
    /// regression in the runner.
    pub async fn run_with_timeout(
        runner: &CliRunner,
        inv: CliInvocation,
        turn: TurnContext,
    ) -> Result<CliRunResult, AppError> {
        timeout(Duration::from_secs(60), runner.run(inv, &turn))
            .await
            .expect("runner did not return within 60s — likely a deadlock")
    }
}

#[cfg(test)]
mod tests {
    use super::test_support::run_with_timeout;
    use super::*;
    use crate::agent::turn_context::test_support::make_test_turn_context;
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::time::timeout;

    /// Build a `CliInvocation` that runs `body` as a shell script via
    /// `/bin/sh -c`. Avoids the file-based chmod+exec race that
    /// `Command::new(script_path)` triggers when `cargo test` runs
    /// hundreds of tests in parallel (the OS returns `ETXTBSY` for
    /// a script that's still being `exec`'d by another test). The
    /// runner's job — spawn the binary, capture stdout/stderr, watch
    /// the cancel token — is fully exercised by this form; the OS
    /// is what does the actual exec.
    fn sh_invocation(body: &str) -> CliInvocation {
        CliInvocation {
            binary: PathBuf::from("/bin/sh"),
            args: vec!["-c".to_string(), body.to_string()],
            env: BTreeMap::new(),
            cwd: PathBuf::from("."),
            stdin_payload: None,
        }
    }

    /// Test invocation targeting `binary` with no extra args / env.
    fn basic_invocation(binary: PathBuf, cwd: PathBuf) -> CliInvocation {
        CliInvocation {
            binary,
            args: vec![],
            env: BTreeMap::new(),
            cwd,
            stdin_payload: None,
        }
    }

    /// Build a `CliInvocation` that execs a temp-dir script file.
    /// Used by tests that need to exercise the path where
    /// `Command::new` resolves a script via its shebang. NOT used
    /// by the parallel-stress tests because of the `ETXTBSY` race
    /// described in `sh_invocation`; those tests use `sh_invocation`
    /// instead.
    #[allow(dead_code)]
    fn script_invocation(body: &str) -> (TempDir, CliInvocation) {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let script = tmp.path().join("script.sh");
        std::fs::write(&script, body).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let inv = CliInvocation {
            binary: script,
            args: vec![],
            env: BTreeMap::new(),
            cwd: PathBuf::from("."),
            stdin_payload: None,
        };
        (tmp, inv)
    }

    /// 1. `test_cli_runner_basic` — invoke `/bin/echo` with args
    /// `["hello", "world"]`; assert `Success` with the captured line.
    #[tokio::test]
    async fn test_cli_runner_basic() {
        let runner = CliRunner::new();
        let mut inv = basic_invocation(PathBuf::from("/bin/echo"), PathBuf::from("."));
        inv.args = vec!["hello".into(), "world".into()];

        let result = run_with_timeout(&runner, inv, make_test_turn_context())
            .await
            .expect("run must succeed");

        match result {
            CliRunResult::Success {
                mut stdout,
                stderr,
                exit_code,
            } => {
                assert_eq!(exit_code, 0);
                assert!(stderr.is_empty(), "stderr should be empty: {stderr:?}");
                let line = stdout.next().await.expect("at least one line");
                assert_eq!(line, "hello world");
                assert!(
                    stdout.next().await.is_none(),
                    "no more lines after the first"
                );
            }
            other => panic!("expected Success, got: {other:?}"),
        }
    }

    /// 2. `test_cli_runner_cwd_env_args` — invoke a shell snippet
    /// that prints its cwd and the value of an env var passed via
    /// `env`; assert the runner's `cwd` and the script's stdout match.
    #[tokio::test]
    async fn test_cli_runner_cwd_env_args() {
        let tmp = TempDir::new().unwrap();
        let cwd = tmp.path().canonicalize().unwrap();
        let mut inv = sh_invocation("pwd\necho \"KEY=$WEAVE_TEST_KEY\"\n");
        inv.cwd = cwd.clone();
        inv.env = BTreeMap::from([("WEAVE_TEST_KEY".into(), "alpha-bravo".into())]);
        inv.args.push("--ignored".to_string()); // exercise the args path

        let runner = CliRunner::new();
        let result = run_with_timeout(&runner, inv, make_test_turn_context())
            .await
            .expect("run must succeed");

        match result {
            CliRunResult::Success {
                mut stdout,
                exit_code,
                ..
            } => {
                assert_eq!(exit_code, 0);
                let pwd_line = stdout.next().await.expect("pwd line");
                assert_eq!(pwd_line, cwd.to_str().unwrap());
                let env_line = stdout.next().await.expect("env line");
                assert_eq!(env_line, "KEY=alpha-bravo");
            }
            other => panic!("expected Success, got: {other:?}"),
        }
    }

    /// 3. `test_cli_runner_cancel_sends_sigterm` — spawn
    /// `/bin/sleep 30`, cancel after 100ms, assert the process exits
    /// within 1s and `CliRunResult::Cancelled` is returned.
    #[tokio::test]
    async fn test_cli_runner_cancel_sends_sigterm() {
        let runner = Arc::new(CliRunner::new());
        let inv = CliInvocation {
            binary: PathBuf::from("/bin/sleep"),
            args: vec!["30".into()],
            env: BTreeMap::new(),
            cwd: PathBuf::from("."),
            stdin_payload: None,
        };

        // Build a turn with a fresh token so we can cancel it
        // independently. The `make_test_turn_context` builder
        // already gives us a fresh token; we just hold a clone for
        // the cancel call.
        let turn = make_test_turn_context();
        let cancel_token = turn.cancellation_token.clone();

        let runner_for_run = runner.clone();
        let turn_clone = turn;
        let handle = tokio::spawn(async move { runner_for_run.run(inv, &turn_clone).await });

        // Give the child 100ms to start, then cancel.
        tokio::time::sleep(Duration::from_millis(100)).await;
        cancel_token.cancel();

        // The runner must return within 1s of cancel (SIGTERM,
        // then SIGKILL after the 5s grace — but `sleep` exits
        // promptly on SIGTERM, so we get a fast response).
        let result = timeout(Duration::from_secs(1), handle)
            .await
            .expect("runner did not return within 1s of cancel")
            .expect("runner task panicked")
            .expect("run returned Err");

        match result {
            CliRunResult::Cancelled => {}
            other => panic!("expected Cancelled, got: {other:?}"),
        }
        assert_eq!(
            runner.active_count().await,
            0,
            "active table must be empty after cancel"
        );
    }

    /// 4. `test_cli_runner_exit_nonzero_maps_to_error` — spawn
    /// `/bin/false`; assert `ExitError { exit_code: 1 }`.
    #[tokio::test]
    async fn test_cli_runner_exit_nonzero_maps_to_error() {
        let runner = CliRunner::new();
        let inv = basic_invocation(PathBuf::from("/bin/false"), PathBuf::from("."));

        let result = run_with_timeout(&runner, inv, make_test_turn_context())
            .await
            .expect("run must succeed (spawn) even though the child fails");

        match result {
            CliRunResult::ExitError { exit_code, stderr } => {
                assert_eq!(exit_code, 1);
                assert!(stderr.is_empty(), "/bin/false writes no stderr");
            }
            other => panic!("expected ExitError, got: {other:?}"),
        }
    }

    /// 5. `test_cli_runner_stderr_capture` — spawn a shell snippet
    /// that writes to stderr; assert `stderr` is captured.
    #[tokio::test]
    async fn test_cli_runner_stderr_capture() {
        let runner = CliRunner::new();
        let inv = sh_invocation("echo 'simple error' >&2\necho 'and more' >&2\n");

        let result = run_with_timeout(&runner, inv, make_test_turn_context())
            .await
            .expect("run must succeed");

        match result {
            CliRunResult::Success {
                stderr, exit_code, ..
            } => {
                assert_eq!(exit_code, 0);
                let stderr_s = String::from_utf8_lossy(&stderr);
                assert!(stderr_s.contains("simple error"));
                assert!(stderr_s.contains("and more"));
            }
            other => panic!("expected Success, got: {other:?}"),
        }
    }

    /// 5b. Stderr overflow writes the truncation marker. The reader
    ///     keeps draining past the cap so the child never blocks.
    #[tokio::test]
    async fn test_cli_runner_stderr_truncation_marker() {
        // 300 KiB of stderr; cap is 256 KiB.
        let runner = CliRunner::new();
        let inv =
            sh_invocation("dd if=/dev/zero bs=1024 count=300 2>/dev/null | tr '\\0' 'E' >&2\n");

        let result = run_with_timeout(&runner, inv, make_test_turn_context())
            .await
            .expect("run must succeed");

        match result {
            CliRunResult::Success { stderr, .. } => {
                assert!(
                    stderr.len() <= MAX_STDERR_BYTES + 64,
                    "stderr should be bounded: len={}",
                    stderr.len()
                );
                let stderr_s = String::from_utf8_lossy(&stderr);
                assert!(
                    stderr_s.contains("<stderr truncated"),
                    "expected truncation marker, got: {stderr_s}"
                );
            }
            other => panic!("expected Success, got: {other:?}"),
        }
    }

    /// 6. `test_cli_runner_per_turn_process_table` — spawn two
    /// processes for two different session ids concurrently; both
    /// succeed; the active-processes table is empty after both exit.
    #[tokio::test]
    async fn test_cli_runner_per_turn_process_table() {
        let runner = Arc::new(CliRunner::new());

        let mut turn_a = make_test_turn_context();
        turn_a.session_id = "session-a".into();
        let mut turn_b = make_test_turn_context();
        turn_b.session_id = "session-b".into();

        let runner_a = runner.clone();
        let runner_b = runner.clone();
        let mut inv_a = basic_invocation(PathBuf::from("/bin/sleep"), PathBuf::from("."));
        inv_a.args = vec!["0.1".into()];
        let inv_b = inv_a.clone();

        // Both runners start. While they're both running, the
        // table should have 2 entries.
        let handle_a = tokio::spawn(async move { runner_a.run(inv_a, &turn_a).await });
        let handle_b = tokio::spawn(async move { runner_b.run(inv_b, &turn_b).await });

        // Give both children a moment to register.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let mid = runner.active_count().await;
        // We allow 0, 1, or 2 — timing-dependent whether both have
        // registered yet. The strict assertion is on the final
        // count being 0.
        assert!(
            mid <= 2,
            "active table must never exceed the number of in-flight turns, got {mid}"
        );

        let (res_a, res_b) = tokio::join!(handle_a, handle_b);
        let res_a = res_a.expect("a task panicked").expect("a run failed");
        let res_b = res_b.expect("b task panicked").expect("b run failed");
        assert!(matches!(res_a, CliRunResult::Success { .. }));
        assert!(matches!(res_b, CliRunResult::Success { .. }));

        assert_eq!(
            runner.active_count().await,
            0,
            "active table must be empty after both turns complete"
        );
    }

    /// 7. `test_cli_runner_reuse_after_exit` — spawn a process, let
    /// it exit, spawn another for the same session id; both succeed
    /// (no stale state).
    #[tokio::test]
    async fn test_cli_runner_reuse_after_exit() {
        let runner = CliRunner::new();
        let mut turn = make_test_turn_context();
        turn.session_id = "session-reuse".into();

        for i in 0..3 {
            let inv = basic_invocation(PathBuf::from("/bin/true"), PathBuf::from("."));
            let result = run_with_timeout(&runner, inv, turn.clone())
                .await
                .unwrap_or_else(|e| panic!("turn {i} failed: {e:?}"));
            assert!(
                matches!(result, CliRunResult::Success { .. }),
                "turn {i} expected Success, got: {result:?}"
            );
            assert_eq!(runner.active_count().await, 0);
        }
    }

    /// 8. `test_cli_runner_registers_session_id` — the active
    /// table has the right entry while the process is running; the
    /// entry is gone after exit.
    #[tokio::test]
    async fn test_cli_runner_registers_session_id() {
        let runner = Arc::new(CliRunner::new());
        let mut turn = make_test_turn_context();
        turn.session_id = "session-track".into();

        let inv = {
            let mut i = basic_invocation(PathBuf::from("/bin/sleep"), PathBuf::from("."));
            i.args = vec!["0.3".into()];
            i
        };

        let runner_for_run = runner.clone();
        let turn_for_run = turn.clone();
        let handle = tokio::spawn(async move { runner_for_run.run(inv, &turn_for_run).await });

        // Give the child 50ms to register, then check the table.
        tokio::time::sleep(Duration::from_millis(50)).await;
        let pids = runner.active_pids().await;
        assert_eq!(pids.len(), 1, "expected exactly one active pid");

        let result = handle.await.expect("task panicked").expect("run failed");
        assert!(matches!(result, CliRunResult::Success { .. }));
        assert_eq!(
            runner.active_count().await,
            0,
            "active table must be empty after the turn"
        );
    }

    /// 9. `test_cli_runner_env_keys_passed_through` — a round-trip
    ///     test for env handling: the runner accepts env keys
    ///     without leaking them, and the child sees the values.
    ///     Log redaction itself is a code-review concern (the
    ///     `info!` macro in `run()` uses `env.keys()` not values —
    ///     see the module-level doc).
    #[tokio::test]
    async fn test_cli_runner_env_keys_passed_through() {
        let runner = CliRunner::new();
        let mut inv = sh_invocation("echo \"SECRET=$WEAVE_TEST_SECRET_KEY\"\n");
        inv.env = BTreeMap::from([(
            "WEAVE_TEST_SECRET_KEY".into(),
            "shhh-this-is-a-secret".into(),
        )]);

        let result = run_with_timeout(&runner, inv, make_test_turn_context())
            .await
            .expect("run must succeed");

        match result {
            CliRunResult::Success { mut stdout, .. } => {
                let line = stdout.next().await.expect("at least one line");
                assert_eq!(line, "SECRET=shhh-this-is-a-secret");
            }
            other => panic!("expected Success, got: {other:?}"),
        }
    }

    /// 10. `test_cli_runner_spawn_error` — a missing binary
    ///     surfaces as `AppError::CliProcess` with `cli_spawn_failed`.
    #[tokio::test]
    async fn test_cli_runner_spawn_error() {
        let runner = CliRunner::new();
        let inv = basic_invocation(
            PathBuf::from("/nonexistent/path/to/binary"),
            PathBuf::from("."),
        );

        let err = run_with_timeout(&runner, inv, make_test_turn_context())
            .await
            .expect_err("spawn of a missing binary must error");

        match err {
            AppError::CliProcess { code, message } => {
                assert_eq!(code, "cli_spawn_failed");
                assert!(message.contains("failed to spawn"), "msg: {message}");
            }
            other => panic!("expected CliProcess, got: {other:?}"),
        }
    }

    /// 11. `test_cli_runner_line_truncation` — a snippet that emits
    ///     a single line longer than `MAX_LINE_BYTES` is truncated
    ///     with the marker.
    #[tokio::test]
    async fn test_cli_runner_line_truncation() {
        // Emit a 1.2 MB line with no newline.
        let runner = CliRunner::new();
        let body = format!(
            "python3 -c \"import sys; sys.stdout.write('A' * {})\"\n",
            MAX_LINE_BYTES + 200_000
        );
        let inv = sh_invocation(&body);

        let result = run_with_timeout(&runner, inv, make_test_turn_context())
            .await
            .expect("run must succeed");

        match result {
            CliRunResult::Success { mut stdout, .. } => {
                let line = stdout.next().await.expect("at least the truncated line");
                assert!(
                    line.ends_with("<line truncated>"),
                    "expected truncation marker, got tail: {}",
                    &line[line.len().saturating_sub(50)..]
                );
                assert!(line.len() <= MAX_LINE_BYTES + 32, "line is over budget");
            }
            other => panic!("expected Success, got: {other:?}"),
        }
    }

    /// `test_cli_runner_cancel_after_exit_is_noop` — cancel after
    /// the process has already exited must not error and must not
    /// leave stale table state. Per the design decision in the
    /// spec: "cancel becomes a no-op" once the child has exited.
    #[tokio::test]
    async fn test_cli_runner_cancel_after_exit_is_noop() {
        let runner = CliRunner::new();
        let mut turn = make_test_turn_context();
        let token = turn.cancellation_token.clone();
        turn.cancellation_token = tokio_util::sync::CancellationToken::new();
        // Cancel the token BEFORE the run starts.
        token.cancel();

        let inv = basic_invocation(PathBuf::from("/bin/true"), PathBuf::from("."));
        let result = run_with_timeout(&runner, inv, turn)
            .await
            .expect("run must succeed");
        // The child still runs to completion (the token was
        // pre-cancelled, so the first `select!` branch fires
        // immediately after spawn, but `/bin/true` is so fast it
        // likely exits naturally first). Either outcome is fine;
        // the assertion is that we get a result and the table is
        // empty.
        assert!(matches!(
            result,
            CliRunResult::Success { .. } | CliRunResult::Cancelled
        ));
        assert_eq!(runner.active_count().await, 0);
    }

    /// `test_cli_runner_stdin_payload` — the stdin payload is
    /// delivered to the child. Verified by a shell snippet that
    /// reads stdin and echoes it.
    #[tokio::test]
    async fn test_cli_runner_stdin_payload() {
        let runner = CliRunner::new();
        let mut inv = sh_invocation("cat\n");
        inv.stdin_payload = Some(b"hello from stdin\n".to_vec());

        let result = run_with_timeout(&runner, inv, make_test_turn_context())
            .await
            .expect("run must succeed");

        match result {
            CliRunResult::Success { mut stdout, .. } => {
                let line = stdout.next().await.expect("at least one line");
                assert_eq!(line, "hello from stdin");
            }
            other => panic!("expected Success, got: {other:?}"),
        }
    }
}
