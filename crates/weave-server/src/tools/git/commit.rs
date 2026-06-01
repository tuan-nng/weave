//! `git_commit` — create a git commit with identity validation.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{run_git, validate_commit_identity, validate_git_cwd};
use crate::tools::fs::{optional_bool, require_string};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct GitCommitTool;

#[async_trait]
impl ToolExecutor for GitCommitTool {
    fn name(&self) -> &str {
        "git_commit"
    }

    fn description(&self) -> &str {
        "Create a git commit. Validates that git user.name and user.email are \
         configured with real values (not placeholders)."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cwd": super::cwd_property(),
                "message": {
                    "type": "string",
                    "description": "Commit message."
                },
                "stage_all": {
                    "type": "boolean",
                    "description": "Stage all changes before committing (git add -A). Defaults to false."
                }
            },
            "required": ["message"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let cwd = match validate_git_cwd(&input, ctx).await {
            Ok(p) => p,
            Err(e) => return e,
        };

        let message = match require_string(&input, "message") {
            Ok(s) => s,
            Err(e) => return e,
        };

        if message.trim().is_empty() {
            return super::error(
                "Commit message cannot be empty. Provide a meaningful commit message.",
            );
        }

        let stage_all = optional_bool(&input, "stage_all");

        tracing::debug!(
            session_id = %ctx.session_id,
            cwd = %cwd.display(),
            stage_all = stage_all,
            "git_commit"
        );

        // Validate git identity (effective config — local, global, or system).
        let name_output = match run_git(&["config", "user.name"], &cwd).await {
            Ok(o) => o,
            Err(e) => return e,
        };
        let email_output = match run_git(&["config", "user.email"], &cwd).await {
            Ok(o) => o,
            Err(e) => return e,
        };

        let name = name_output.stdout.trim();
        let email = email_output.stdout.trim();

        if let Err(e) = validate_commit_identity(name, email) {
            return e;
        }

        // Stage all if requested.
        if stage_all {
            let add_output = match run_git(&["add", "-A"], &cwd).await {
                Ok(o) => o,
                Err(e) => return e,
            };
            if add_output.exit_code != 0 {
                return super::error(format!("git add failed: {}", add_output.stderr.trim()));
            }
        }

        // Commit.
        let commit_output = match run_git(&["commit", "-m", &message], &cwd).await {
            Ok(o) => o,
            Err(e) => return e,
        };

        if commit_output.exit_code != 0 {
            return super::error(format!(
                "git commit failed: {}",
                commit_output.stderr.trim()
            ));
        }

        // Parse commit hash from output: "[branch abc1234] message"
        let hash = parse_commit_hash(&commit_output.stdout);

        super::success(json!({
            "hash": hash,
            "message": message
        }))
    }
}

