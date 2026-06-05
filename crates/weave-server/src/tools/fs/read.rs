//! `fs_read` — read file content.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{error, require_string, success, PathValidator};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct FsReadTool;

#[async_trait]
impl ToolExecutor for FsReadTool {
    fn name(&self) -> &str {
        "fs_read"
    }

    fn description(&self) -> &str {
        "Read the contents of a file. Path must be absolute."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file to read"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let path_str = match require_string(&input, "path") {
            Ok(s) => s,
            Err(e) => return e,
        };

        let path = match PathValidator::require_absolute(&path_str) {
            Ok(p) => p,
            Err(e) => return e,
        };

        // Existence/type checks happen on the raw path BEFORE
        // containment, so non-existent files still produce a clear
        // "Failed to read" error rather than a containment error.
        if !path.exists() {
            return error(format!(
                "Failed to read '{}': file does not exist.",
                path.display()
            ));
        }
        if !path.is_file() {
            return error(format!(
                "Failed to read '{}': not a regular file. \
                 fs_read only works on files, not directories.",
                path.display()
            ));
        }

        // Bound sessions enforce containment: reads must stay inside
        // the registered codebase root. Unbound sessions (codebase_root
        // == ".") keep legacy permissive behavior.
        let canonical_path = match PathValidator::validate_read_path(&path, &ctx.codebase_root) {
            Ok(p) => p,
            Err(e) => return e,
        };

        match std::fs::read_to_string(&canonical_path) {
            Ok(content) => {
                let bytes_read = content.len();
                success(json!({
                    "content": content,
                    "bytes_read": bytes_read
                }))
            }
            Err(e) => error(format!(
                "Failed to read '{}': {}. \
                 Check that the file exists and is readable.",
                path.display(),
                e
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::make_context;
    use std::fs;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_fs_read_existing_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("hello.txt");
        fs::write(&file, "hello world").unwrap();

        let ctx = make_context(tmp.path());
        let result = FsReadTool
            .execute(json!({"path": file.to_str().unwrap()}), &ctx)
            .await;

        assert!(result.success);
        assert_eq!(result.data["content"], "hello world");
        assert_eq!(result.data["bytes_read"], 11);
    }

    #[tokio::test]
    async fn test_fs_read_nonexistent_file() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = FsReadTool
            .execute(json!({"path": "/nonexistent/file.txt"}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Failed to read"));
    }

    #[tokio::test]
    async fn test_fs_read_relative_path_rejected() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = FsReadTool
            .execute(json!({"path": "relative/path.txt"}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("must be absolute"));
    }

    #[tokio::test]
    async fn test_fs_read_missing_path_field() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = FsReadTool.execute(json!({}), &ctx).await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Missing"));
    }
}
