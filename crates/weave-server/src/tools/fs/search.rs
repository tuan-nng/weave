//! `fs_search` — regex search across files with optional glob filter.

use std::path::Path;

use async_trait::async_trait;
use serde_json::{json, Value};

use super::{
    error, optional_string, require_string, success, PathValidator, MAX_DEPTH, MAX_RESULTS,
};
use crate::tools::{ToolContext, ToolExecutor, ToolResult};

pub struct FsSearchTool;

#[async_trait]
impl ToolExecutor for FsSearchTool {
    fn name(&self) -> &str {
        "fs_search"
    }

    fn description(&self) -> &str {
        "Search for a regex pattern in files. Optional glob filter and search path."
    }

    fn input_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for"
                },
                "glob": {
                    "type": "string",
                    "description": "Optional glob filter (e.g. '*.rs')"
                },
                "path": {
                    "type": "string",
                    "description": "Absolute directory to search in (defaults to codebase root)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, input: Value, ctx: &ToolContext) -> ToolResult {
        let pattern = match require_string(&input, "pattern") {
            Ok(s) => s,
            Err(e) => return e,
        };
        let glob_filter = optional_string(&input, "glob");
        let search_path_str = optional_string(&input, "path");

        let search_path = match search_path_str {
            Some(ref p) => {
                let raw = match PathValidator::require_absolute(p) {
                    Ok(pb) => pb,
                    Err(e) => return e,
                };
                // Bound sessions enforce containment on explicit search
                // paths. Unbound stay permissive.
                match PathValidator::validate_read_path(&raw, &ctx.codebase_root) {
                    Ok(pb) => pb,
                    Err(e) => return e,
                }
            }
            // Default search root: already the codebase_root. When
            // unbound (`codebase_root == "."`), this walks from the
            // server's CWD — matching pre-binding behavior.
            None => ctx.codebase_root.clone(),
        };

        let regex = match regex::RegexBuilder::new(&pattern)
            .size_limit(1 << 20) // 1MB compiled regex size limit
            .build()
        {
            Ok(r) => r,
            Err(e) => {
                return error(format!(
                    "Invalid regex pattern '{}': {}. Use a valid regular expression.",
                    pattern, e
                ));
            }
        };

        let glob_pattern = match glob_filter {
            Some(ref g) => match glob::Pattern::new(g) {
                Ok(p) => Some(p),
                Err(e) => {
                    return error(format!(
                        "Invalid glob pattern '{}': {}. Use a valid glob (e.g. '*.rs').",
                        g, e
                    ));
                }
            },
            None => None,
        };

        let mut results = Vec::new();
        walk_and_search(&search_path, &regex, glob_pattern.as_ref(), 0, &mut results);

        let total = results.len();
        success(json!({
            "results": results,
            "total_matches": total
        }))
    }
}

/// Recursively walk directories and search files for regex matches.
fn walk_and_search(
    dir: &Path,
    regex: &regex::Regex,
    glob: Option<&glob::Pattern>,
    depth: usize,
    results: &mut Vec<Value>,
) {
    if depth >= MAX_DEPTH || results.len() >= MAX_RESULTS {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        if results.len() >= MAX_RESULTS {
            return;
        }

        // Use file_type() (does NOT follow symlinks) and skip
        // symlinks entirely. This is the load-bearing piece of the
        // bound-session sandbox: a symlink inside the codebase that
        // points outside (e.g. `ln -s /etc <repo>/etc_link`) cannot
        // be traversed to read its target. The tradeoff: legitimate
        // symlinks inside a repo are also skipped.
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };
        if file_type.is_symlink() {
            continue;
        }

        let path = entry.path();

        if file_type.is_dir() {
            // Skip hidden directories (starting with '.').
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with('.') {
                    continue;
                }
            }
            walk_and_search(&path, regex, glob, depth + 1, results);
        } else if file_type.is_file() {
            check_file(&path, regex, glob, results);
        }
    }
}