/// Parse commit hash from `git commit` output.
///
/// Output format: "[branch abc1234] commit message"
/// Returns the hash portion, or the full output if parsing fails.
fn parse_commit_hash(stdout: &str) -> String {
    // Look for pattern: "[branch hash]"
    if let Some(bracket_end) = stdout.find(']') {
        let inner = &stdout[1..bracket_end];
        // The hash is the last space-separated token before "]".
        if let Some(space_pos) = inner.rfind(' ') {
            return inner[space_pos + 1..].to_string();
        }
    }
    // Fallback: return first 7 chars of stdout (likely the hash).
    stdout.chars().take(7).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::git::git_test_support::git_init;
    use crate::tools::test_support::make_context;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_git_commit_basic() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Alice Smith", "alice@company.com", false);
        std::fs::write(tmp.path().join("file.txt"), "hello").unwrap();
        std::process::Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        let ctx = make_context(tmp.path());

        let result = GitCommitTool
            .execute(
                json!({"cwd": tmp.path().to_str().unwrap(), "message": "initial commit"}),
                &ctx,
            )
            .await;

        assert!(result.success);
        let hash = result.data["hash"].as_str().unwrap();
        assert!(hash.len() >= 7);
        assert_eq!(result.data["message"], "initial commit");
    }

    #[tokio::test]
    async fn test_git_commit_stage_all() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Alice Smith", "alice@company.com", false);
        std::fs::write(tmp.path().join("file.txt"), "hello").unwrap();
        let ctx = make_context(tmp.path());

        let result = GitCommitTool
            .execute(
                json!({
                    "cwd": tmp.path().to_str().unwrap(),
                    "message": "add file",
                    "stage_all": true
                }),
                &ctx,
            )
            .await;

        assert!(result.success);
        let hash = result.data["hash"].as_str().unwrap();
        assert!(hash.len() >= 7);
    }

    #[tokio::test]
    async fn test_git_commit_message_required() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Alice Smith", "alice@company.com", false);
        let ctx = make_context(tmp.path());

        let result = GitCommitTool
            .execute(json!({"cwd": tmp.path().to_str().unwrap()}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Missing"));
    }

    #[tokio::test]
    async fn test_git_commit_empty_message() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Alice Smith", "alice@company.com", false);
        std::fs::write(tmp.path().join("file.txt"), "hello").unwrap();
        std::process::Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        let ctx = make_context(tmp.path());

        let result = GitCommitTool
            .execute(
                json!({"cwd": tmp.path().to_str().unwrap(), "message": "  "}),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("cannot be empty"));
    }

    #[tokio::test]
    async fn test_git_commit_rejects_placeholder_name() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "test", "test@test.com", false);
        std::fs::write(tmp.path().join("file.txt"), "hello").unwrap();
        std::process::Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        let ctx = make_context(tmp.path());

        let result = GitCommitTool
            .execute(
                json!({"cwd": tmp.path().to_str().unwrap(), "message": "test"}),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("placeholder"));
    }

    #[tokio::test]
    async fn test_git_commit_rejects_placeholder_email() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Alice", "user@example.com", false);
        std::fs::write(tmp.path().join("file.txt"), "hello").unwrap();
        std::process::Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        let ctx = make_context(tmp.path());

        let result = GitCommitTool
            .execute(
                json!({"cwd": tmp.path().to_str().unwrap(), "message": "test"}),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("placeholder domain"));
    }

    #[tokio::test]
    async fn test_git_commit_rejects_name_equals_email() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "same@value.com", "same@value.com", false);
        std::fs::write(tmp.path().join("file.txt"), "hello").unwrap();
        std::process::Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        let ctx = make_context(tmp.path());

        let result = GitCommitTool
            .execute(
                json!({"cwd": tmp.path().to_str().unwrap(), "message": "test"}),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("same"));
    }

    #[tokio::test]
    async fn test_git_commit_rejects_empty_identity() {
        let tmp = TempDir::new().unwrap();
        // Init without setting user.name/user.email.
        std::process::Command::new("git")
            .args(["init", "-b", "main"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        std::fs::write(tmp.path().join("file.txt"), "hello").unwrap();
        std::process::Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        let ctx = make_context(tmp.path());

        let result = GitCommitTool
            .execute(
                json!({"cwd": tmp.path().to_str().unwrap(), "message": "test"}),
                &ctx,
            )
            .await;

        assert!(!result.success);
        let err = result.error.unwrap();
        // Should fail on identity validation (empty name or email).
        assert!(err.contains("not configured") || err.contains("placeholder"));
    }

    #[tokio::test]
    async fn test_git_commit_not_a_repo() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = GitCommitTool
            .execute(
                json!({"cwd": tmp.path().to_str().unwrap(), "message": "test"}),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Not a git repository"));
    }

    #[tokio::test]
    async fn test_git_commit_validation() {
        // Valid identity — should succeed.
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Alice Smith", "alice@company.com", false);
        std::fs::write(tmp.path().join("file.txt"), "hello").unwrap();
        std::process::Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        let ctx = make_context(tmp.path());

        let result = GitCommitTool
            .execute(
                json!({"cwd": tmp.path().to_str().unwrap(), "message": "valid commit"}),
                &ctx,
            )
            .await;

        assert!(result.success);
        assert!(result.data["hash"].as_str().unwrap().len() >= 7);

        // Placeholder identity — should fail.
        let tmp2 = TempDir::new().unwrap();
        git_init(tmp2.path(), "test", "test@test.com", false);
        std::fs::write(tmp2.path().join("file.txt"), "hello").unwrap();
        std::process::Command::new("git")
            .args(["add", "file.txt"])
            .current_dir(tmp2.path())
            .status()
            .unwrap();
        let ctx2 = make_context(tmp2.path());

        let result2 = GitCommitTool
            .execute(
                json!({"cwd": tmp2.path().to_str().unwrap(), "message": "should fail"}),
                &ctx2,
            )
            .await;

        assert!(!result2.success);
        assert!(result2.error.unwrap().contains("placeholder"));
    }

    #[test]
    fn test_parse_commit_hash() {
        let stdout = "[main abc1234] initial commit\n 1 file changed, 1 insertion(+)\n";
        let hash = parse_commit_hash(stdout);
        assert_eq!(hash, "abc1234");
    }

    #[test]
    fn test_parse_commit_hash_fallback() {
        let hash = parse_commit_hash("unexpected format");
        assert_eq!(hash.len(), 7);
    }
}
