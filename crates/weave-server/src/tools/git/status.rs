//! `git_status` — get repository status (branch, staged, unstaged, untracked).

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{run_git, validate_git_cwd};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct GitStatusTool;

#[async_trait]
impl ToolExecutor for GitStatusTool {
    fn name(&self) -> &str {
        "git_status"
    }

    fn description(&self) -> &str {
        "Get git repository status: current branch, staged, unstaged, and untracked files. \
         Returns empty lists when clean."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "cwd": super::cwd_property()
            },
            "required": []
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let cwd = match validate_git_cwd(&input, ctx).await {
            Ok(p) => p,
            Err(e) => return e,
        };

        tracing::debug!(
            session_id = %ctx.session_id,
            cwd = %cwd.display(),
            "git_status"
        );

        let output = match run_git(&["status", "--porcelain=v1", "-b"], &cwd).await {
            Ok(o) => o,
            Err(e) => return e,
        };

        if output.exit_code != 0 {
            return super::error(format!("git status failed: {}", output.stderr.trim()));
        }

        let (branch, staged, unstaged, untracked) = parse_status(&output.stdout);

        super::success(json!({
            "branch": branch,
            "staged": staged,
            "unstaged": unstaged,
            "untracked": untracked
        }))
    }
}

/// Parse `git status --porcelain=v1 -b` output.
///
/// Returns (branch, staged, unstaged, untracked).
pub(crate) fn parse_status(stdout: &str) -> (String, Vec<String>, Vec<String>, Vec<String>) {
    let mut branch = String::new();
    let mut staged = Vec::new();
    let mut unstaged = Vec::new();
    let mut untracked = Vec::new();

    for line in stdout.lines() {
        if line.starts_with("## ") {
            // Header line: "## main...origin/main [ahead 1]"
            branch = line
                .strip_prefix("## ")
                .unwrap_or(line)
                .split("...")
                .next()
                .unwrap_or("")
                .trim()
                .to_string();
            continue;
        }

        if line.len() < 3 {
            continue;
        }

        let x = line.as_bytes()[0] as char;
        let y = line.as_bytes()[1] as char;
        let path = &line[3..];

        if x == '?' && y == '?' {
            untracked.push(path.to_string());
        } else {
            if x != ' ' && x != '?' {
                staged.push(path.to_string());
            }
            if y != ' ' && y != '?' {
                unstaged.push(path.to_string());
            }
        }
    }

    (branch, staged, unstaged, untracked)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::git::git_test_support::git_init;
    use crate::tools::test_support::make_context;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_git_status_clean() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Test", "test@test.com", true);
        let ctx = make_context(tmp.path());

        let result = GitStatusTool
            .execute(json!({"cwd": tmp.path().to_str().unwrap()}), &ctx)
            .await;

        assert!(result.success);
        assert_eq!(result.data["branch"], "main");
        assert!(result.data["staged"].as_array().unwrap().is_empty());
        assert!(result.data["unstaged"].as_array().unwrap().is_empty());
        assert!(result.data["untracked"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_git_status_untracked() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Test", "test@test.com", true);
        std::fs::write(tmp.path().join("new.txt"), "hello").unwrap();
        let ctx = make_context(tmp.path());

        let result = GitStatusTool
            .execute(json!({"cwd": tmp.path().to_str().unwrap()}), &ctx)
            .await;

        assert!(result.success);
        let untracked = result.data["untracked"].as_array().unwrap();
        assert_eq!(untracked.len(), 1);
        assert_eq!(untracked[0], "new.txt");
    }

    #[tokio::test]
    async fn test_git_status_staged() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Test", "test@test.com", true);
        std::fs::write(tmp.path().join("staged.txt"), "hello").unwrap();
        std::process::Command::new("git")
            .args(["add", "staged.txt"])
            .current_dir(tmp.path())
            .status()
            .unwrap();
        let ctx = make_context(tmp.path());

        let result = GitStatusTool
            .execute(json!({"cwd": tmp.path().to_str().unwrap()}), &ctx)
            .await;

        assert!(result.success);
        let staged = result.data["staged"].as_array().unwrap();
        assert_eq!(staged.len(), 1);
        assert_eq!(staged[0], "staged.txt");
    }

    #[tokio::test]
    async fn test_git_status_unstaged() {
        let tmp = TempDir::new().unwrap();
        git_init(tmp.path(), "Test", "test@test.com", true);
        std::fs::write(tmp.path().join("file.txt"), "initial").unwrap();
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
        std::fs::write(tmp.path().join("file.txt"), "modified").unwrap();
        let ctx = make_context(tmp.path());

        let result = GitStatusTool
            .execute(json!({"cwd": tmp.path().to_str().unwrap()}), &ctx)
            .await;

        assert!(result.success);
        let unstaged = result.data["unstaged"].as_array().unwrap();
        assert_eq!(unstaged.len(), 1);
        assert_eq!(unstaged[0], "file.txt");
    }

    #[tokio::test]
    async fn test_git_status_not_a_repo() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = GitStatusTool
            .execute(json!({"cwd": tmp.path().to_str().unwrap()}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Not a git repository"));
    }

    #[tokio::test]
    async fn test_git_status_cwd_validation() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = GitStatusTool
            .execute(json!({"cwd": "relative/path"}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("must be absolute"));
    }

    #[test]
    fn test_parse_status_format() {
        let stdout = "## main...origin/main\nM  staged.txt\n M unstaged.txt\n?? untracked.txt\n";
        let (branch, staged, unstaged, untracked) = parse_status(stdout);
        assert_eq!(branch, "main");
        assert_eq!(staged, vec!["staged.txt"]);
        assert_eq!(unstaged, vec!["unstaged.txt"]);
        assert_eq!(untracked, vec!["untracked.txt"]);
    }
}
