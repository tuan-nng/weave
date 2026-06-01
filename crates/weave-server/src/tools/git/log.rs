//! `git_log` — get recent commits with hash and message.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{run_git, validate_git_cwd};
use crate::tools::fs::optional_u64;
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

/// Default number of commits to return.
const DEFAULT_LIMIT: u64 = 10;

pub struct GitLogTool;

#[async_trait]
impl ToolExecutor for GitLogTool {
    fn name(&self) -> &str {
        "git_log"
    }

    fn description(&self) -> &str {
        "Get recent git log with commit hashes and messages."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cwd": super::cwd_property(),
                "limit": {
                    "type": "integer",
                    "description": "Number of commits to return. Defaults to 10."
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

        let limit = optional_u64(&input, "limit").unwrap_or(DEFAULT_LIMIT);
        let limit_str = limit.to_string();

        tracing::debug!(
            session_id = %ctx.session_id,
            cwd = %cwd.display(),
            limit = limit,
            "git_log"
        );

        let output =
            match run_git(&["log", "--oneline", "-n", &limit_str, "--no-color"], &cwd).await {
                Ok(o) => o,
                Err(e) => return e,
            };

        if output.exit_code != 0 {
            // Empty repo (no commits) returns exit code 128 with "bad default revision"
            // but that's not an error — just return empty list.
            if output.stderr.contains("does not have any commits")
                || output.stderr.contains("bad default revision")
            {
                return super::success(json!({"commits": []}));
            }
            return super::error(format!("git log failed: {}", output.stderr.trim()));
        }

        let commits = parse_log(&output.stdout);

        super::success(json!({"commits": commits}))
    }
}

/// Parse `git log --oneline` output into (hash, message) pairs.
fn parse_log(stdout: &str) -> Vec<Value> {
    stdout
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            // Format: "abc1234 commit message here"
            let (hash, message) = match line.find(' ') {
                Some(pos) => (&line[..pos], &line[pos + 1..]),
                None => (line, ""),
            };
            json!({
                "hash": hash,
                "message": message
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::git::git_test_support::git_init;
    use crate::tools::test_support::make_context;
    use tempfile::TempDir;

    /// Helper: create a commit with a message.
    fn git_commit(dir: &std::path::Path, message: &str) {
        std::process::Command::new("git")
            .args(["commit", "--allow-empty", "-m", message])
            .current_dir(dir)
            .status()
            .unwrap();
    }

    #[tokio::test]
    async fn test_git_log_empty() {
        let tmp = TempDir::new().unwrap();
        // Init with no commits — git log will fail with exit 128.
        // But we also need identity for git init to work.
        git_init(tmp.path(), "Test", "test@test.com", false);
        let ctx = make_context(tmp.path());

        let result = GitLogTool
            .execute(json!({"cwd": tmp.path().to_str().unwrap()}), &ctx)
            .await;

        // Should succeed with empty commits list.
        assert!(result.success, "git_log on empty repo should succeed");
        let commits = result.data["commits"].as_array().unwrap();
        assert!(
            commits.is_empty(),
            "git_log on empty repo should return empty commits"
        );
    }

    #[tokio::test]
    async fn test_git_log_default() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Test", "test@test.com", false);
        git_commit(tmp.path(), "first commit");
        git_commit(tmp.path(), "second commit");
        git_commit(tmp.path(), "third commit");
        let ctx = make_context(tmp.path());

        let result = GitLogTool
            .execute(json!({"cwd": tmp.path().to_str().unwrap()}), &ctx)
            .await;

        assert!(result.success);
        let commits = result.data["commits"].as_array().unwrap();
        assert_eq!(commits.len(), 3);
        assert_eq!(commits[0]["message"], "third commit");
        assert_eq!(commits[1]["message"], "second commit");
        assert_eq!(commits[2]["message"], "first commit");
        // Hashes should be 7+ chars.
        for c in commits {
            assert!(c["hash"].as_str().unwrap().len() >= 7);
        }
    }

    #[tokio::test]
    async fn test_git_log_limit() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Test", "test@test.com", false);
        for i in 1..=5 {
            git_commit(tmp.path(), &format!("commit {}", i));
        }
        let ctx = make_context(tmp.path());

        let result = GitLogTool
            .execute(
                json!({"cwd": tmp.path().to_str().unwrap(), "limit": 2}),
                &ctx,
            )
            .await;

        assert!(result.success);
        let commits = result.data["commits"].as_array().unwrap();
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0]["message"], "commit 5");
        assert_eq!(commits[1]["message"], "commit 4");
    }

    #[tokio::test]
    async fn test_git_log_not_a_repo() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = GitLogTool
            .execute(json!({"cwd": tmp.path().to_str().unwrap()}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Not a git repository"));
    }

    #[test]
    fn test_parse_log_format() {
        let stdout = "abc1234 first commit\ndef5678 second commit\n";
        let commits = parse_log(stdout);
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0]["hash"], "abc1234");
        assert_eq!(commits[0]["message"], "first commit");
        assert_eq!(commits[1]["hash"], "def5678");
        assert_eq!(commits[1]["message"], "second commit");
    }

    #[test]
    fn test_parse_log_empty() {
        let commits = parse_log("");
        assert!(commits.is_empty());
    }
}
