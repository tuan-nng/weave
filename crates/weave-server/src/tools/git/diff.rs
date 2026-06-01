//! `git_diff` — get unified diff of changes with optional filters.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{run_git, truncate_diff, validate_git_cwd};
use crate::tools::fs::{optional_bool, optional_string};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct GitDiffTool;

#[async_trait]
impl ToolExecutor for GitDiffTool {
    fn name(&self) -> &str {
        "git_diff"
    }

    fn description(&self) -> &str {
        "Get unified diff of changes. Output truncated at 50KB with truncated flag."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cwd": super::cwd_property(),
                "staged": {
                    "type": "boolean",
                    "description": "Show staged changes instead of working tree. Defaults to false."
                },
                "file": {
                    "type": "string",
                    "description": "Restrict diff to a specific file path (relative to repo root)."
                }
            },
            "required": []
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let cwd = match validate_git_cwd(&input, ctx).await {
            Ok(p) => p,
            Err(e) => return e,
        };

        let staged = optional_bool(&input, "staged");
        let file = optional_string(&input, "file");

        tracing::debug!(
            session_id = %ctx.session_id,
            cwd = %cwd.display(),
            staged = staged,
            file = ?file,
            "git_diff"
        );

        let mut args = vec!["diff", "--no-color"];
        if staged {
            args.push("--cached");
        }
        if let Some(ref f) = file {
            args.push("--");
            args.push(f);
        }

        let output = match run_git(&args, &cwd).await {
            Ok(o) => o,
            Err(e) => return e,
        };

        if output.exit_code != 0 {
            return super::error(format!("git diff failed: {}", output.stderr.trim()));
        }

        let (diff, truncated) = truncate_diff(output.stdout.as_bytes());

        super::success(json!({
            "diff": diff,
            "truncated": truncated
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::git::git_test_support::git_init;
    use crate::tools::test_support::make_context;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_git_diff_empty() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Test", "test@test.com", true);
        let ctx = make_context(tmp.path());

        let result = GitDiffTool
            .execute(json!({"cwd": tmp.path().to_str().unwrap()}), &ctx)
            .await;

        assert!(result.success);
        assert_eq!(result.data["diff"], "");
        assert_eq!(result.data["truncated"], false);
    }

    #[tokio::test]
    async fn test_git_diff_unstaged() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Test", "test@test.com", true);
        std::fs::write(tmp.path().join("file.txt"), "hello\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "add file"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::fs::write(tmp.path().join("file.txt"), "hello\nworld\n").unwrap();
        let ctx = make_context(tmp.path());

        let result = GitDiffTool
            .execute(json!({"cwd": tmp.path().to_str().unwrap()}), &ctx)
            .await;

        assert!(result.success);
        let diff = result.data["diff"].as_str().unwrap();
        assert!(diff.contains("+world"));
        assert_eq!(result.data["truncated"], false);
    }

    #[tokio::test]
    async fn test_git_diff_staged() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Test", "test@test.com", true);
        std::fs::write(tmp.path().join("file.txt"), "hello\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "add file"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::fs::write(tmp.path().join("file.txt"), "hello\nworld\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        let ctx = make_context(tmp.path());

        let result = GitDiffTool
            .execute(
                json!({"cwd": tmp.path().to_str().unwrap(), "staged": true}),
                &ctx,
            )
            .await;

        assert!(result.success);
        let diff = result.data["diff"].as_str().unwrap();
        assert!(diff.contains("+world"));
    }

    #[tokio::test]
    async fn test_git_diff_file_filter() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Test", "test@test.com", true);
        std::fs::write(tmp.path().join("a.txt"), "a\n").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "b\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "a.txt", "b.txt"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "add files"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::fs::write(tmp.path().join("a.txt"), "a\nmodified\n").unwrap();
        std::fs::write(tmp.path().join("b.txt"), "b\nmodified\n").unwrap();
        let ctx = make_context(tmp.path());

        let result = GitDiffTool
            .execute(
                json!({"cwd": tmp.path().to_str().unwrap(), "file": "a.txt"}),
                &ctx,
            )
            .await;

        assert!(result.success);
        let diff = result.data["diff"].as_str().unwrap();
        assert!(diff.contains("+modified"));
        assert!(!diff.contains("b.txt"));
    }

    #[tokio::test]
    async fn test_git_diff_truncation() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Test", "test@test.com", true);
        // Create a file with >50KB of content.
        let content = "x".repeat(60 * 1024);
        std::fs::write(tmp.path().join("big.txt"), &content).unwrap();
        std::process::Command::new("git")
            .args(["add", "big.txt"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::process::Command::new("git")
            .args(["commit", "-m", "add big file"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        // Modify to generate a large diff.
        let modified = "y".repeat(60 * 1024);
        std::fs::write(tmp.path().join("big.txt"), &modified).unwrap();
        let ctx = make_context(tmp.path());

        let result = GitDiffTool
            .execute(json!({"cwd": tmp.path().to_str().unwrap()}), &ctx)
            .await;

        assert!(result.success);
        assert_eq!(result.data["truncated"], true);
        let diff = result.data["diff"].as_str().unwrap();
        assert!(diff.len() <= super::super::MAX_DIFF_BYTES);
    }

    #[tokio::test]
    async fn test_git_diff_not_a_repo() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = GitDiffTool
            .execute(json!({"cwd": tmp.path().to_str().unwrap()}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Not a git repository"));
    }
}
