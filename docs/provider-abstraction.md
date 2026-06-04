# Provider Abstraction

Trait-based abstraction for AI providers. Add new agents by implementing `CodingAgent` — no other code changes needed.

## The CodingAgent Trait

```rust
#[async_trait]
pub trait CodingAgent: Send + Sync {
    fn provider_type(&self) -> &str;
    fn display_name(&self) -> &str;
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;
    async fn send_message(
        &self,
        request: MessageRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>>>>>;
    async fn health_check(&self) -> Result<ProviderHealth>;
}

pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub context_window: u32,
}

pub struct MessageRequest {
    pub model: String,
    pub messages: Vec<Message>,
    pub system: Option<String>,
    pub max_tokens: u32,
    pub tools: Option<Vec<ToolDefinition>>,
}

pub struct Message {
    pub role: Role,
    pub content: Content,
}

pub enum Role { User, Assistant }

pub enum Content {
    Text(String),
    Blocks(Vec<ContentBlock>),
}

pub enum ContentBlock {
    Text { text: String },
    ToolUse { id: String, name: String, input: serde_json::Value },
    ToolResult { tool_use_id: String, content: String },
    Thinking { text: String },
}

pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

pub struct ProviderHealth {
    pub healthy: bool,
    pub latency_ms: u64,
    pub error: Option<String>,
}
```

## StreamEvent Types

```rust
pub enum StreamEvent {
    TextDelta { text: String },
    ToolUseStart { id: String, name: String, input: serde_json::Value },
    ToolUseDelta { id: String, delta: String },
    ToolResult { id: String, result: String },
    Thinking { text: String },
    Done { stop_reason: StopReason },
    Error { message: String },
}

pub enum StopReason {
    EndTurn,
    MaxTokens,
    ToolUse,
    Cancelled,
}
```

## AnthropicAgent Implementation

```rust
pub struct AnthropicAgent {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    default_model: String,
}
```

Sends requests to `{base_url}/v1/messages` with `stream: true`. The Anthropic SSE stream maps to `StreamEvent`:

| Anthropic Event | Maps to StreamEvent |
|-----------------|---------------------|
| `content_block_start` (type: tool_use) | `ToolUseStart { id, name, input }` |
| `content_block_delta` (type: text_delta) | `TextDelta { text }` |
| `content_block_delta` (type: input_json_delta) | `ToolUseDelta { id, delta }` |
| `message_delta` | `Done { stop_reason }` |
| `error` | `Error { message }` |

**Request format** — standard Anthropic Messages API with `stream: true` and tools injected from the ToolRegistry.

## Built-in Tools

Tools are organized into categories. Not all tools are available to all agents — see Tool Profiles below. All tools execute **server-side** — Weave validates, executes, and returns results.

### Filesystem
| Tool | Purpose |
|------|---------|
| `file_read` | Read file contents |
| `file_write` | Write/overwrite file (auto-creates parent dirs) |
| `file_edit` | Patch file via search/replace |
| `search_files` | Grep across files with optional glob |
| `list_directory` | List directory contents (optionally recursive) |

### Shell
| Tool | Purpose |
|------|---------|
| `shell_exec` | Execute shell command with optional cwd and timeout |

### Git
| Tool | Purpose |
|------|---------|
| `git_status` | Working tree status (branch, staged/unstaged/untracked) |
| `git_diff` | Unified diff output (truncated at 50KB) |
| `git_log` | Recent commit history |
| `git_commit` | Create commit (validates git identity — rejects placeholders) |

### Task Context
| Tool | Purpose |
|------|---------|
| `get_task` | Get current task details |
| `list_tasks` | List tasks in workspace |
| `update_task_status` | Report task progress |
| `update_task_fields` | Update task metadata |

### Inter-Agent Communication
| Tool | Purpose |
|------|---------|
| `send_message_to_agent` | Send message to another agent |
| `read_agent_conversation` | Read another agent's history |
| `get_agent_status` | Get agent status |
| `delegate_task` | Assign task to agent |
| `report_to_parent` | Submit completion report |

### Kanban
| Tool | Purpose |
|------|---------|
| `get_board` | Get board with columns and cards |
| `move_card` | Move card between columns (enforces transition gates) |
| `create_card` | Create new card |
| `search_cards` | Search cards by criteria |

### Note, Artifact, Web
| Tool | Purpose |
|------|---------|
| `create_note` / `read_note` / `list_notes` / `set_note_content` / `append_to_note` | CRUD for structured notes |
| `request_artifact` / `provide_artifact` / `list_artifacts` | Evidence artifacts (screenshots, diffs, logs) |
| `fetch_webpage` | Fetch URL content (truncated at 12KB) |

### Tool Result Format
```json
// Success
{"success": true, "data": {"bytes_written": 1234, "path": "/src/login.tsx"}}
// Error
{"success": false, "error": "Permission denied: path outside workspace root"}
```

## Tool Profiles

Tool profiles control which tools each specialist can access.

| Profile | Tools | Use Case |
|---------|-------|----------|
| `full` | All tools | Coordinator agents (routa) |
| `implementation` | Filesystem + Shell + Git + Task context | Implementor agents (crafter, dev-crafter) |
| `review` | Filesystem (read-only) + Git + Task context + Artifacts | Verifier agents (gate, review-guard) |
| `planning` | Task context + Kanban + Notes (no filesystem/shell) | Planning agents (backlog-refiner, todo-orchestrator) |
| `reporting` | Task context (read-only) + Notes + Artifacts | Reporting agents (done-reporter) |

Specialists specify their profile in YAML frontmatter: `tool_profile: implementation`. Defaults to `full` if unspecified. `SessionService` filters the full tool inventory by the specialist's profile when building the `MessageRequest.tools` list.

## Execution Model

```
Provider returns: tool_use { id, name, input }
         │
         ▼
ToolExecutor validates input against JSON schema
         │
         ▼
Check tool is allowed by session's tool profile
         │
         ▼
Execute tool server-side (write file, run command, etc.)
         │
         ▼
Record trace event (tool name, input, output, duration_ms)
         │
         ▼
Send tool_result back to provider
         │
         ▼
Provider continues generation
```

### Rust Trait

```rust
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    fn name(&self) -> &str;
    fn input_schema(&self) -> serde_json::Value;
    async fn execute(&self, input: serde_json::Value, context: &ToolContext) -> ToolResult;
}

pub struct ToolContext {
    pub session_id: String,
    pub cwd: PathBuf,
    pub codebase_root: PathBuf,
    pub trace_collector: Arc<TraceCollector>,
}

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ToolExecutor>>,
    profiles: HashMap<String, Vec<String>>,  // profile_name -> tool_names
}
```

Built once in `main.rs`, shared via `AppState`.

## Security Constraints

**Path containment:**
- All file paths must be absolute
- Write operations restricted to session's codebase path
- `..` traversal rejected — path must resolve within codebase root
- No privilege separation in v1 (same OS user as Weave process)

**Control-plane protection** — these paths are read-only:
- `.git/config`, `.git/hooks/`, `.git/HEAD`
- `weave.db`, `weave.db-wal`, `weave.db-shm`
- `resources/specialists/*.md`
- `Cargo.toml`, `Cargo.lock`

**Git identity validation:** `git_commit` rejects placeholder/test identities.

**Kanban transition gates:** `move_card` enforces required artifacts, task fields, and description freeze. Repeated failures label the card `contract-gate-blocked`.

**Audit trail:** All tool executions recorded as trace events with name, sanitized input (secrets stripped), output (truncated to 10KB), duration, and status.
