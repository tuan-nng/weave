//! `fs_edit` — search-and-replace in a file.

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{error, require_string, success, PathValidator};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct FsEditTool;

#[async_trait]
impl ToolExecutor for FsEditTool {
    fn name(&self) -> &str {
        "fs_edit"
    }

    fn description(&self) -> &str {
        "Replace a specific string in a file. The old_string must match exactly once."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the file to edit"
                },
                "old_string": {
                    "type": "string",
                    "description": "The exact string to find and replace (must match exactly once)"
                },
                "new_string": {
                    "type": "string",
                    "description": "The replacement string"
                }
            },
            "required": ["path", "old_string", "new_string"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let path_str = match require_string(&input, "path") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let old_string = match require_string(&input, "old_string") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let new_string = match require_string(&input, "new_string") {
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

        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(e) => {
                return error(format!(
                    "Failed to read '{}': {}. Check that the file exists.",
                    path.display(),
                    e
                ));
            }
        };

        let count = content.matches(&old_string).count();
        if count == 0 {
            return error(format!(
                "old_string not found in '{}'. Ensure the string matches exactly, \
                 including whitespace and indentation.",
                path.display()
            ));
        }
        if count > 1 {
            return error(format!(
                "old_string matches {} times in '{}'; expected exactly 1 match. \
                 Provide more surrounding context to make the match unique.",
                count,
                path.display()
            ));
        }

        // Exactly one match — safe to replace.
        let new_content = content.replace(&old_string, &new_string);

        match std::fs::write(&path, &new_content) {
            Ok(()) => success(json!({ "path": path_str })),
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
    async fn test_fs_edit_single_match() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("code.rs");
        fs::write(&file, "fn main() {\n    println!(\"hello\");\n}").unwrap();

        let ctx = make_context(tmp.path());
        let result = FsEditTool
            .execute(
                json!({
                    "path": file.to_str().unwrap(),
                    "old_string": "println!(\"hello\")",
                    "new_string": "println!(\"world\")"
                }),
                &ctx,
            )
            .await;

        assert!(result.success);
        let content = fs::read_to_string(&file).unwrap();
        assert!(content.contains("println!(\"world\")"));
        assert!(!content.contains("println!(\"hello\")"));
    }

    #[tokio::test]
    async fn test_fs_edit_zero_matches() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("code.rs");
        fs::write(&file, "fn main() {}").unwrap();

        let ctx = make_context(tmp.path());
        let result = FsEditTool
            .execute(
                json!({
                    "path": file.to_str().unwrap(),
                    "old_string": "nonexistent",
                    "new_string": "replacement"
                }),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_fs_edit_multiple_matches() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("code.rs");
        fs::write(&file, "foo + foo").unwrap();

        let ctx = make_context(tmp.path());
        let result = FsEditTool
            .execute(
                json!({
                    "path": file.to_str().unwrap(),
                    "old_string": "foo",
                    "new_string": "bar"
                }),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("matches 2 times"));
    }

    #[tokio::test]
    async fn test_fs_edit_control_plane_rejected() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("Cargo.toml");
        fs::write(&file, "[package]").unwrap();

        let ctx = make_context(tmp.path());
        let result = FsEditTool
            .execute(
                json!({
                    "path": file.to_str().unwrap(),
                    "old_string": "[package]",
                    "new_string": "[package]\nname = \"test\""
                }),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("control-plane"));
    }

    #[tokio::test]
    async fn test_fs_edit_nonexistent_file() {
        let tmp = TempDir::new().unwrap();
        let file = tmp.path().join("nonexistent.txt");
        let ctx = make_context(tmp.path());

        let result = FsEditTool
            .execute(
                json!({
                    "path": file.to_str().unwrap(),
                    "old_string": "old",
                    "new_string": "new"
                }),
                &ctx,
            )
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Failed to read"));
    }
}
