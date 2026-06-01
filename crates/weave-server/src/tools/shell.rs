//! `shell_exec` — execute shell commands with timeout and output capture.

use std::path::PathBuf;
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::process::Command;

use super::fs::{error, optional_string, optional_u64, require_string, success, PathValidator};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

/// Maximum bytes captured per stream (stdout, stderr).
const MAX_STREAM_BYTES: usize = 100 * 1024; // 100KB

/// Default timeout in milliseconds.
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

pub struct ShellExecTool;

#[async_trait]
impl ToolExecutor for ShellExecTool {
    fn name(&self) -> &str {
        "shell_exec"
    }

    fn description(&self) -> &str {
        "Execute a shell command and return its output. Runs via `sh -c`. \
         Captures stdout, stderr, and exit code. Timeout kills the process. \
         Output is truncated at 100KB per stream."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "Shell command to execute"
                },
                "cwd": {
                    "type": "string",
                    "description": "Working directory (absolute path). Defaults to session cwd."
                },
                "timeout_ms": {
                    "type": "integer",
                    "description": "Timeout in milliseconds. Defaults to 30000."
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        // Parse inputs
        let command = match require_string(&input, "command") {
            Ok(s) => s,
            Err(e) => return e,
        };

        let cwd = match optional_string(&input, "cwd") {
            Some(path_str) => match PathValidator::require_absolute(&path_str) {
                Ok(p) => p,
                Err(e) => return e,
            },
            None => ctx.cwd.clone(),
        };

        // Validate cwd exists and is a directory
        if !cwd.is_dir() {
            return error(format!(
                "Working directory '{}' does not exist or is not a directory.",
                cwd.display()
            ));
        }

        let timeout_ms = optional_u64(&input, "timeout_ms").unwrap_or(DEFAULT_TIMEOUT_MS);

        // Log trace event (DEBUG to avoid persisting secrets in command strings)
        tracing::debug!(
            session_id = %ctx.session_id,
            command = %command,
            cwd = %cwd.display(),
            timeout_ms = timeout_ms,
            "shell_exec"
        );

        // Execute command
        run_command(&command, &cwd, timeout_ms).await
    }
}

/// Execute a shell command with timeout and output capture.
///
/// Uses `child.wait()` instead of `child.wait_with_output()` so we can still
/// call `child.kill()` on timeout. Stdout/stderr are read in separate tasks.
async fn run_command(command: &str, cwd: &PathBuf, timeout_ms: u64) -> ToolResult {
    let mut child = match Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(e) => {
            return error(format!(
                "Failed to spawn shell process: {}. \
                 Check that 'sh' is available on the system.",
                e
            ));
        }
    };

    // Take ownership of stdout/stderr handles before waiting.
    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    // Spawn tasks to read output streams concurrently.
    let stdout_task = spawn_read_task(stdout_handle);
    let stderr_task = spawn_read_task(stderr_handle);

    let start = std::time::Instant::now();

    // Wait for the process with timeout.
    // child.wait() borrows mutably, so we can still call kill() on timeout.
    match tokio::time::timeout(Duration::from_millis(timeout_ms), child.wait()).await {
        Ok(Ok(status)) => {
            let duration_ms = start.elapsed().as_millis() as u64;
            let exit_code = status.code().unwrap_or(-1);

            // Collect output from the spawned tasks.
            let stdout_bytes = stdout_task.await.unwrap_or_default();
            let stderr_bytes = stderr_task.await.unwrap_or_default();

            let (stdout, stdout_truncated) = truncate_output(&stdout_bytes);
            let (stderr, stderr_truncated) = truncate_output(&stderr_bytes);

            success(json!({
                "stdout": stdout,
                "stderr": stderr,
                "exit_code": exit_code,
                "stdout_truncated": stdout_truncated,
                "stderr_truncated": stderr_truncated,
                "duration_ms": duration_ms
            }))
        }
        Ok(Err(e)) => {
            // Await reader tasks to release pipe FDs.
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            error(format!(
                "Failed to collect command output: {}. \
                 The process may have been killed externally.",
                e
            ))
        }
        Err(_) => {
            // Timeout — kill the child process and reap the zombie.
            let _ = child.kill().await;
            let _ = child.wait().await;
            // Await reader tasks to release pipe FDs.
            let _ = stdout_task.await;
            let _ = stderr_task.await;
            error(format!(
                "Command timed out after {}ms. Process was killed.",
                timeout_ms
            ))
        }
    }
}

/// Spawn a task that reads an async reader to completion, returning the bytes.
fn spawn_read_task(
    handle: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>,
) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        match handle {
            Some(mut h) => {
                use tokio::io::AsyncReadExt;
                let mut buf = Vec::new();
                h.read_to_end(&mut buf).await.ok();
                buf
            }
            None => Vec::new(),
        }
    })
}