/// Check a single file for regex matches.
fn check_file(
    path: &Path,
    regex: &regex::Regex,
    glob: Option<&glob::Pattern>,
    results: &mut Vec<Value>,
) {
    // Apply glob filter if provided.
    if let Some(g) = glob {
        let file_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !g.matches(file_name) {
            return;
        }
    }

    // Read file — skip binary files (non-UTF-8).
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let path_str = path.to_string_lossy().to_string();

    for (line_idx, line) in content.lines().enumerate() {
        if results.len() >= MAX_RESULTS {
            return;
        }
        if regex.is_match(line) {
            results.push(json!({
                "file": path_str,
                "line_number": line_idx + 1,
                "snippet": line
            }));
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
    async fn test_fs_search_basic_match() {
        let tmp = TempDir::new().unwrap();
        fs::write(
            tmp.path().join("main.rs"),
            "fn main() {\n    println!(\"hi\");\n}",
        )
        .unwrap();

        let ctx = make_context(tmp.path());
        let result = FsSearchTool
            .execute(json!({"pattern": "fn main"}), &ctx)
            .await;

        assert!(result.success);
        let results = result.data["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["line_number"], 1);
        assert!(results[0]["snippet"].as_str().unwrap().contains("fn main"));
    }

    #[tokio::test]
    async fn test_fs_search_with_glob_filter() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("main.rs"), "fn main() {}").unwrap();
        fs::write(tmp.path().join("readme.md"), "fn main is documented").unwrap();

        let ctx = make_context(tmp.path());
        let result = FsSearchTool
            .execute(json!({"pattern": "fn main", "glob": "*.rs"}), &ctx)
            .await;

        assert!(result.success);
        let results = result.data["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0]["file"].as_str().unwrap().ends_with("main.rs"));
    }

    #[tokio::test]
    async fn test_fs_search_no_matches() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("main.rs"), "fn main() {}").unwrap();

        let ctx = make_context(tmp.path());
        let result = FsSearchTool
            .execute(json!({"pattern": "nonexistent_pattern"}), &ctx)
            .await;

        assert!(result.success);
        assert_eq!(result.data["total_matches"], 0);
        assert!(result.data["results"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn test_fs_search_invalid_regex() {
        let tmp = TempDir::new().unwrap();
        let ctx = make_context(tmp.path());

        let result = FsSearchTool
            .execute(json!({"pattern": "[invalid"}), &ctx)
            .await;

        assert!(!result.success);
        assert!(result.error.unwrap().contains("Invalid regex"));
    }

    #[tokio::test]
    async fn test_fs_search_skips_binary_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("text.rs"), "fn main() {}").unwrap();
        // Write invalid UTF-8 bytes.
        fs::write(tmp.path().join("binary.bin"), [0xFF, 0xFE]).unwrap();

        let ctx = make_context(tmp.path());
        let result = FsSearchTool
            .execute(json!({"pattern": "fn main"}), &ctx)
            .await;

        assert!(result.success);
        let results = result.data["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn test_fs_search_skips_hidden_directories() {
        let tmp = TempDir::new().unwrap();
        let hidden = tmp.path().join(".hidden");
        fs::create_dir(&hidden).unwrap();
        fs::write(hidden.join("secret.rs"), "fn main() {}").unwrap();
        fs::write(tmp.path().join("visible.rs"), "fn main() {}").unwrap();

        let ctx = make_context(tmp.path());
        let result = FsSearchTool
            .execute(json!({"pattern": "fn main"}), &ctx)
            .await;

        assert!(result.success);
        let results = result.data["results"].as_array().unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0]["file"].as_str().unwrap().contains("visible"));
    }

    #[tokio::test]
    async fn test_fs_search_respects_max_results() {
        let tmp = TempDir::new().unwrap();
        // Create a file with more than MAX_RESULTS matching lines.
        let content = "match line\n".repeat(MAX_RESULTS + 50);
        fs::write(tmp.path().join("big.rs"), &content).unwrap();

        let ctx = make_context(tmp.path());
        let result = FsSearchTool
            .execute(json!({"pattern": "match line"}), &ctx)
            .await;

        assert!(result.success);
        let results = result.data["results"].as_array().unwrap();
        assert_eq!(results.len(), MAX_RESULTS);
    }

    #[tokio::test]
    async fn test_fs_search_multiple_files() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("a.rs"), "fn alpha() {}").unwrap();
        fs::write(tmp.path().join("b.rs"), "fn beta() {}").unwrap();

        let ctx = make_context(tmp.path());
        let result = FsSearchTool
            .execute(json!({"pattern": "fn \\w+", "glob": "*.rs"}), &ctx)
            .await;

        assert!(result.success);
        assert_eq!(result.data["total_matches"], 2);
    }
}
