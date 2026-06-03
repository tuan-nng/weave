//! Filesystem tools for agent sessions.
//!
//! Provides five tools: `fs_read`, `fs_write`, `fs_edit`, `fs_search`, `fs_list`.
//! All write operations are contained within the session's `codebase_root`
//! and cannot touch control-plane paths.

mod edit;
mod list;
mod read;
mod search;
mod write;

pub use edit::FsEditTool;
pub use list::FsListTool;
pub use read::FsReadTool;
pub use search::FsSearchTool;
pub use write::FsWriteTool;

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::tools::ToolResult;

// ---------------------------------------------------------------------------
// Shared constants
// ---------------------------------------------------------------------------

/// Maximum directory recursion depth for `fs_list` and `fs_search`.
pub(crate) const MAX_DEPTH: usize = 10;

/// Maximum number of results returned by `fs_list` and `fs_search`.
pub(crate) const MAX_RESULTS: usize = 100;

// ---------------------------------------------------------------------------
// Control-plane protection
// ---------------------------------------------------------------------------

/// Directory prefixes (relative to codebase_root) that write ops cannot touch.
const CONTROL_PLANE_PREFIXES: &[&str] = &[
    ".git/",
    "resources/specialists/",
    "resources/migrations/",
    ".claude/",
];

/// Individual files (relative to codebase_root) that write ops cannot touch.
const CONTROL_PLANE_FILES: &[&str] = &[
    "weave.db",
    "Cargo.toml",
    "CLAUDE.md",
    "DECISIONS.md",
    "PROGRESS.md",
    "feature_list.json",
    "init.sh",
    "justfile",
];

// ---------------------------------------------------------------------------
// PathValidator
// ---------------------------------------------------------------------------

/// Validates paths for security constraints.
///
/// All methods return `Result<_, ToolResult>` so callers can propagate
/// errors directly without constructing their own `ToolResult`.
pub struct PathValidator;

impl PathValidator {
    /// Reject non-absolute paths.
    pub fn require_absolute(path: &str) -> Result<PathBuf, ToolResult> {
        let p = Path::new(path);
        if p.is_absolute() {
            Ok(p.to_path_buf())
        } else {
            Err(error(format!(
                "Path must be absolute, got '{}'. \
                 Use a full path starting with '/' (e.g. /mnt/data/works/weave/src/main.rs).",
                path
            )))
        }
    }

    /// Full write-path validation: absolute + within codebase_root + not control-plane.
    ///
    /// Resolves symlinks by canonicalizing the path (or its nearest existing
    /// ancestor for non-existent files) before checking containment. This
    /// prevents symlink-based escapes from the codebase root.
    pub fn validate_write_path(path: &Path, codebase_root: &Path) -> Result<PathBuf, ToolResult> {
        let canonical_root = codebase_root.canonicalize().map_err(|e| {
            error(format!(
                "Cannot resolve codebase root '{}': {}",
                codebase_root.display(),
                e
            ))
        })?;

        // Resolve symlinks. For non-existent files, walk up to the nearest
        // existing ancestor, canonicalize it, then re-append the remaining
        // components.
        let canonical_path = Self::resolve_path(path)?;

        if !canonical_path.starts_with(&canonical_root) {
            return Err(error(format!(
                "Path '{}' is outside the codebase root '{}'. \
                 Write operations are restricted to the project directory.",
                path.display(),
                canonical_root.display()
            )));
        }

        if Self::is_control_plane(&canonical_path, &canonical_root) {
            return Err(error(format!(
                "Path '{}' is a control-plane resource and cannot be modified. \
                 These files manage project configuration and state.",
                path.display()
            )));
        }

        Ok(canonical_path)
    }

    /// Resolve a path through the filesystem (following symlinks).
    ///
    /// For existing files, this is equivalent to `canonicalize`. For
    /// non-existent files, walks up to the nearest existing ancestor,
    /// canonicalizes it, then re-appends the remaining components.
    fn resolve_path(path: &Path) -> Result<PathBuf, ToolResult> {
        // Fast path: file exists, canonicalize directly.
        if let Ok(canonical) = path.canonicalize() {
            return Ok(canonical);
        }

        // File doesn't exist — find the nearest existing ancestor.
        let mut tail = Vec::new();
        let mut candidate = path;
        loop {
            match candidate.parent() {
                Some(parent) if parent.exists() => {
                    let canonical_parent = parent.canonicalize().map_err(|e| {
                        error(format!(
                            "Cannot resolve ancestor '{}': {}",
                            parent.display(),
                            e
                        ))
                    })?;
                    // Append remaining components: tail (collected from
                    // walking up) plus the current candidate's relative
                    // path from the found parent.
                    let mut result = tail
                        .iter()
                        .rev()
                        .fold(canonical_parent, |acc, c| acc.join(c));
                    if let Ok(remaining) = candidate.strip_prefix(parent) {
                        if !remaining.as_os_str().is_empty() {
                            result = result.join(remaining);
                        }
                    }
                    return Ok(result);
                }
                Some(parent) => {
                    // Parent doesn't exist — collect its name so we can
                    // re-append it after canonicalizing an ancestor.
                    if let Some(name) = parent.file_name() {
                        tail.push(name.to_os_string());
                    }
                    candidate = parent;
                }
                None => {
                    return Err(error(format!(
                        "Cannot resolve any ancestor of '{}'. \
                         Ensure the path exists or has a valid parent directory.",
                        path.display()
                    )));
                }
            }
        }
    }