/// Truncate output to `MAX_STREAM_BYTES`, returning (content, truncated_flag).
///
/// Finds a UTF-8 boundary at or before `MAX_STREAM_BYTES`. Uses `from_utf8_lossy`
/// to handle any incomplete multi-byte sequence at the boundary.
fn truncate_output(bytes: &[u8]) -> (String, bool) {
    if bytes.len() <= MAX_STREAM_BYTES {
        return (String::from_utf8_lossy(bytes).into_owned(), false);
    }

    // Find last valid UTF-8 boundary at or before MAX_STREAM_BYTES.
    let mut end = MAX_STREAM_BYTES;
    while end > 0 && (bytes[end] & 0xC0) == 0x80 {
        end -= 1;
    }

    (String::from_utf8_lossy(&bytes[..end]).into_owned(), true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::make_context;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_shell_exec_basic() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = ShellExecTool
            .execute(json!({"command": "echo hello"}), &ctx)
            .await;

        assert!(result.success);
        assert_eq!(result.data["stdout"], "hello\n");
        assert_eq!(result.data["stderr"], "");
        assert_eq!(result.data["exit_code"], 0);
        assert_eq!(result.data["stdout_truncated"], false);
        assert_eq!(result.data["stderr_truncated"], false);
    }

    #[tokio::test]
    async fn test_shell_exec_stderr_capture() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = ShellExecTool
            .execute(json!({"command": "echo err >&2"}), &ctx)
            .await;

        assert!(result.success);
        assert_eq!(result.data["stdout"], "");
        assert_eq!(result.data["stderr"], "err\n");
        assert_eq!(result.data["exit_code"], 0);
    }

    #[tokio::test]
    async fn test_shell_exec_nonzero_exit() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = ShellExecTool
            .execute(json!({"command": "exit 42"}), &ctx)
            .await;

        // Non-zero exit is still success=true — the tool worked, the command failed.
        assert!(result.success);
        assert_eq!(result.data["exit_code"], 42);
    }

    #[tokio::test]
    async fn test_shell_exec_timeout() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = ShellExecTool
            .execute(json!({"command": "sleep 60", "timeout_ms": 100}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("timed out"));
    }

    #[tokio::test]
    async fn test_shell_exec_cwd() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = ShellExecTool
            .execute(
                json!({"command": "pwd", "cwd": tmp.path().to_str().unwrap()}),
                &ctx,
            )
            .await;

        assert!(result.success);
        let stdout = result.data["stdout"].as_str().unwrap().trim();
        // pwd returns the canonical path
        assert_eq!(stdout, tmp.path().canonicalize().unwrap().to_str().unwrap());
    }

    #[tokio::test]
    async fn test_shell_exec_cwd_defaults_to_ctx() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = ShellExecTool.execute(json!({"command": "pwd"}), &ctx).await;

        assert!(result.success);
        let stdout = result.data["stdout"].as_str().unwrap().trim();
        assert_eq!(stdout, tmp.path().canonicalize().unwrap().to_str().unwrap());
    }

    #[tokio::test]
    async fn test_shell_exec_relative_cwd_rejected() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = ShellExecTool
            .execute(json!({"command": "pwd", "cwd": "relative/path"}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("must be absolute"));
    }

    #[tokio::test]
    async fn test_shell_exec_nonexistent_cwd() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = ShellExecTool
            .execute(
                json!({"command": "pwd", "cwd": "/nonexistent/directory"}),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("does not exist"));
    }

    #[tokio::test]
    async fn test_shell_exec_missing_command() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = ShellExecTool.execute(json!({}), &ctx).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Missing"));
    }

    #[tokio::test]
    async fn test_shell_exec_output_truncation() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        // Generate >100KB of output
        // dd if=/dev/zero bs=1024 count=150 produces 150KB
        let result = ShellExecTool
            .execute(
                json!({"command": "dd if=/dev/zero bs=1024 count=150 2>/dev/null | tr '\\0' 'A'"}),
                &ctx,
            )
            .await;

        assert!(result.success);
        assert_eq!(result.data["stdout_truncated"], true);
        let stdout = result.data["stdout"].as_str().unwrap();
        assert!(stdout.len() <= MAX_STREAM_BYTES);
    }

    #[tokio::test]
    async fn test_shell_exec_empty_output() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = ShellExecTool
            .execute(json!({"command": "true"}), &ctx)
            .await;

        assert!(result.success);
        assert_eq!(result.data["stdout"], "");
        assert_eq!(result.data["stderr"], "");
        assert_eq!(result.data["exit_code"], 0);
    }
}
