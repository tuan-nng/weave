use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

pub mod fs;
pub mod git;
pub mod shell;

use crate::agent::ToolDefinition;
use crate::error::AppError;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Stub trace collector for future observability (feat-017).
///
/// Will be replaced with a real implementation that records tool calls,
/// file changes, and decisions during agent execution.
pub struct TraceCollector;

impl TraceCollector {
    pub fn new() -> Self {
        Self
    }
}

/// Context passed to tool execution.
///
/// Carries session-scoped state that tools need to operate:
/// the session they belong to, the working directory, the codebase
/// root (for path containment), and a trace collector for observability.
pub struct ToolContext {
    pub session_id: String,
    pub cwd: PathBuf,
    pub codebase_root: PathBuf,
    pub trace_collector: Arc<TraceCollector>,
}

/// Result of a tool invocation.
///
/// Returned to the provider as a JSON-serialized `tool_result` content block.
/// The `success` field lets the provider distinguish success from failure
/// without parsing the data payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub success: bool,
    pub data: serde_json::Value,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// ToolExecutor trait
// ---------------------------------------------------------------------------

/// A tool that can be invoked by an agent during a session.
///
/// Implementations are registered in the `ToolRegistry` at startup.
/// The trait is `async_trait` + `Send + Sync` to match the `CodingAgent`
/// pattern and allow future async tool implementations (shell, HTTP).
///
/// Concrete implementations will be added in feat-013+ (filesystem, shell,
/// git, task, kanban, notes, artifacts).
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Tool name (matches the name sent to the provider).
    fn name(&self) -> &str;

    /// Human-readable description of what the tool does.
    fn description(&self) -> &str;

    /// JSON Schema describing the tool's input parameters.
    fn input_schema(&self) -> serde_json::Value;

    /// Execute the tool and return the result.
    async fn execute(&self, input: serde_json::Value, context: &ToolContext) -> ToolResult;
}

// ---------------------------------------------------------------------------
// ToolRegistry
// ---------------------------------------------------------------------------

/// Registry of available tools and their profile mappings.
///
/// Built once in `main.rs` and shared via `AppState` as `Arc<ToolRegistry>`.
/// Tools are registered at startup via `register()` and never modified after.
/// No `Mutex` is needed since the registry is immutable after construction.
///
/// Profiles map specialist roles to subsets of tools. When building a
/// `MessageRequest`, `SessionService` calls `resolve_profile()` to get
/// the filtered `Vec<ToolDefinition>` for the specialist's profile.
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ToolExecutor>>,
    profiles: HashMap<String, Vec<String>>,
}

impl ToolRegistry {
    /// Create a new registry with the five built-in profiles.
    ///
    /// The `"full"` profile is a sentinel — it resolves dynamically to all
    /// registered tools at resolution time.
    pub fn new() -> Self {
        let mut profiles = HashMap::new();
        // "full" is a sentinel — resolved dynamically to all tools
        profiles.insert("full".to_string(), Vec::new());
        profiles.insert(
            "implementation".to_string(),
            vec![
                "fs_read".to_string(),
                "fs_write".to_string(),
                "fs_edit".to_string(),
                "fs_search".to_string(),
                "fs_list".to_string(),
                "shell_exec".to_string(),
                "git_status".to_string(),
                "git_diff".to_string(),
                "git_log".to_string(),
                "git_commit".to_string(),
                "task".to_string(),
            ],
        );
        profiles.insert(
            "review".to_string(),
            vec![
                "fs_read".to_string(),
                "fs_search".to_string(),
                "git_status".to_string(),
                "git_diff".to_string(),
                "git_log".to_string(),
                "task".to_string(),
                "artifacts".to_string(),
            ],
        );
        profiles.insert(
            "planning".to_string(),
            vec![
                "task".to_string(),
                "kanban".to_string(),
                "notes".to_string(),
            ],
        );
        profiles.insert(
            "reporting".to_string(),
            vec![
                "task_read".to_string(),
                "notes".to_string(),
                "artifacts".to_string(),
            ],
        );

        Self {
            tools: HashMap::new(),
            profiles,
        }
    }

    /// Register a tool in the registry.
    ///
    /// The tool's `name()` is used as the key. If a tool with the same name
    /// already exists, it is replaced.
    pub fn register(&mut self, tool: Arc<dyn ToolExecutor>) {
        let name = tool.name().to_string();
        self.tools.insert(name, tool);
    }

    /// Get a tool by name.
    pub fn get(&self, name: &str) -> Option<&Arc<dyn ToolExecutor>> {
        self.tools.get(name)
    }

