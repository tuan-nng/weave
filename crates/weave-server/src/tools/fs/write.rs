//! `fs_write` — write/overwrite file content.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{error, require_string, success, PathValidator};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct FsWriteTool;

#[async_trait]
impl ToolExecutor for FsWriteTool {
    fn name(&self) -> &str {
        "fs_write"
    }

    fn description(&self) -> &str {
        "Write content to a file, creating parent directories. \
         Path must be absolute and within the codebase root."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file to write"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let path_str = match require_string(&input, "path") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let content = match require_string(&input, "content") {
            Ok(s) => s,
            Err(e) => return e,
        };

        let path = match PathValidator::require_absolute(&path_str) {
            Ok(p) => p,
            Err(e) => return e,
        };

        if let Err(e) = PathValidator::validate_write_path(&path, &ctx.codebase_root) {
            return e;
        }

        // Create parent directories if they don't exist.
        if let Some(parent) = path.parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                return error(format!("Failed to create parent directories: {}.", e));
            }
        }

        match std::fs::write(&path, &content) {
            Ok(()) => success(json!({
                "bytes_written": content.len(),
                "path": path_str
            })),
            Err(e) => error(format!("Failed to write '{}': {}.", path.display(), e)),
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
    async fn test_fs_write_creates_file() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let file = tmp.path().join("output.txt");

        let result = FsWriteTool
            .execute(
                json!({"path": file.to_str().unwrap(), "content": "hello"}),
                &ctx,
            )
            .await;

        assert!(result.success);
        assert_eq!(result.data["bytes_written"], 5);
        assert_eq!(fs::read_to_string(&file).unwrap(), "hello");
    }

    #[tokio::test]
    async fn test_fs_write_creates_parent_dirs() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let file = tmp.path().join("deep/nested/dir/file.txt");

        let result = FsWriteTool
            .execute(
                json!({"path": file.to_str().unwrap(), "content": "deep"}),
                &ctx,
            )
            .await;

        assert!(result.success);
        assert_eq!(fs::read_to_string(&file).unwrap(), "deep");
    }

    #[tokio::test]
    async fn test_fs_write_overwrites_existing() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("existing.txt");
        fs::write(&file, "old content").unwrap();

        let ctx = make_context(tmp.path());
        let result = FsWriteTool
            .execute(
                json!({"path": file.to_str().unwrap(), "content": "new content"}),
                &ctx,
            )
            .await;

        assert!(result.success);
        assert_eq!(fs::read_to_string(&file).unwrap(), "new content");
    }

    #[tokio::test]
    async fn test_fs_write_containment_blocks_escape() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = FsWriteTool
            .execute(json!({"path": "/tmp/evil.txt", "content": "pwned"}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("outside the codebase root"));
    }

    #[tokio::test]
    async fn test_fs_write_control_plane_rejected() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let file = tmp.path().join("Cargo.toml");

        let result = FsWriteTool
            .execute(
                json!({"path": file.to_str().unwrap(), "content": "[package]"}),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("control-plane"));
    }

    #[tokio::test]
    async fn test_fs_write_relative_path_rejected() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = FsWriteTool
            .execute(json!({"path": "relative.txt", "content": "data"}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("must be absolute"));
    }
}
