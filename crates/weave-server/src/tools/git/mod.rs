//! Git tools for agent sessions.
//!
//! Provides four tools: `git_status`, `git_diff`, `git_log`, `git_commit`.
//! All tools validate that the working directory is inside a git repository.

mod commit;
mod diff;
mod log;
mod status;

pub use commit::GitCommitTool;
pub use diff::GitDiffTool;
pub use log::GitLogTool;
pub use status::GitStatusTool;

use std::path::PathBuf;

use serde_json::Value;

use super::fs::{optional_string, PathValidator};
use super::{spawn_read_task, truncate_bytes};
use crate::tools::{ToolContext, ToolResult};

// Re-export helpers for submodules.
pub(crate) use crate::tools::fs::{error, success};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Maximum bytes for diff output truncation.
pub(crate) const MAX_DIFF_BYTES: usize = 50 * 1024; // 50KB

// ---------------------------------------------------------------------------
// GitOutput
// ---------------------------------------------------------------------------

/// Structured result from running a git command.
pub(crate) struct GitOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

// ---------------------------------------------------------------------------
// Schema helpers
// ---------------------------------------------------------------------------

/// Build the common `cwd` property for git tool input schemas.
pub(crate) fn cwd_property() -> Value {
    serde_json::json!({
        "type": "string",
        "description": "Working directory (absolute path). Must be a git repo. Defaults to session cwd."
    })
}

// ---------------------------------------------------------------------------
// run_git
// ---------------------------------------------------------------------------

/// Run a git command with the given args in the given cwd.
///
/// Uses `tokio::process::Command` directly (not `sh -c`).
/// Returns `Ok(GitOutput)` on success, or `Err(ToolResult)` if the process
/// could not be spawned (e.g. git not installed).
pub(crate) async fn run_git(args: &[&str], cwd: &PathBuf) -> Result<GitOutput, ToolResult> {
    use std::process::Stdio;
    use tokio::process::Command;

    let mut cmd = Command::new("git");
    cmd.args(args)
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(unix)]
    {
        cmd.process_group(0);
    }

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(e) => {
            return Err(error(format!(
                "Failed to spawn git process: {}. \
                 Ensure git is installed and available on PATH.",
                e
            )));
        }
    };

    let stdout_handle = child.stdout.take();
    let stderr_handle = child.stderr.take();

    let stdout_task = spawn_read_task(stdout_handle);
    let stderr_task = spawn_read_task(stderr_handle);

    let status = child.wait().await.map_err(|e| {
        error(format!(
            "Failed to wait for git process: {}. \
             The process may have been killed externally.",
            e
        ))
    })?;

    let stdout_bytes = stdout_task.await.unwrap_or_default();
    let stderr_bytes = stderr_task.await.unwrap_or_default();

    Ok(GitOutput {
        stdout: String::from_utf8_lossy(&stdout_bytes).into_owned(),
        stderr: String::from_utf8_lossy(&stderr_bytes).into_owned(),
        exit_code: status.code().unwrap_or(-1),
    })
}

// ---------------------------------------------------------------------------
// validate_git_cwd
// ---------------------------------------------------------------------------

/// Validate that `cwd` is absolute, exists, and is inside a git repository.
///
/// Async — uses `run_git` to avoid blocking the tokio runtime.
pub(crate) async fn validate_git_cwd(
    input: &Value,
    ctx: &ToolContext,
) -> Result<PathBuf, ToolResult> {
    let cwd = match optional_string(input, "cwd") {
        Some(path_str) => PathValidator::require_absolute(&path_str)?,
        None => ctx.cwd.clone(),
    };

    if !cwd.is_dir() {
        return Err(error(format!(
            "Working directory '{}' does not exist or is not a directory.",
            cwd.display()
        )));
    }

    match run_git(&["rev-parse", "--git-dir"], &cwd).await {
        Ok(o) if o.exit_code == 0 => Ok(cwd),
        Ok(o) => Err(error(format!(
            "Not a git repository: {}. \
             Ensure the working directory is inside a git repo.",
            o.stderr.trim()
        ))),
        Err(e) => Err(e),
    }
}

// ---------------------------------------------------------------------------
// truncate_diff (convenience wrapper)
// ---------------------------------------------------------------------------

/// Truncate diff output to `MAX_DIFF_BYTES`, returning (content, truncated_flag).
pub(crate) fn truncate_diff(bytes: &[u8]) -> (String, bool) {
    truncate_bytes(bytes, MAX_DIFF_BYTES)
}

// ---------------------------------------------------------------------------
// validate_commit_identity
// ---------------------------------------------------------------------------

/// Placeholder names that should not appear in real commits.
const PLACEHOLDER_NAMES: &[&str] = &[
    "test",
    "tester",
    "testing",
    "example",
    "user",
    "nobody",
    "root",
    "admin",
    "placeholder",
    "todo",
    "fixme",
    "unknown",
    "dev",
    "developer",
];

/// Placeholder email domains that should not appear in real commits.
const PLACEHOLDER_DOMAINS: &[&str] = &[
    "@example.com",
    "@test.com",
    "@localhost",
    "@example.org",
    "@test.org",
];