    /// Validate that a profile name is recognized without resolving tool definitions.
    ///
    /// Cheaper than `resolve_profile` — used for early validation in `send_prompt`
    /// where we want fail-fast behavior without constructing `ToolDefinition` objects.
    pub fn validate_profile_name(&self, profile_name: &str) -> Result<(), AppError> {
        if profile_name == "full" || self.profiles.contains_key(profile_name) {
            Ok(())
        } else {
            Err(AppError::Validation(
                self.unknown_profile_error(profile_name),
            ))
        }
    }

    /// Resolve a profile name to a list of tool definitions.
    ///
    /// For the `"full"` profile, all registered tools are returned.
    /// For other profiles, only tools that are both in the profile list AND
    /// registered in the registry are included (unregistered names are silently
    /// skipped — tools register in feat-013+).
    ///
    /// Returns `AppError::Validation` if the profile name is not recognized.
    pub fn resolve_profile(&self, profile_name: &str) -> Result<Vec<ToolDefinition>, AppError> {
        if profile_name == "full" {
            return Ok(self.all_definitions());
        }

        let tool_names = self
            .profiles
            .get(profile_name)
            .ok_or_else(|| AppError::Validation(self.unknown_profile_error(profile_name)))?;

        let defs: Vec<ToolDefinition> = tool_names
            .iter()
            .filter_map(|name| self.tools.get(name))
            .map(Self::to_definition)
            .collect();

        Ok(defs)
    }

    /// Get definitions for all registered tools, sorted by name for deterministic output.
    fn all_definitions(&self) -> Vec<ToolDefinition> {
        let mut defs: Vec<ToolDefinition> = self.tools.values().map(Self::to_definition).collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// Convert a tool executor reference to a `ToolDefinition`.
    fn to_definition(tool: &Arc<dyn ToolExecutor>) -> ToolDefinition {
        ToolDefinition {
            name: tool.name().to_string(),
            description: tool.description().to_string(),
            input_schema: tool.input_schema(),
        }
    }

    /// Build the error message for an unknown profile, dynamically listing valid names.
    fn unknown_profile_error(&self, profile_name: &str) -> String {
        let mut valid: Vec<&str> = self.profiles.keys().map(|k| k.as_str()).collect();
        valid.sort();
        valid.push("full");
        format!(
            "unknown tool profile '{}'; valid profiles: {}",
            profile_name,
            valid.join(", ")
        )
    }

    /// Get the number of registered tools.
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// Get the number of defined profiles.
    pub fn profile_count(&self) -> usize {
        self.profiles.len()
    }
}

impl Default for ToolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Shared I/O helpers
// ---------------------------------------------------------------------------

/// Spawn a task that reads an async reader to completion, returning the bytes.
///
/// Used by shell and git tools to capture stdout/stderr from child processes.
pub(crate) fn spawn_read_task(
    handle: Option<impl tokio::io::AsyncRead + Unpin + Send + 'static>,
) -> tokio::task::JoinHandle<Vec<u8>> {
    tokio::spawn(async move {
        match handle {
            Some(mut h) => {
                use tokio::io::AsyncReadExt;
                let mut buf = Vec::new();
                h.read_to_end(&mut buf).await.ok();
                buf
            }
            None => Vec::new(),
        }
    })
}

/// Truncate bytes to `max_bytes`, finding a safe UTF-8 boundary.
///
/// Returns (content, truncated_flag).
pub(crate) fn truncate_bytes(bytes: &[u8], max_bytes: usize) -> (String, bool) {
    if bytes.len() <= max_bytes {
        return (String::from_utf8_lossy(bytes).into_owned(), false);
    }

    let mut end = max_bytes;
    while end > 0 && (bytes[end] & 0xC0) == 0x80 {
        end -= 1;
    }

    (String::from_utf8_lossy(&bytes[..end]).into_owned(), true)
}

// ---------------------------------------------------------------------------
// Test support (shared across test modules)
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use async_trait::async_trait;

    /// Create a `ToolContext` for testing with the given root path.
    pub(crate) fn make_context(root: &std::path::Path) -> super::ToolContext {
        super::ToolContext {
            session_id: "test-session".to_string(),
            cwd: root.to_path_buf(),
            codebase_root: root.to_path_buf(),
            trace_collector: std::sync::Arc::new(super::TraceCollector::new()),
        }
    }

    /// Mock tool for testing tool registry operations.
    pub(crate) struct MockTool {
        tool_name: String,
    }

    impl MockTool {
        pub(crate) fn new(name: &str) -> Self {
            Self {
                tool_name: name.to_string(),
            }
        }
    }

    #[async_trait]
    impl ToolExecutor for MockTool {
        fn name(&self) -> &str {
            &self.tool_name
        }

        fn description(&self) -> &str {
            "mock tool for testing"
        }