    /// Check whether a canonical path is a control-plane resource.
    fn is_control_plane(path: &Path, root: &Path) -> bool {
        let relative = match path.strip_prefix(root) {
            Ok(r) => r,
            Err(_) => return false,
        };

        // Empty relative path means the root itself — not control plane.
        let rel_str = relative.to_string_lossy();
        if rel_str.is_empty() {
            return false;
        }

        for prefix in CONTROL_PLANE_PREFIXES {
            if rel_str.starts_with(prefix) {
                return true;
            }
        }

        for file in CONTROL_PLANE_FILES {
            if rel_str == *file || rel_str.starts_with(&format!("{}/", file)) {
                return true;
            }
        }

        false
    }

    /// Resolve `.` and `..` components without filesystem access.
    ///
    /// Unlike `canonicalize`, this works for paths that don't exist yet
    /// and does not follow symlinks. Used internally by `resolve_path`
    /// and exposed for testing.
    #[cfg(test)]
    fn normalize_path(path: &Path) -> PathBuf {
        let mut components = Vec::new();
        for component in path.components() {
            match component {
                std::path::Component::ParentDir => {
                    components.pop();
                }
                std::path::Component::CurDir => {}
                other => components.push(other),
            }
        }
        components.iter().collect()
    }
}

// ---------------------------------------------------------------------------
// Input helpers
// ---------------------------------------------------------------------------

/// Extract a required string field from the input JSON.
pub fn require_string(input: &Value, field: &str) -> Result<String, ToolResult> {
    input
        .get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| error(format!("Missing or invalid required field '{}'.", field)))
}