/// Validate git commit identity (user.name and user.email).
///
/// Rejects empty, placeholder, and obviously fake identities.
/// Returns `Ok(())` if valid, or `Err(ToolResult)` with a descriptive error.
pub(crate) fn validate_commit_identity(name: &str, email: &str) -> Result<(), ToolResult> {
    let name_trimmed = name.trim();
    let email_trimmed = email.trim();

    if name_trimmed.is_empty() {
        return Err(error(
            "git user.name is not configured. \
             Run: git config user.name \"Your Name\"",
        ));
    }

    if email_trimmed.is_empty() {
        return Err(error(
            "git user.email is not configured. \
             Run: git config user.email \"you@example.com\"",
        ));
    }

    let name_lower = name_trimmed.to_lowercase();
    if PLACEHOLDER_NAMES.iter().any(|&p| name_lower == p) {
        return Err(error(format!(
            "git user.name '{}' looks like a placeholder. \
             Configure a real name: git config user.name \"Your Name\"",
            name_trimmed
        )));
    }

    let email_lower = email_trimmed.to_lowercase();
    if PLACEHOLDER_DOMAINS
        .iter()
        .any(|&d| email_lower.ends_with(d))
    {
        return Err(error(format!(
            "git user.email '{}' uses a placeholder domain. \
             Configure a real email: git config user.email \"you@company.com\"",
            email_trimmed
        )));
    }

    if name_lower == email_lower {
        return Err(error(format!(
            "git user.name and user.email are the same ('{}'). \
             This usually indicates a misconfigured git identity.",
            name_trimmed
        )));
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Test support
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod git_test_support {
    use std::path::Path;

    /// Initialize a git repo in `dir` with the given identity.
    /// If `commit` is true, creates an empty initial commit.
    pub(crate) fn git_init(dir: &Path, name: &str, email: &str, commit: bool) {
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .status()
                .unwrap();
        };
        run(&["init", "-b", "main"]);
        run(&["config", "user.name", name]);
        run(&["config", "user.email", email]);
        if commit {
            run(&["commit", "--allow-empty", "-m", "init"]);
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_validate_commit_identity_valid() {
        assert!(validate_commit_identity("Alice Smith", "alice@company.com").is_ok());
    }

    #[test]
    fn test_validate_commit_identity_empty_name() {
        let result = validate_commit_identity("", "alice@company.com");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(!err.success);
        assert!(err.error.unwrap().contains("user.name is not configured"));
    }

    #[test]
    fn test_validate_commit_identity_empty_email() {
        let result = validate_commit_identity("Alice", "");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(!err.success);
        assert!(err.error.unwrap().contains("user.email is not configured"));
    }

    #[test]
    fn test_validate_commit_identity_placeholder_name() {
        for name in &[
            "test", "tester", "example", "user", "nobody", "root", "admin",
        ] {
            let result = validate_commit_identity(name, "alice@company.com");
            assert!(result.is_err(), "should reject placeholder name: {}", name);
            let err = result.unwrap_err();
            assert!(err.error.unwrap().contains("placeholder"));
        }
    }

    #[test]
    fn test_validate_commit_identity_placeholder_email() {
        for domain in &["@example.com", "@test.com", "@localhost"] {
            let email = format!("user{}", domain);
            let result = validate_commit_identity("Alice", &email);
            assert!(
                result.is_err(),
                "should reject placeholder email: {}",
                email
            );
            let err = result.unwrap_err();
            assert!(err.error.unwrap().contains("placeholder domain"));
        }
    }

    #[test]
    fn test_validate_commit_identity_name_equals_email() {
        let result = validate_commit_identity("alice@company.com", "alice@company.com");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.error.unwrap().contains("same"));
    }

    #[test]
    fn test_validate_commit_identity_case_insensitive() {
        assert!(validate_commit_identity("TEST", "alice@company.com").is_err());
        assert!(validate_commit_identity("Alice", "user@EXAMPLE.COM").is_err());
    }

    #[tokio::test]
    async fn test_validate_git_cwd_not_a_repo() {
        let tmp = TempDir::new().unwrap();
        let ctx = crate::tools::test_support::make_context(tmp.path());
        let input = serde_json::json!({"cwd": tmp.path().to_str().unwrap()});
        let result = validate_git_cwd(&input, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.error.unwrap().contains("Not a git repository"));
    }

    #[tokio::test]
    async fn test_validate_git_cwd_relative_path() {
        let tmp = TempDir::new().unwrap();
        let ctx = crate::tools::test_support::make_context(tmp.path());
        let input = serde_json::json!({"cwd": "relative/path"});
        let result = validate_git_cwd(&input, &ctx).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_truncate_diff_no_truncation() {
        let bytes = b"small diff";
        let (content, truncated) = truncate_diff(bytes);
        assert_eq!(content, "small diff");
        assert!(!truncated);
    }

    #[test]
    fn test_truncate_diff_with_truncation() {
        let bytes = vec![b'a'; 60 * 1024];
        let (content, truncated) = truncate_diff(&bytes);
        assert!(truncated);
        assert!(content.len() <= MAX_DIFF_BYTES);
    }

    #[test]
    fn test_truncate_diff_utf8_boundary() {
        // Create bytes with a multi-byte UTF-8 char at the boundary.
        let mut bytes = vec![b'x'; MAX_DIFF_BYTES - 2];
        // Add a 3-byte UTF-8 char (U+20AC = €)
        bytes.extend_from_slice(&[0xE2, 0x82, 0xAC]);
        let (content, truncated) = truncate_diff(&bytes);
        assert!(truncated);
        // Should cut before the multi-byte char
        assert!(content.len() <= MAX_DIFF_BYTES);
    }
}