        fn input_schema(&self) -> serde_json::Value {
            serde_json::json!({"type": "object"})
        }

        async fn execute(&self, _input: serde_json::Value, _ctx: &ToolContext) -> ToolResult {
            ToolResult {
                success: true,
                data: serde_json::json!(null),
                error: None,
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::MockTool;

    #[test]
    fn test_registry_new_has_five_profiles() {
        let registry = ToolRegistry::new();
        assert_eq!(registry.profile_count(), 5);
    }

    #[test]
    fn test_register_and_get_tool() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(MockTool::new("fs_read")));

        let tool = registry.get("fs_read");
        assert!(tool.is_some());
        assert_eq!(tool.unwrap().name(), "fs_read");
    }

    #[test]
    fn test_get_nonexistent_tool() {
        let registry = ToolRegistry::new();
        assert!(registry.get("nonexistent").is_none());
    }

    #[test]
    fn test_register_replaces_existing() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(MockTool::new("fs_read")));
        registry.register(Arc::new(MockTool::new("fs_read"))); // replace
        assert_eq!(registry.tool_count(), 1);
    }

    #[test]
    fn test_resolve_profile_implementation() {
        let mut registry = ToolRegistry::new();
        // Register some tools that match the implementation profile
        registry.register(Arc::new(MockTool::new("fs_read")));
        registry.register(Arc::new(MockTool::new("fs_write")));
        registry.register(Arc::new(MockTool::new("shell_exec")));
        registry.register(Arc::new(MockTool::new("git_status")));
        registry.register(Arc::new(MockTool::new("git_diff")));
        registry.register(Arc::new(MockTool::new("git_log")));
        registry.register(Arc::new(MockTool::new("git_commit")));
        registry.register(Arc::new(MockTool::new("task")));

        let defs = registry.resolve_profile("implementation").unwrap();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(
            names,
            vec![
                "fs_read",
                "fs_write",
                "shell_exec",
                "git_status",
                "git_diff",
                "git_log",
                "git_commit",
                "task"
            ]
        );
    }

    #[test]
    fn test_resolve_profile_full_expands_all() {
        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(MockTool::new("fs_read")));
        registry.register(Arc::new(MockTool::new("shell_exec")));
        registry.register(Arc::new(MockTool::new("git_status")));
        registry.register(Arc::new(MockTool::new("git_diff")));

        let defs = registry.resolve_profile("full").unwrap();
        assert_eq!(defs.len(), 4);
        let mut names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        names.sort();
        assert_eq!(
            names,
            vec!["fs_read", "git_diff", "git_status", "shell_exec"]
        );
    }

    #[test]
    fn test_resolve_profile_invalid_name() {
        let registry = ToolRegistry::new();
        let result = registry.resolve_profile("nonexistent");
        assert!(result.is_err());
        match result.unwrap_err() {
            AppError::Validation(msg) => {
                assert!(msg.contains("nonexistent"));
                assert!(msg.contains("unknown tool profile"));
            }
            other => panic!("expected Validation, got: {:?}", other),
        }
    }

    #[test]
    fn test_resolve_profile_empty_registry() {
        let registry = ToolRegistry::new();
        // "full" with no registered tools returns empty vec
        let defs = registry.resolve_profile("full").unwrap();
        assert!(defs.is_empty());
    }

    #[test]
    fn test_resolve_profile_skips_unregistered_tools() {
        let mut registry = ToolRegistry::new();
        // Only register fs_read, not fs_write
        registry.register(Arc::new(MockTool::new("fs_read")));

        let defs = registry.resolve_profile("implementation").unwrap();
        let names: Vec<&str> = defs.iter().map(|d| d.name.as_str()).collect();
        // fs_write is in the profile but not registered — silently skipped
        assert_eq!(names, vec!["fs_read"]);
    }

    #[test]
    fn test_tool_result_serde_roundtrip() {
        let result = ToolResult {
            success: true,
            data: serde_json::json!({"bytes_written": 42}),
            error: None,
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ToolResult = serde_json::from_str(&json).unwrap();
        assert!(deserialized.success);
        assert_eq!(deserialized.data["bytes_written"], 42);
        assert!(deserialized.error.is_none());
    }

    #[test]
    fn test_tool_result_error_serde() {
        let result = ToolResult {
            success: false,
            data: serde_json::json!(null),
            error: Some("permission denied".to_string()),
        };
        let json = serde_json::to_string(&result).unwrap();
        let deserialized: ToolResult = serde_json::from_str(&json).unwrap();
        assert!(!deserialized.success);
        assert_eq!(deserialized.error.unwrap(), "permission denied");
    }

    #[test]
    fn test_types_are_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<ToolResult>();
        assert_send_sync::<ToolContext>();
    }
}
