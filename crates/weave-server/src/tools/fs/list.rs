//! `fs_list` — list directory entries.

use std::path::Path;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{error, optional_bool, require_string, success, PathValidator, MAX_DEPTH, MAX_RESULTS};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct FsListTool;

#[async_trait]
impl ToolExecutor for FsListTool {
    fn name(&self) -> &str {
        "fs_list"
    }

    fn description(&self) -> &str {
        "List directory entries. Path must be absolute."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path to the directory to list"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "Whether to list subdirectories recursively (default: false)"
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
        let recursive = optional_bool(&input, "recursive");

        let path = match PathValidator::require_absolute(&path_str) {
            Ok(p) => p,
            Err(e) => return e,
        };

        // Type check first so a non-existent directory still gets the
        // "not a directory" error rather than a containment error.
        if !path.is_dir() {
            return error(format!(
                "Path '{}' is not a directory. fs_list only works on directories.",
                path.display()
            ));
        }

        // Bound sessions enforce containment. Unbound stay permissive.
        let canonical_path = match PathValidator::validate_read_path(&path, &ctx.codebase_root) {
            Ok(p) => p,
            Err(e) => return e,
        };

        let mut entries = Vec::new();
        walk_directory(&canonical_path, recursive, 0, &mut entries);

        let total = entries.len();
        success(json!({
            "entries": entries,
            "total": total
        }))
    }
}

/// Recursively walk a directory and collect entries.
///
/// Skips symlinks entirely. This is the load-bearing piece of the
/// "bound session = sandbox" guarantee: a model cannot `ln -s /etc
/// <repo>/etc_link` and then call `fs_list`/`fs_search` to read the
/// target. We use `entry.file_type()` (does NOT follow symlinks) and
/// filter on `is_symlink()`, rather than the follow-symlink
/// `path.is_dir()` / `path.is_file()`. The tradeoff: legitimate
/// symlinks inside a repo (e.g. `node_modules/.bin` shims) are also
/// skipped. The contract is "this walker only reports what is
/// lexically in the tree, not what symlinks dereference to".
fn walk_directory(dir: &Path, recursive: bool, depth: usize, entries: &mut Vec<Value>) {
    if depth >= MAX_DEPTH || entries.len() >= MAX_RESULTS {
        return;
    }

    let read_entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in read_entries.flatten() {
        if entries.len() >= MAX_RESULTS {
            return;
        }

        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        // Skip symlinks. They can point anywhere on the host, so
        // following them would let the model escape the codebase
        // sandbox.
        if file_type.is_symlink() {
            continue;
        }

        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = file_type.is_dir();

        // Skip hidden directories (starting with '.').
        if is_dir && name.starts_with('.') {
            continue;
        }

        entries.push(json!({
            "name": name,
            "path": path.to_string_lossy(),
            "is_dir": is_dir
        }));

        if recursive && is_dir {
            walk_directory(&path, true, depth + 1, entries);
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
    async fn test_fs_list_non_recursive() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("file.txt"), "content").unwrap();
        fs::create_dir(tmp.path().join("subdir")).unwrap();

        let ctx = make_context(tmp.path());
        let result = FsListTool
            .execute(json!({"path": tmp.path().to_str().unwrap()}), &ctx)
            .await;

        assert!(result.success);
        let entries = result.data["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(result.data["total"], 2);

        // Find the file entry.
        let file_entry = entries.iter().find(|e| e["name"] == "file.txt").unwrap();
        assert!(!file_entry["is_dir"].as_bool().unwrap());

        // Find the directory entry.
        let dir_entry = entries.iter().find(|e| e["name"] == "subdir").unwrap();
        assert!(dir_entry["is_dir"].as_bool().unwrap());
    }

    #[tokio::test]
    async fn test_fs_list_recursive() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("top.txt"), "top").unwrap();
        let sub = tmp.path().join("sub");
        fs::create_dir(&sub).unwrap();
        fs::write(sub.join("nested.txt"), "nested").unwrap();

        let ctx = make_context(tmp.path());
        let result = FsListTool
            .execute(
                json!({"path": tmp.path().to_str().unwrap(), "recursive": true}),
                &ctx,
            )
            .await;

        assert!(result.success);
        let entries = result.data["entries"].as_array().unwrap();
        // top.txt + sub/ + sub/nested.txt = 3
        assert_eq!(entries.len(), 3);
    }

    #[tokio::test]
    async fn test_fs_list_empty_directory() {
        let tmp = TempDir::new().unwrap();
        let empty = tmp.path().join("empty");
        fs::create_dir(&empty).unwrap();

        let ctx = make_context(tmp.path());
        let result = FsListTool
            .execute(json!({"path": empty.to_str().unwrap()}), &ctx)
            .await;

        assert!(result.success);
        assert_eq!(result.data["total"], 0);
    }

    #[tokio::test]
    async fn test_fs_list_nonexistent_directory() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = FsListTool
            .execute(json!({"path": "/nonexistent/dir"}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("not a directory"));
    }

    #[tokio::test]
    async fn test_fs_list_relative_path_rejected() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = FsListTool
            .execute(json!({"path": "relative/dir"}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("must be absolute"));
    }
}