/// Extract an optional string field from the input JSON.
pub fn optional_string(input: &Value, field: &str) -> Option<String> {
    input
        .get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Extract an optional bool field (defaults to `false`).
pub fn optional_bool(input: &Value, field: &str) -> bool {
    input.get(field).and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Extract an optional u64 field from the input JSON.
pub fn optional_u64(input: &Value, field: &str) -> Option<u64> {
    input.get(field).and_then(|v| v.as_u64())
}

// ---------------------------------------------------------------------------
// Result helpers
// ---------------------------------------------------------------------------

/// Construct a success `ToolResult`.
pub fn success(data: Value) -> ToolResult {
    ToolResult {
        success: true,
        data,
        error: None,
    }
}

/// Construct an error `ToolResult` with a descriptive message.
pub fn error(message: impl Into<String>) -> ToolResult {
    ToolResult {
        success: false,
        data: Value::Null,
        error: Some(message.into()),
    }
}

/// Validate an optional task status. Returns `Err(ToolResult)` when the
/// status is non-empty and not in the canonical list. `Ok(())` for `None`
/// or a valid status. Delegates to the canonical `store::tasks::validate_status`.
pub fn check_optional_status(s: Option<&str>) -> Result<(), ToolResult> {
    if let Some(s) = s {
        crate::store::tasks::validate_status(s).map_err(|e| error(e.to_string()))?;
    }
    Ok(())
}

/// Max length for a card/task title. Mirrors the HTTP API's
/// `MAX_TASK_TITLE_LEN` in `api/kanban.rs:41`. Kept as a tool-layer
/// constant so the tool and the HTTP handler stay in sync; if the
/// value is changed, update both.
pub const MAX_TASK_TITLE_LEN: usize = 500;

/// Validate a user-supplied card title. Returns the canonical error
/// message shape (matching the HTTP handler at `api/kanban.rs::create_card`).
pub fn validate_task_title(raw: &str) -> Result<&str, ToolResult> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(error("task title must not be empty"));
    }
    if trimmed.chars().count() > MAX_TASK_TITLE_LEN {
        return Err(error(format!(
            "task title must be at most {MAX_TASK_TITLE_LEN} characters"
        )));
    }
    Ok(trimmed)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::test_support::make_context;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn make_root() -> (TempDir, PathBuf) {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        (tmp, root)
    }

    // --- PathValidator::require_absolute ---

    #[test]
    fn test_require_absolute_valid() {
        let result = PathValidator::require_absolute("/mnt/data/src/main.rs");
        assert!(result.is_ok());
    }

    #[test]
    fn test_require_absolute_rejects_relative() {
        let result = PathValidator::require_absolute("src/main.rs");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(!err.success);
        assert!(err.error.unwrap().contains("must be absolute"));
    }

    #[test]
    fn test_require_absolute_rejects_dot_relative() {
        let result = PathValidator::require_absolute("./src/main.rs");
        assert!(result.is_err());
    }

    // --- PathValidator::normalize_path ---

    #[test]
    fn test_normalize_removes_dot_dot() {
        let path = Path::new("/foo/bar/../baz");
        let normalized = PathValidator::normalize_path(path);
        assert_eq!(normalized, PathBuf::from("/foo/baz"));
    }

    #[test]
    fn test_normalize_removes_dot() {
        let path = Path::new("/foo/./bar");
        let normalized = PathValidator::normalize_path(path);
        assert_eq!(normalized, PathBuf::from("/foo/bar"));
    }

    #[test]
    fn test_normalize_complex_traversal() {
        let path = Path::new("/foo/bar/../../baz/qux/..");
        let normalized = PathValidator::normalize_path(path);
        assert_eq!(normalized, PathBuf::from("/baz"));
    }

    // --- PathValidator::validate_write_path ---

    #[test]
    fn test_validate_write_path_within_root() {
        let (_tmp, root) = make_root();
        let path = root.join("src/main.rs");
        let result = PathValidator::validate_write_path(&path, &root);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_write_path_escape_rejected() {
        let (_tmp, root) = make_root();
        let path = root.join("../../etc/passwd");
        let result = PathValidator::validate_write_path(&path, &root);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.error.unwrap().contains("outside the codebase root"));
    }

    #[test]
    fn test_validate_write_path_control_plane_git() {
        let (_tmp, root) = make_root();
        let path = root.join(".git/config");
        let result = PathValidator::validate_write_path(&path, &root);
        assert!(result.is_err());
        assert!(result.unwrap_err().error.unwrap().contains("control-plane"));
    }

    #[test]
    fn test_validate_write_path_control_plane_cargo_toml() {
        let (_tmp, root) = make_root();
        let path = root.join("Cargo.toml");
        let result = PathValidator::validate_write_path(&path, &root);
        assert!(result.is_err());
        assert!(result.unwrap_err().error.unwrap().contains("control-plane"));
    }

    #[test]
    fn test_validate_write_path_control_plane_migrations() {
        let (_tmp, root) = make_root();
        let path = root.join("resources/migrations/001.sql");
        let result = PathValidator::validate_write_path(&path, &root);
        assert!(result.is_err());
        assert!(result.unwrap_err().error.unwrap().contains("control-plane"));
    }

    #[test]
    fn test_validate_write_path_non_control_plane_in_subdir() {
        let (_tmp, root) = make_root();
        // src/Cargo.toml is NOT control plane — only root-level Cargo.toml is.
        let path = root.join("src/Cargo.toml");
        let result = PathValidator::validate_write_path(&path, &root);
        assert!(result.is_ok());
    }

    // --- Input helpers ---

    #[test]
    fn test_require_string_valid() {
        let input = serde_json::json!({"path": "/foo/bar"});
        assert_eq!(require_string(&input, "path").unwrap(), "/foo/bar");
    }

    #[test]
    fn test_require_string_missing() {
        let input = serde_json::json!({});
        assert!(require_string(&input, "path").is_err());
    }

    #[test]
    fn test_require_string_wrong_type() {
        let input = serde_json::json!({"path": 42});
        assert!(require_string(&input, "path").is_err());
    }

    #[test]
    fn test_optional_string_present() {
        let input = serde_json::json!({"glob": "*.rs"});
        assert_eq!(optional_string(&input, "glob"), Some("*.rs".to_string()));
    }

    #[test]
    fn test_optional_string_absent() {
        let input = serde_json::json!({});
        assert_eq!(optional_string(&input, "glob"), None);
    }

    #[test]
    fn test_optional_bool_default_false() {
        let input = serde_json::json!({});
        assert!(!optional_bool(&input, "recursive"));
    }

    #[test]
    fn test_optional_bool_true() {
        let input = serde_json::json!({"recursive": true});
        assert!(optional_bool(&input, "recursive"));
    }

    #[test]
    fn test_optional_u64_present() {
        let input = serde_json::json!({"timeout_ms": 5000});
        assert_eq!(optional_u64(&input, "timeout_ms"), Some(5000));
    }

    #[test]
    fn test_optional_u64_absent() {
        let input = serde_json::json!({});
        assert_eq!(optional_u64(&input, "timeout_ms"), None);
    }

    #[test]
    fn test_optional_u64_wrong_type() {
        let input = serde_json::json!({"timeout_ms": "not_a_number"});
        assert_eq!(optional_u64(&input, "timeout_ms"), None);
    }

    // --- Result helpers ---

    #[test]
    fn test_success_result() {
        let result = success(serde_json::json!({"bytes": 42}));
        assert!(result.success);
        assert_eq!(result.data["bytes"], 42);
        assert!(result.error.is_none());
    }

    #[test]
    fn test_error_result() {
        let result = error("something went wrong");
        assert!(!result.success);
        assert_eq!(result.error.unwrap(), "something went wrong");
    }

    // --- Verification tests (feature_list.json feat-013) ---

    #[tokio::test]
    async fn test_file_write_read() {
        use super::read::FsReadTool;
        use super::write::FsWriteTool;
        use crate::tools::ToolExecutor;

        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());
        let file = tmp.path().join("test_output.txt");

        // Write a file.
        let write_result = FsWriteTool
            .execute(
                serde_json::json!({"path": file.to_str().unwrap(), "content": "hello weave"}),
                &ctx,
            )
            .await;
        assert!(write_result.success);
        assert_eq!(write_result.data["bytes_written"], 11);

        // Read it back.
        let read_result = FsReadTool
            .execute(serde_json::json!({"path": file.to_str().unwrap()}), &ctx)
            .await;
        assert!(read_result.success);
        assert_eq!(read_result.data["content"], "hello weave");
        assert_eq!(read_result.data["bytes_read"], 11);
    }

    #[tokio::test]
    async fn test_file_path_containment() {
        use super::write::FsWriteTool;
        use crate::tools::ToolExecutor;

        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        // Attempt to escape via `..` traversal.
        let escape_path = tmp.path().join("../../etc/evil.txt");
        let result = FsWriteTool
            .execute(
                serde_json::json!({"path": escape_path.to_str().unwrap(), "content": "pwned"}),
                &ctx,
            )
            .await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("outside the codebase root"));
    }

    #[tokio::test]
    async fn test_control_plane_protection() {
        use super::edit::FsEditTool;
        use super::write::FsWriteTool;
        use crate::tools::ToolExecutor;
        use std::fs;

        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        // Create a control-plane file to attempt to modify.
        let cargo_toml = tmp.path().join("Cargo.toml");
        fs::write(&cargo_toml, "[package]\nname = \"test\"").unwrap();

        // Attempt to overwrite via fs_write.
        let write_result = FsWriteTool
            .execute(
                serde_json::json!({"path": cargo_toml.to_str().unwrap(), "content": "[package]\nname = \"pwned\""}),
                &ctx,
            )
            .await;
        assert!(!write_result.success);
        assert!(write_result.error.unwrap().contains("control-plane"));

        // Attempt to edit via fs_edit.
        let edit_result = FsEditTool
            .execute(
                serde_json::json!({
                    "path": cargo_toml.to_str().unwrap(),
                    "old_string": "[package]",
                    "new_string": "[package]\nversion = \"0.0.0\""
                }),
                &ctx,
            )
            .await;
        assert!(!edit_result.success);
        assert!(edit_result.error.unwrap().contains("control-plane"));

        // Verify the file was NOT modified.
        let content = fs::read_to_string(&cargo_toml).unwrap();
        assert_eq!(content, "[package]\nname = \"test\"");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_symlink_escape_blocked() {
        use super::write::FsWriteTool;
        use crate::tools::ToolExecutor;
        use std::os::unix::fs::symlink;

        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        // Create a target directory outside the codebase root.
        let outside = TempDir::new().unwrap();
        let outside_file = outside.path().join("evil.txt");

        // Create a symlink inside the codebase root pointing outside.
        let link = tmp.path().join("escape_link");
        symlink(outside.path(), &link).unwrap();

        // Attempt to write through the symlink.
        let symlink_path = link.join("evil.txt");
        let result = FsWriteTool
            .execute(
                serde_json::json!({"path": symlink_path.to_str().unwrap(), "content": "pwned"}),
                &ctx,
            )
            .await;

        // Should be rejected because the resolved path is outside the root.
        assert!(!result.success);
        assert!(result.error.unwrap().contains("outside the codebase root"));

        // Verify the outside file was NOT created.
        assert!(!outside_file.exists());
    }
}
