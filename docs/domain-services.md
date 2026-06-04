# Domain Services

Business logic orchestration layer. Services coordinate between stores, the provider registry, specialists, tools, and SSE — but never access the database directly (that's the store layer).

## SessionService

Manages the full lifecycle of agent sessions.

```rust
pub struct SessionService {
    store: Arc<SessionStore>,
    provider_registry: Arc<ProviderRegistry>,
    specialist_loader: Arc<SpecialistLoader>,
    tool_registry: Arc<ToolRegistry>,
    trace_collector: Arc<TraceCollector>,
    streams: Arc<RwLock<HashMap<String, broadcast::Sender<StreamEvent>>>>,
}
```

### Create Session
```
1. Validate provider_id exists in registry
2. Validate specialist_id exists (if provided)
3. If parent_session_id provided:
   a. Load parent session's message history
   b. Copy messages into new session as initial context
4. Insert session row (status: "connecting")
5. Create broadcast channel for SSE events
6. Return session
```

### Send Prompt
```
1. Load session from DB (verify status is "ready" or "connecting")
2. Save user message to DB
3. Load specialist system prompt (if specialist_id set)
4. Build message history from DB + new user message
5. Resolve tool set: filter tool_registry by specialist's tool_profile
6. Get CodingAgent from provider_registry
7. Call agent.send_message(request) with filtered tools
8. Spawn async task to:
   a. Stream events from agent
   b. For each event:
      - Broadcast to SSE channel
      - If text_delta: accumulate into response buffer
      - If tool_use_start: validate tool is in profile, record trace event
      - If tool_result: execute tool via ToolExecutor, record trace event
      - If thinking: record decision trace event
      - If done: save full assistant message, update session status
      - If error: save error, update session status
9. Return message_id immediately
```

### Cancel / Resume
- **Cancel**: Drop agent stream (abort HTTP request to provider), update status to "cancelled", broadcast cancel event.
- **Resume**: Load parent's full message history, create new session with `parent_session_id` set, copy all messages into new session.

**Performance note:** Copying all messages on resume is O(n). For typical sessions of <200 messages this is acceptable. Future: chain sessions at query time or copy only last N messages with summary.

## Agent Execution Model

Each agent session operates within a **workspace context** — a working directory and a set of tools. No sandboxing in v1.

**Runtime context per session:**

| Property | Source | Purpose |
|----------|--------|---------|
| `cwd` | Session `cwd` column (nullable) | Working directory for shell_exec, file tools |
| `codebase` | Session → Codebase via `cwd` match | Git context for git_* tools |
| `tools` | Built-in tool set | What the agent can do |
| `system_prompt` | Specialist markdown body | Agent behavior instructions |
| `provider` | Session → Provider | Which LLM to call |

**Tool call lifecycle:**
1. Provider returns a `tool_use` content block (name + input JSON)
2. Weave validates the input against the tool's JSON schema
3. Weave executes the tool server-side (file I/O, subprocess, git command)
4. Weave records a trace event (tool name, input, output, duration)
5. Weave sends the `tool_result` back to the provider as the next user message
6. Provider continues generation

**Filesystem access rules:**
- All file paths must be absolute
- Write operations restricted to the session's codebase path
- `..` traversal rejected — path must resolve within codebase root
- `shell_exec` runs with same OS user as Weave process (no privilege separation in v1)

## ProviderRegistry

```rust
pub struct ProviderRegistry {
    providers: RwLock<HashMap<String, Arc<dyn CodingAgent>>>,
    store: Arc<ProviderStore>,
}
```

- Loaded from SQLite on startup
- `add_provider()`: validates config, creates agent instance, inserts into DB
- `remove_provider()`: removes from map and DB, fails if active sessions reference it
- `get_agent()`: returns `Arc<dyn CodingAgent>` by provider ID
- `health_check_all()`: runs health checks in parallel, returns status map

## SpecialistLoader

Loads specialist definitions from markdown files in `resources/specialists/`.

```rust
pub struct Specialist {
    pub id: String,           // filename without extension
    pub name: String,         // from frontmatter
    pub model: Option<String>, // from frontmatter
    pub description: String,  // from frontmatter
    pub tool_profile: String, // from frontmatter, defaults to "full"
    pub tags: Vec<String>,    // from frontmatter
    pub system_prompt: String, // markdown body after frontmatter
}
```

**Frontmatter fields:**
| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Human-readable specialist name |
| `description` | Yes | One-line description of the specialist's role |
| `model` | No | Preferred model tier (`sonnet`, `haiku`, `opus`) |
| `tool_profile` | No | Tool profile name. Defaults to `full` |
| `tags` | No | Metadata tags for filtering/search |

**Error handling:**
| Scenario | Behavior |
|----------|----------|
| Malformed YAML frontmatter | Log warning, skip file |
| Missing required fields | Log warning, skip file |
| File deleted while session active | Session continues with already-loaded prompt |
| Column references non-existent specialist | `move_task` returns error, card move rejected |
| Empty `specialists_dir` | Server starts with zero specialists |

## KanbanService

```rust
pub struct KanbanService {
    store: Arc<KanbanStore>,
    session_service: Arc<SessionService>,
    specialist_loader: Arc<SpecialistLoader>,
    streams: Arc<RwLock<HashMap<String, broadcast::Sender<KanbanEvent>>>>,
}
```

**Lane automation flow:**
```
move_task(task_id, new_column_id):
  1. Load task and target column
  2. Update task.column_id and task.position
  3. Broadcast task_moved event
  4. If column.auto_trigger AND column.specialist_id:
     a. Load specialist by column.specialist_id
     b. Create session with specialist's system prompt
     c. Associate session with task (task.session_id = session.id)
     d. Send initial prompt: "Process task: {task.title}\n\n{task.description}"
     e. Broadcast session_started event
  5. Return updated task
```

## TraceCollector

```rust
pub struct TraceCollector {
    store: Arc<TraceStore>,
}
```

**Collection points:**
- **Tool calls**: Intercepted from `StreamEvent::ToolUseStart` / `ToolResult`
- **File changes**: Extracted from tool call inputs/outputs (e.g., file_write tool)
- **Decisions**: Extracted from `StreamEvent::Thinking` blocks
- **Errors**: From `StreamEvent::Error`
- **Reviews**: Explicitly recorded when specialist completes verification

**Journey summary** (`GET /api/sessions/:sid/trace/journey`): Load all trace events ordered by timestamp, filter to Decision/Milestone/Review events, return ordered list with timestamps and summaries.
