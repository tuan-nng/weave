# Weave — System Design

## 1. Overview

Weave is a web-based multi-agent coordination platform. A single Rust binary serves a REST+SSE API and a bundled React SPA. Agent sessions are managed through a trait-based provider abstraction with Anthropic as the first implementation. Kanban boards serve as coordination buses where column transitions can trigger specialist agent sessions.

**Key design decisions:**
- Single binary, no external runtime dependencies
- SQLite (WAL mode) for all persistence — zero configuration
- SSE for all real-time streaming — no WebSocket, no polling
- Trait-based provider abstraction — add new agents by implementing `CodingAgent`
- Workspace-scoped resources — everything lives under a workspace (except providers, which are global)
- Single-user, localhost-first — no auth layer in v1 (see §17.5 threat model)
- Agent tools execute server-side — no sandboxing in v1 (see §6.6, §7.4)

---

## 2. Architecture Layers

```
┌─────────────────────────────────────────────────────────┐
│                     Browser (React SPA)                  │
│  Pages: Home · Session · Kanban · Settings · Codebase    │
│  Hooks: useSession · useKanban · useProviders            │
│  Lib:   api.ts (fetch wrapper) · types.ts                │
└────────────────────────┬────────────────────────────────┘
                         │  REST + SSE
┌────────────────────────┴────────────────────────────────┐
│                   Axum HTTP Server                       │
│  Middleware: CORS · Request ID · Tracing · Error Handler  │
│  Routes: /api/workspaces · /api/sessions · /api/providers│
│          /api/boards · /api/codebases · /api/traces       │
│          /api/health                                      │
└────────────────────────┬────────────────────────────────┘
                         │
┌────────────────────────┴────────────────────────────────┐
│                   Domain Services                         │
│  SessionService · KanbanService · ProviderRegistry        │
│  SpecialistLoader · ToolRegistry · TraceCollector         │
└──────────┬─────────────────────────────────┬────────────┘
           │                                 │
┌──────────┴──────────┐           ┌──────────┴──────────────┐
│   SQLite Store      │           │   CodingAgent Trait      │
│   (rusqlite)        │           │   ┌──────────────────┐  │
│   WorkspaceStore    │           │   │  AnthropicAgent  │  │
│   SessionStore      │           │   │  (reqwest + SSE) │  │
│   ProviderStore     │           │   └──────────────────┘  │
│   KanbanStore       │           │   ┌──────────────────┐  │
│   TraceStore        │           │   │  Future Agents   │  │
│   CodebaseStore     │           │   └──────────────────┘  │
└─────────────────────┘           └─────────────────────────┘
                                          │
                                          │ HTTPS
                                          ▼
                                  ┌─────────────────┐
                                  │  Anthropic API  │
                                  │  (or proxy)     │
                                  └─────────────────┘
```

### Layer Responsibilities

| Layer | Responsibility | Key Constraint |
|-------|---------------|----------------|
| **HTTP Server** | Request routing, middleware, serialization, SSE framing | No business logic |
| **Domain Services** | Orchestration, state transitions, event dispatch | No direct DB access — goes through stores |
| **Store** | SQL queries, row↔struct mapping, transactions | No business logic — pure data access |
| **Agent** | Provider communication, stream normalization, tool result injection | No persistence — returns `Stream<Item = StreamEvent>` |
| **Tool** | Tool execution, input validation, path containment, audit logging | No provider communication — pure side-effect execution |

---

## 3. Module Design

### 3.1 Entry Point (`main.rs`)

```rust
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Parse CLI args (--host, --port, --db-path, --allow-remote)
    // 2. Initialize tracing subscriber (env filter)
    // 3. Open SQLite database, run migrations
    // 4. Seed default workspace if none exists
    // 5. Load providers from DB into ProviderRegistry
    // 6. Load specialists from filesystem into SpecialistLoader
    // 7. Build ToolRegistry (register all tools + profiles)
    // 8. Build Axum router with all routes
    // 9. Start HTTP server
    // 10. Graceful shutdown on SIGTERM/SIGINT
}
```

**Startup order matters.** The database must be ready before providers load, providers must be ready before the HTTP server starts, and the tool registry must be built before sessions can be created (tools are injected into provider requests).

### 3.2 Configuration (`config.rs`)

```rust
pub struct Config {
    pub host: String,           // default: "127.0.0.1"
    pub port: u16,              // default: 3000
    pub db_path: PathBuf,       // default: "weave.db"
    pub specialists_dir: PathBuf, // default: "resources/specialists"
    pub allow_remote: bool,     // default: false, required to bind to non-localhost (see §17.5)
}
```

Loaded from CLI args only for v1. Environment variable and config file support deferred to Phase 6.

### 3.3 Database (`db.rs`)

```rust
pub fn open_database(path: &Path) -> anyhow::Result<Connection> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    run_migrations(&conn)?;
    Ok(conn)
}
```

**Migration strategy**: Versioned SQL files embedded in the binary via `include_str!`. Each migration has an up and down. The `schema_version` pragma tracks current version.

```rust
const MIGRATIONS: &[(&str, &str)] = &[
    ("001", include_str!("migrations/001_init.sql")),
    ("002", include_str!("migrations/002_kanban.sql")),
    // ...
];

fn run_migrations(conn: &Connection) -> anyhow::Result<()> {
    let current: i32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    for (version, sql) in MIGRATIONS {
        let v: i32 = version.parse()?;
        if v > current {
            conn.execute_batch(sql)?;
            conn.pragma_update(None, "user_version", v)?;
        }
    }
    Ok(())
}
```

---

## 4. Data Model

### 4.1 Entity-Relationship Diagram

```
Workspace (1) ─────┬── (N) Session
                    ├── (N) Codebase
                    └── (N) Board

Provider (*)        Global — not workspace-scoped (see §5.5)
Specialist (*)      Filesystem-loaded — not in DB (see §6.3)

Session (1) ────────┬── (N) Message
                    ├── (N) TraceEvent
                    └── (N) FileChange

Session (N) ──────── (0..1) Session  (parent_session_id for resume)

Board (1) ──────────┬── (N) Column
                    └── (N) Task

Column (1) ───────── (N) Task
Column (N) ───────── (0..1) Specialist (specialist_id binding, filesystem reference)

Task (N) ─────────── (0..1) Session (session_id when agent is working on it)

Codebase (1) ─────── (N) Worktree  (reserved for v2 — see §4.2)
```

**Scope rules:**
- **Providers** are global — a single provider can serve sessions across all workspaces. The `/api/providers` endpoints are not workspace-scoped.
- **Specialists** are filesystem resources (markdown files), not DB entities. Columns reference them by string ID.
- **Worktrees** are defined in the schema but have no API surface in v1. The table is reserved for future git worktree isolation (see §4.2).

### 4.2 Schema Definitions

```sql
-- Workspaces
CREATE TABLE workspaces (
    id          TEXT PRIMARY KEY,          -- UUID v4
    name        TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'active',  -- active | archived
    created_at  TEXT NOT NULL,             -- ISO 8601
    updated_at  TEXT NOT NULL              -- ISO 8601
);

-- Sessions
CREATE TABLE sessions (
    id                  TEXT PRIMARY KEY,
    workspace_id        TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    provider_id         TEXT NOT NULL REFERENCES providers(id),
    specialist_id       TEXT,              -- nullable, references specialists loaded from filesystem
    parent_session_id   TEXT REFERENCES sessions(id),  -- nullable, for resume
    status              TEXT NOT NULL DEFAULT 'connecting',
    model               TEXT,              -- nullable, overrides provider default
    cwd                 TEXT,              -- nullable, working directory
    created_at          TEXT NOT NULL,
    updated_at          TEXT NOT NULL
);
CREATE INDEX idx_sessions_workspace ON sessions(workspace_id);
CREATE INDEX idx_sessions_status ON sessions(status);

-- Messages
-- Design note: messages are immutable — no updated_at column.
-- Once written, a message is never modified. New messages are appended.
CREATE TABLE messages (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role        TEXT NOT NULL,             -- user | assistant | tool_use | tool_result
    content     TEXT NOT NULL,             -- JSON-encoded content (see §4.5)
    metadata    TEXT,                      -- JSON-encoded extra data
    created_at  TEXT NOT NULL
);
CREATE INDEX idx_messages_session ON messages(session_id);

-- Providers
CREATE TABLE providers (
    id          TEXT PRIMARY KEY,
    type        TEXT NOT NULL,             -- anthropic | openai | local | cli
    name        TEXT NOT NULL,
    config_json TEXT NOT NULL,             -- {base_url, api_key, default_model, ...}
    created_at  TEXT NOT NULL
);

-- Codebases
CREATE TABLE codebases (
    id          TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    path        TEXT NOT NULL,             -- absolute filesystem path
    branch      TEXT,                      -- nullable, defaults to current
    label       TEXT,                      -- human-readable name
    created_at  TEXT NOT NULL
);
CREATE UNIQUE INDEX idx_codebases_path ON codebases(workspace_id, path);

-- Worktrees
-- Reserved for v2: git worktree isolation per agent session.
-- No API endpoints or domain service in v1. Schema is pre-allocated
-- to avoid a migration later. Do NOT create store/API code for this table yet.
CREATE TABLE worktrees (
    id          TEXT PRIMARY KEY,
    codebase_id TEXT NOT NULL REFERENCES codebases(id) ON DELETE CASCADE,
    branch      TEXT NOT NULL,
    path        TEXT NOT NULL,             -- absolute path to worktree
    session_id  TEXT REFERENCES sessions(id),  -- nullable, linked session
    created_at  TEXT NOT NULL
);

-- Kanban: Boards
CREATE TABLE boards (
    id              TEXT PRIMARY KEY,
    workspace_id    TEXT NOT NULL REFERENCES workspaces(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    created_at      TEXT NOT NULL
);

-- Kanban: Columns
CREATE TABLE columns (
    id              TEXT PRIMARY KEY,
    board_id        TEXT NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
    name            TEXT NOT NULL,
    position        INTEGER NOT NULL DEFAULT 0,
    specialist_id   TEXT,                  -- nullable, references filesystem specialist
    auto_trigger    INTEGER NOT NULL DEFAULT 0,  -- boolean: 0=off, 1=on
    created_at      TEXT NOT NULL
);
CREATE INDEX idx_columns_board ON columns(board_id);

-- Kanban: Tasks
CREATE TABLE tasks (
    id          TEXT PRIMARY KEY,
    board_id    TEXT NOT NULL REFERENCES boards(id) ON DELETE CASCADE,
    column_id   TEXT NOT NULL REFERENCES columns(id),
    title       TEXT NOT NULL,
    description TEXT,
    position    INTEGER NOT NULL DEFAULT 0,
    status      TEXT NOT NULL DEFAULT 'active',  -- active | done | archived
    session_id  TEXT REFERENCES sessions(id),     -- nullable, agent working on it
    created_at  TEXT NOT NULL,
    updated_at  TEXT NOT NULL
);
CREATE INDEX idx_tasks_board ON tasks(board_id);
CREATE INDEX idx_tasks_column ON tasks(column_id);

-- Traces
-- Design note: traces are immutable append-only records — no updated_at column.
-- Traces are never modified after creation.
CREATE TABLE traces (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    event_type  TEXT NOT NULL,             -- decision | tool_call | file_change | error | milestone | review
    summary     TEXT NOT NULL,
    data_json   TEXT,                      -- JSON-encoded event-specific data
    timestamp   TEXT NOT NULL
);
CREATE INDEX idx_traces_session ON traces(session_id);

-- File Changes
CREATE TABLE file_changes (
    id          TEXT PRIMARY KEY,
    trace_id    TEXT REFERENCES traces(id),  -- nullable, linked trace event
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    path        TEXT NOT NULL,
    action      TEXT NOT NULL,             -- read | write | create | delete
    timestamp   TEXT NOT NULL
);
CREATE INDEX idx_file_changes_session ON file_changes(session_id);
```

### 4.3 ID Generation

All IDs are UUID v4 strings (`uuid::Uuid::new_v4().to_string()`). No auto-increment, no sequential IDs. This avoids:
- ID enumeration attacks
- Cross-database conflicts if SQLite files are copied
- Coordination overhead in concurrent inserts

### 4.4 Timestamps

All timestamps are ISO 8601 strings stored as TEXT (`chrono::Utc::now().to_rfc3339()`). SQLite has no native datetime type; TEXT with ISO format is sortable and unambiguous.

### 4.5 Message Content Format

Messages store `content` as JSON-encoded TEXT. The schema depends on the `role`:

**User messages** (`role: "user"`):
```json
// Simple text
{"type": "text", "text": "Implement the login page"}

// Tool result (response to agent's tool use)
{"type": "tool_result", "tool_use_id": "toolu_...", "content": "File written successfully"}
```

**Assistant messages** (`role: "assistant"`):
```json
// Simple text response
{"type": "text", "text": "I'll implement the login page."}

// Mixed content (text + tool calls)
[
    {"type": "text", "text": "I'll implement the login page."},
    {"type": "tool_use", "id": "toolu_...", "name": "file_write", "input": {"path": "/src/login.tsx", "content": "..."}},
    {"type": "thinking", "text": "The user wants a login page with email/password..."}
]
```

**Storage rule**: Content is stored exactly as received from the provider (normalized to a JSON array of blocks). The `metadata` column holds session-level annotations (e.g., `{"model": "claude-sonnet-4-5", "tokens_used": 1234}`) that are not part of the conversation.

### 4.6 Pagination

All list endpoints accept optional query parameters for pagination:

```
GET /api/workspaces?page=1&limit=50
GET /api/workspaces/:wid/sessions?page=1&limit=20
GET /api/sessions/:sid/history?cursor=msg_abc123&limit=50
```

**Two strategies:**
- **Offset-based** (`page` + `limit`): Used for small, bounded collections (workspaces, sessions, boards). Default `limit=50`, max `100`.
- **Cursor-based** (`cursor` + `limit`): Used for append-only, potentially large collections (messages, traces). The cursor is the `id` of the last item from the previous page. More efficient for real-time data where items are inserted between pages.

**Response envelope** (updated):
```json
{
    "data": [...],
    "total": 42,
    "page": 1,          // offset-based only
    "limit": 50,
    "has_more": true,
    "next_cursor": "uuid"  // cursor-based only
}
```

---

## 5. API Contracts

### 5.1 Common Envelope

All JSON responses use a consistent shape:

```json
// Success (single resource)
{"data": { ... }}

// Success (list)
{"data": [ ... ], "total": 42}

// Error
{"error": {"code": "not_found", "message": "Workspace not found"}}
```

**HTTP status codes:**
| Code | Meaning |
|------|---------|
| 200 | Success (GET, PATCH) |
| 201 | Created (POST) |
| 204 | No Content (DELETE) |
| 400 | Bad Request (validation failure) |
| 404 | Not Found |
| 409 | Conflict (duplicate, state conflict) |
| 500 | Internal Server Error |

### 5.2 Workspace API

```json
// POST /api/workspaces
// Request:
{"name": "my-project"}
// Response (201):
{"data": {"id": "uuid", "name": "my-project", "status": "active", "created_at": "...", "updated_at": "..."}}

// GET /api/workspaces
// Response (200):
{"data": [...], "total": 1}

// GET /api/workspaces/:id
// Response (200):
{"data": {"id": "uuid", "name": "my-project", "status": "active", ...}}

// PATCH /api/workspaces/:id
// Request:
{"name": "renamed-project"}
// Response (200):
{"data": {...}}

// DELETE /api/workspaces/:id
// Response: 204 No Content
```

### 5.3 Session API

```json
// POST /api/workspaces/:wid/sessions
// Request:
{
    "provider_id": "uuid",
    "specialist_id": "crafter",     // optional
    "model": "claude-sonnet-4-5",   // optional, overrides provider default
    "parent_session_id": "uuid"     // optional, for resume
}
// Response (201):
{
    "data": {
        "id": "uuid",
        "workspace_id": "wid",
        "provider_id": "uuid",
        "specialist_id": "crafter",
        "status": "connecting",
        "model": "claude-sonnet-4-5",
        "created_at": "...",
        "updated_at": "..."
    }
}

// POST /api/sessions/:sid/prompt
// Request:
{"message": "Implement the login page"}
// Response (200):
{"data": {"message_id": "uuid"}}

// POST /api/sessions/:sid/cancel
// Response: 204 No Content

// GET /api/sessions/:sid/history?cursor=msg_abc123&limit=50
// cursor: optional, message ID from previous page
// limit: optional, default 50, max 200
// Response (200):
{
    "data": [
        {"id": "uuid", "role": "user", "content": "...", "created_at": "..."},
        {"id": "uuid", "role": "assistant", "content": "...", "created_at": "..."}
    ],
    "has_more": true,
    "next_cursor": "uuid"
}
```

### 5.4 SSE Protocol

All SSE endpoints follow the same event format:

```
event: <event_type>
id: <sequential_id>
data: <json_payload>

```

**Session stream events** (`GET /api/sessions/:sid/stream`):

| Event Type | Payload | When |
|------------|---------|------|
| `connected` | `{"session_id": "..."}` | Client connected |
| `text_delta` | `{"text": "..."}` | Agent text chunk |
| `tool_use_start` | `{"id": "...", "name": "...", "input": {...}}` | Tool call begins |
| `tool_use_delta` | `{"id": "...", "delta": "..."}` | Tool input streaming |
| `tool_result` | `{"id": "...", "result": "..."}` | Tool call completes |
| `thinking` | `{"text": "..."}` | Agent reasoning |
| `done` | `{"stop_reason": "end_turn"}` | Generation complete |
| `error` | `{"message": "..."}` | Error occurred |
| `heartbeat` | `{}` | Keep-alive (every 15s) |

**Kanban stream events** (`GET /api/boards/:bid/stream`):

| Event Type | Payload | When |
|------------|---------|------|
| `task_created` | `{task}` | Card added |
| `task_moved` | `{"task_id": "...", "from_column": "...", "to_column": "..."}` | Card moved |
| `task_updated` | `{task}` | Card modified |
| `task_deleted` | `{"task_id": "..."}` | Card removed |
| `column_added` | `{column}` | Column added |
| `session_started` | `{"task_id": "...", "session_id": "..."}` | Lane automation triggered |
| `heartbeat` | `{}` | Keep-alive (every 15s) |

**Reconnection**: Clients send `Last-Event-ID: <id>` header. Server replays buffered events (last 100 per stream) after that ID.

### 5.5 Provider API

```json
// POST /api/providers
// Request:
{
    "type": "anthropic",
    "name": "My Anthropic Provider",
    "config": {
        "base_url": "https://api.anthropic.com",
        "api_key": "sk-ant-...",
        "default_model": "claude-sonnet-4-5"
    }
}
// Response (201):
{"data": {"id": "uuid", "type": "anthropic", "name": "My Anthropic Provider", "created_at": "..."}}
// Note: api_key is never returned in responses

// GET /api/providers/:id/models
// Response (200):
{
    "data": [
        {"id": "claude-sonnet-4-5", "name": "Claude Sonnet 4.5", "context_window": 200000},
        {"id": "claude-haiku-4-5", "name": "Claude Haiku 4.5", "context_window": 200000}
    ]
}
```

### 5.6 Kanban API

```json
// POST /api/workspaces/:wid/boards
// Request:
{
    "name": "Sprint 1",
    "columns": [
        {"name": "Backlog", "specialist_id": "backlog-refiner", "auto_trigger": true},
        {"name": "To Do", "specialist_id": "todo-orchestrator", "auto_trigger": true},
        {"name": "In Progress", "specialist_id": "dev-crafter", "auto_trigger": true},
        {"name": "Review", "specialist_id": "review-guard", "auto_trigger": true},
        {"name": "Done", "specialist_id": "done-reporter", "auto_trigger": false}
    ]
}

// POST /api/boards/:bid/cards
// Request:
{
    "column_id": "uuid",
    "title": "Implement login page",
    "description": "Build a login form with email/password validation"
}

// PATCH /api/tasks/:tid
// Request (move task):
{
    "column_id": "new-column-uuid",
    "position": 0
}
// When moving to a column with auto_trigger=true and specialist_id set,
// the server automatically creates a session with that specialist.
```

---

## 6. Domain Services

### 6.1 SessionService

The `SessionService` manages the full lifecycle of agent sessions.

```rust
pub struct SessionService {
    store: Arc<SessionStore>,
    provider_registry: Arc<ProviderRegistry>,
    specialist_loader: Arc<SpecialistLoader>,
    tool_registry: Arc<ToolRegistry>,
    trace_collector: Arc<TraceCollector>,
    // Active SSE connections: session_id -> broadcast sender
    streams: Arc<RwLock<HashMap<String, broadcast::Sender<StreamEvent>>>>,
}
```

**Key operations:**

#### Create Session
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

#### Send Prompt
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

#### Cancel Session
```
1. Load session (verify status is not "completed"/"cancelled")
2. Drop the agent stream (abort HTTP request to provider)
3. Update session status to "cancelled"
4. Broadcast cancel event to SSE channel
```

#### Resume Session
```
1. Load parent session's full message history
2. Create new session with parent_session_id set
3. Copy all messages from parent into new session
4. Return new session (user can then send prompts)
```

**Performance note:** Copying all messages is O(n) in message count. For sessions with hundreds of messages (including tool_use/tool_result pairs), this can be slow. Mitigations:
- **v1**: Copy all messages (acceptable for typical sessions of <200 messages)
- **Future**: Chain sessions via `parent_session_id` at query time (view that unions messages from parent chain) to avoid copying. Or copy only the last N messages with a summary of earlier context.

### 6.2 ProviderRegistry

```rust
pub struct ProviderRegistry {
    providers: RwLock<HashMap<String, Arc<dyn CodingAgent>>>,
    store: Arc<ProviderStore>,
}
```

**Behavior:**
- Loaded from SQLite on startup
- `add_provider()`: validates config, creates agent instance, inserts into DB
- `remove_provider()`: removes from map and DB, fails if active sessions reference it
- `get_agent()`: returns `Arc<dyn CodingAgent>` by provider ID
- `health_check_all()`: runs health checks in parallel, returns status map

### 6.3 SpecialistLoader

```rust
pub struct SpecialistLoader {
    specialists: RwLock<HashMap<String, Specialist>>,
    base_dir: PathBuf,
}

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

**Loading:**
```
1. Scan base_dir for *.md files
2. For each file:
   a. Parse YAML frontmatter (between --- delimiters)
   b. Extract body as system prompt
   c. Insert into HashMap keyed by filename (without .md)
3. Watch filesystem for changes (optional, Phase 6)
```

**Frontmatter parsing:**
```yaml
---
name: Dev Crafter
model: sonnet
description: Implements changes within task scope
tool_profile: implementation
tags: [implementation, coding]
---
You are a dev crafter. Your job is to implement changes within the scope of the assigned task.

Rules:
- Stay within task scope
- Run verification before committing
- Commit in small units
```

**Frontmatter fields:**
| Field | Required | Description |
|-------|----------|-------------|
| `name` | Yes | Human-readable specialist name |
| `description` | Yes | One-line description of the specialist's role |
| `model` | No | Preferred model tier (`sonnet`, `haiku`, `opus`) — overrides provider default |
| `tool_profile` | No | Tool profile name (see §7.4.10). Defaults to `full` |
| `tags` | No | Metadata tags for filtering/search |

**Error handling:**
| Scenario | Behavior |
|----------|----------|
| Malformed YAML frontmatter | Log warning, skip file, continue loading others |
| Missing required fields (`name`, `description`) | Log warning, skip file |
| File is not valid UTF-8 | Log warning, skip file |
| Specialist file deleted while session is active | Session continues with the already-loaded system prompt (prompt is copied into session at creation time, not referenced live) |
| Column references non-existent `specialist_id` | `KanbanService.move_task` returns an error; card move is rejected |
| Empty `specialists_dir` | Server starts with zero specialists; kanban automation silently skips columns with bound specialists |

**Startup validation:** On startup, `SpecialistLoader` logs the count of successfully loaded specialists and lists any files that were skipped with reasons. This is a `WARN`-level log, not a fatal error — the system is usable without specialists.

### 6.4 KanbanService

```rust
pub struct KanbanService {
    store: Arc<KanbanStore>,
    session_service: Arc<SessionService>,
    specialist_loader: Arc<SpecialistLoader>,
    // SSE broadcast per board
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

### 6.5 TraceCollector

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

**Journey summary:**
```
GET /api/sessions/:sid/trace/journey:
  1. Load all trace events for session, ordered by timestamp
  2. Filter to: Decision, Milestone, Review events
  3. Return ordered list with timestamps and summaries
```

### 6.6 Agent Execution Model

Each agent session operates within a **workspace context** — a working directory and a set of tools. The agent does not run in a sandbox or container for v1; it executes tools directly on the host filesystem.

**Runtime context per session:**

| Property | Source | Purpose |
|----------|--------|---------|
| `cwd` | Session `cwd` column (nullable) | Working directory for shell_exec, file tools |
| `codebase` | Session → Codebase via `cwd` match | Git context for git_* tools |
| `tools` | Built-in tool set (§7.4) | What the agent can do |
| `system_prompt` | Specialist markdown body | Agent behavior instructions |
| `provider` | Session → Provider | Which LLM to call |

**Lifecycle of a tool call:**
1. Provider returns a `tool_use` content block (name + input JSON)
2. Weave validates the input against the tool's JSON schema
3. Weave executes the tool server-side (file I/O, subprocess, git command)
4. Weave records a trace event (tool name, input, output, duration)
5. Weave sends the `tool_result` back to the provider as the next user message
6. Provider continues generation

**Filesystem access rules:**
- All file paths must be absolute
- Write operations (`file_write`, `file_edit`) are restricted to the session's codebase path
- `..` traversal is rejected — path must resolve within the codebase root
- `shell_exec` runs with the same OS user as the Weave process (no privilege separation in v1)

**No sandboxing in v1.** The agent has the same filesystem and network access as the Weave process. This is acceptable for single-user, local-first deployment. Docker-based isolation is listed as a future enhancement.

---

## 7. Provider Abstraction

### 7.1 The CodingAgent Trait

```rust
#[async_trait]
pub trait CodingAgent: Send + Sync {
    /// Unique provider type identifier (e.g. "anthropic", "openai")
    fn provider_type(&self) -> &str;

    /// Human-readable name
    fn display_name(&self) -> &str;

    /// Available models for this provider
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;

    /// Send a message and stream back the response
    async fn send_message(
        &self,
        request: MessageRequest,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<StreamEvent>>>>>;

    /// Check if the provider is reachable and credentials are valid
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

pub enum Role {
    User,
    Assistant,
}

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

### 7.2 StreamEvent Types

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

### 7.3 AnthropicAgent Implementation

```rust
pub struct AnthropicAgent {
    client: reqwest::Client,
    base_url: String,
    api_key: String,
    default_model: String,
}
```

**Anthropic Messages API streaming:**

The agent sends requests to `{base_url}/v1/messages` with `stream: true`. The response is an SSE stream with these event types:

| Anthropic Event | Maps to StreamEvent |
|-----------------|---------------------|
| `message_start` | (internal: initialize state) |
| `content_block_start` (type: text) | (internal: start text block) |
| `content_block_start` (type: tool_use) | `ToolUseStart { id, name, input }` |
| `content_block_delta` (type: text_delta) | `TextDelta { text }` |
| `content_block_delta` (type: input_json_delta) | `ToolUseDelta { id, delta }` |
| `content_block_stop` | (internal: finalize block) |
| `message_delta` | `Done { stop_reason }` |
| `message_stop` | (internal: stream complete) |
| `ping` | (ignored) |
| `error` | `Error { message }` |

**Request format:**
```json
{
    "model": "claude-sonnet-4-5",
    "max_tokens": 8192,
    "stream": true,
    "system": "You are a dev crafter...",
    "messages": [
        {"role": "user", "content": "Implement the login page"},
        {"role": "assistant", "content": [
            {"type": "text", "text": "I'll implement the login page..."},
            {"type": "tool_use", "id": "toolu_...", "name": "file_write", "input": {...}}
        ]},
        {"role": "user", "content": [
            {"type": "tool_result", "tool_use_id": "toolu_...", "content": "File written successfully"}
        ]}
    ],
    "tools": [
        {
            "name": "file_write",
            "description": "Write content to a file",
            "input_schema": {
                "type": "object",
                "properties": {
                    "path": {"type": "string"},
                    "content": {"type": "string"}
                },
                "required": ["path", "content"]
            }
        }
    ]
}
```

**Error handling:**
| HTTP Status | Meaning | Retry? |
|-------------|---------|--------|
| 400 | Bad request (invalid message format) | No |
| 401 | Invalid API key | No |
| 403 | Permission denied | No |
| 404 | Model not found | No |
| 429 | Rate limited | Yes (with backoff) |
| 500 | Server error | Yes (once) |
| 529 | Overloaded | Yes (with backoff) |

### 7.4 Built-in Tool Definitions

Weave provides tools to agents so they can interact with the workspace. These are injected into every `MessageRequest.tools` alongside any provider-native tools. Tool calls are **executed server-side** by Weave — the provider returns tool_use blocks, Weave executes them, and feeds tool_result back.

Tools are organized into categories. Not all tools are available to all agents — see **Tool Profiles** below.

#### 7.4.1 Filesystem Tools

| Tool | Purpose | Key Input | Output |
|------|---------|-----------|--------|
| `file_read` | Read file contents | `{path: string}` | File content as string |
| `file_write` | Write/overwrite file | `{path: string, content: string}` | Confirmation + bytes written |
| `file_edit` | Patch file (search/replace) | `{path: string, old_string: string, new_string: string}` | Confirmation |
| `search_files` | Grep across files | `{cwd: string, pattern: string, glob?: string}` | Matching lines with context |
| `list_directory` | List directory contents | `{path: string, recursive?: boolean}` | File tree |

#### 7.4.2 Shell Tools

| Tool | Purpose | Key Input | Output |
|------|---------|-----------|--------|
| `shell_exec` | Execute shell command | `{command: string, cwd?: string, timeout_ms?: number}` | stdout + stderr + exit code |

#### 7.4.3 Git Tools

| Tool | Purpose | Key Input | Output |
|------|---------|-----------|--------|
| `git_status` | Working tree status | `{cwd: string}` | Branch, staged/unstaged/untracked files |
| `git_diff` | Diff output | `{cwd: string, staged?: boolean, file?: string}` | Unified diff (truncated at 50KB) |
| `git_log` | Recent commit history | `{cwd: string, count?: number}` | Commit list with hashes and messages |
| `git_commit` | Create commit | `{cwd: string, message: string, stage_all?: boolean}` | Commit hash. Validates git identity — rejects placeholder/test emails |

#### 7.4.4 Task Context Tools

These tools let agents understand their assignment and the broader task context.

| Tool | Purpose | Key Input | Output |
|------|---------|-----------|--------|
| `get_task` | Get current task details | `{task_id?: string}` | Task with title, objective, scope, acceptance criteria, status |
| `list_tasks` | List tasks in workspace | `{status?: string, column_id?: string}` | Task list with assignments and status |
| `update_task_status` | Report task progress | `{task_id: string, status: string}` | Updated task. Statuses: `in_progress`, `review_required`, `completed`, `needs_fix`, `blocked` |
| `update_task_fields` | Update task metadata | `{task_id: string, completion_summary?: string, verification_report?: string}` | Updated task |

#### 7.4.5 Inter-Agent Communication Tools

For multi-agent coordination. Agents can message each other and delegate work.

| Tool | Purpose | Key Input | Output |
|------|---------|-----------|--------|
| `send_message_to_agent` | Send message to another agent | `{agent_id: string, message: string}` | Delivery confirmation |
| `read_agent_conversation` | Read another agent's history | `{agent_id: string, last_n?: number}` | Message list |
| `get_agent_status` | Get agent status | `{agent_id: string}` | Status, message count, active tasks |
| `delegate_task` | Assign task to agent | `{task_id: string, agent_id: string}` | Delegation confirmation |
| `report_to_parent` | Submit completion report | `{task_id: string, summary: string, artifacts?: string[]}` | Acknowledgment |

#### 7.4.6 Kanban Tools

Agents can interact with the kanban board directly.

| Tool | Purpose | Key Input | Output |
|------|---------|-----------|--------|
| `get_board` | Get board with columns and cards | `{board_id: string}` | Full board state |
| `move_card` | Move card between columns | `{task_id: string, column_id: string}` | Updated card. Enforces transition gates |
| `create_card` | Create new card | `{column_id: string, title: string, description?: string}` | Created card |
| `search_cards` | Search cards by criteria | `{query?: string, labels?: string[], assignee?: string}` | Matching cards |

#### 7.4.7 Note/Document Tools

Agents can create and share structured notes for context persistence.

| Tool | Purpose | Key Input | Output |
|------|---------|-----------|--------|
| `create_note` | Create a note | `{title: string, content: string, type?: string}` | Note ID. Types: `spec`, `task`, `general` |
| `read_note` | Read note content | `{note_id: string}` | Note content |
| `list_notes` | List notes | `{type?: string}` | Note list |
| `set_note_content` | Replace note content | `{note_id: string, content: string}` | Updated note |
| `append_to_note` | Append to note | `{note_id: string, content: string}` | Updated note |

#### 7.4.8 Artifact Tools

Agents can request and provide evidence artifacts (screenshots, test results, diffs, logs).

| Tool | Purpose | Key Input | Output |
|------|---------|-----------|--------|
| `request_artifact` | Request artifact from agent | `{task_id: string, type: string, agent_id?: string}` | Request ID. Types: `screenshot`, `test_results`, `code_diff`, `logs` |
| `provide_artifact` | Attach artifact to task | `{task_id: string, type: string, content: string}` | Artifact ID |
| `list_artifacts` | List artifacts for task | `{task_id: string}` | Artifact list |

#### 7.4.9 Web Tools

| Tool | Purpose | Key Input | Output |
|------|---------|-----------|--------|
| `fetch_webpage` | Fetch URL content | `{url: string}` | Stripped text content (truncated at 12KB) |

---

**Tool definition format** (sent to provider):
```json
{
    "name": "file_write",
    "description": "Write content to a file. Creates parent directories if needed. Overwrites existing files.",
    "input_schema": {
        "type": "object",
        "properties": {
            "path": {"type": "string", "description": "Absolute file path"},
            "content": {"type": "string", "description": "File content to write"}
        },
        "required": ["path", "content"]
    }
}
```

**Tool result format** (returned to provider):
```json
// Success
{"success": true, "data": {"bytes_written": 1234, "path": "/src/login.tsx"}}

// Error
{"success": false, "error": "Permission denied: path outside workspace root"}
```

All tool results are JSON-serialized strings in the `tool_result` content block. The `success` field lets the provider distinguish success from failure without parsing the data payload.

---

#### 7.4.10 Tool Profiles

Not all tools are available to all agents. Tool profiles control which tools each specialist can access, preventing low-privilege agents from performing high-risk operations.

**Profile definitions:**

| Profile | Tools | Use Case |
|---------|-------|----------|
| `full` | All tools | Coordinator agents (routa) |
| `implementation` | Filesystem + Shell + Git + Task context | Implementor agents (crafter, dev-crafter) |
| `review` | Filesystem (read-only) + Git + Task context + Artifacts | Verifier agents (gate, review-guard) |
| `planning` | Task context + Kanban + Notes (no filesystem/shell) | Planning agents (backlog-refiner, todo-orchestrator) |
| `reporting` | Task context (read-only) + Notes + Artifacts | Reporting agents (done-reporter) |

**Implementation:** Each specialist's frontmatter can specify a `tool_profile` field:

```yaml
---
name: Dev Crafter
model: sonnet
description: Implements changes within task scope
tool_profile: implementation
---
```

When building the `MessageRequest.tools` list, `SessionService` filters the full tool inventory by the specialist's profile. If no profile is specified, defaults to `full`.

---

#### 7.4.11 Execution Model

```
Provider returns: tool_use { id: "toolu_...", name: "file_write", input: {path, content} }
         │
         ▼
Weave ToolExecutor validates input against JSON schema
         │
         ▼
Weave checks tool is allowed by the session's tool profile
         │
         ▼
Weave executes tool server-side (write file, run command, etc.)
         │
         ▼
Weave records trace event (tool name, input, output, duration_ms)
         │
         ▼
Weave sends tool_result back to provider: { tool_use_id: "toolu_...", content: "..." }
         │
         ▼
Provider continues generation
```

**Rust trait for tool execution:**

```rust
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Tool name (matches the name sent to the provider)
    fn name(&self) -> &str;

    /// JSON schema for input validation
    fn input_schema(&self) -> serde_json::Value;

    /// Execute the tool and return the result
    async fn execute(&self, input: serde_json::Value, context: &ToolContext) -> ToolResult;
}

pub struct ToolContext {
    pub session_id: String,
    pub cwd: PathBuf,              // session working directory
    pub codebase_root: PathBuf,    // codebase root (for path containment)
    pub trace_collector: Arc<TraceCollector>,
}

pub struct ToolResult {
    pub success: bool,
    pub data: serde_json::Value,
    pub error: Option<String>,
}
```

**Tool registration:** All tools are registered in a `ToolRegistry` at startup:

```rust
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn ToolExecutor>>,
    profiles: HashMap<String, Vec<String>>,  // profile_name -> tool_names
}
```

The registry is built once in `main.rs` and shared via `AppState`. When `SessionService` builds a request, it queries the registry for the specialist's profile and filters tools accordingly.

---

#### 7.4.12 Security Constraints

**Path containment:**
- All file paths must be absolute
- Write operations (`file_write`, `file_edit`) are restricted to the session's codebase path
- `..` traversal is rejected — path must resolve within the codebase root
- `shell_exec` runs with the same OS user as the Weave process (no privilege separation in v1)

**Control-plane protection:**
The following paths are read-only and cannot be modified by agents:
- `.git/config`, `.git/hooks/`, `.git/HEAD`
- `weave.db`, `weave.db-wal`, `weave.db-shm`
- `resources/specialists/*.md` (specialist definitions)
- `Cargo.toml`, `Cargo.lock` (build config)

Agents attempting to write to these paths receive a `ToolResult { success: false, error: "Path is protected" }` and a trace event is logged.

**Git identity validation:**
`git_commit` rejects placeholder/test identities (e.g., `test@example.com`, `placeholder`). Requires real `user.name` and `user.email` configured in the repository.

**Kanban transition gates:**
`move_card` enforces:
- Required artifacts must be attached before moving to review/done columns
- Required task fields (acceptance criteria, verification report) must be populated
- Description is frozen from dev stage onward (cannot be changed via `update_task_fields`)
- After repeated failures on the same column transition, the card is labeled `contract-gate-blocked`

**Audit trail:**
All tool executions are recorded as trace events with:
- Tool name
- Input (sanitized — API keys and secrets stripped)
- Output (truncated to 10KB)
- Duration in milliseconds
- Success/failure status

---

## 8. SSE Implementation

### 8.1 Server-Side Architecture

```rust
pub struct SseManager {
    // Per-entity broadcast channels
    session_streams: RwLock<HashMap<String, broadcast::Sender<ServerEvent>>>,
    board_streams: RwLock<HashMap<String, broadcast::Sender<KanbanEvent>>>,
}

impl SseManager {
    /// Create a new stream for an entity. Returns the sender side.
    pub fn create_stream(&self, entity_id: &str) -> broadcast::Sender<ServerEvent> {
        let (tx, _) = broadcast::channel(256);
        self.session_streams.write().insert(entity_id.to_string(), tx.clone());
        tx
    }

    /// Subscribe to an entity's stream. Returns receiver.
    pub fn subscribe(&self, entity_id: &str) -> Option<broadcast::Receiver<ServerEvent>> {
        self.session_streams.read().get(entity_id).map(|tx| tx.subscribe())
    }

    /// Broadcast an event to all subscribers.
    pub fn broadcast(&self, entity_id: &str, event: ServerEvent) {
        if let Some(tx) = self.session_streams.read().get(entity_id) {
            let _ = tx.send(event); // Ignore if no receivers
        }
    }
}
```

### 8.2 SSE Endpoint Handler

```rust
async fn session_stream(
    Path(session_id): Path<String>,
    headers: HeaderMap,
    State(state): State<AppState>,
) -> impl IntoResponse {
    // 1. Verify session exists
    // 2. Get or create broadcast channel
    // 3. If Last-Event-ID header present, replay buffered events
    // 4. Return SSE response stream

    let last_event_id = headers
        .get("Last-Event-ID")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());

    let receiver = state.sse_manager.subscribe(&session_id);

    Sse::new(stream! {
        // Send connected event
        yield Event::default()
            .event("connected")
            .data(json!({"session_id": session_id}).to_string())
            .id("0");

        // Replay missed events if reconnecting
        if let Some(last_id) = last_event_id {
            for event in state.event_buffer.get_after(&session_id, last_id) {
                yield event.to_sse_event();
            }
        }

        // Stream live events
        while let Ok(event) = receiver.recv().await {
            yield event.to_sse_event();
        }
    })
    .keep_alive(KeepAlive::new(Duration::from_secs(15)))
}
```

### 8.3 Event Buffer

```rust
pub struct EventBuffer {
    buffers: RwLock<HashMap<String, VecDeque<BufferedEvent>>>,
    max_size: usize,  // 100 events per entity
}

struct BufferedEvent {
    id: u64,
    event_type: String,
    data: String,
}
```

Each entity (session, board) maintains a ring buffer of the last 100 events. On reconnection, events after `Last-Event-ID` are replayed from this buffer.

### 8.4 Connection Lifecycle and Failure Behavior

**Server restart:**
- All SSE connections drop. In-memory event buffers are lost.
- Active agent sessions are cancelled (graceful shutdown, §12.4).
- On reconnect, clients receive a `connected` event with no replay (buffer was lost).
- The frontend detects the reconnect (new event stream) and refetches session state via REST.

**Client disconnect:**
- The SSE stream generator exits. No cleanup needed — broadcast channels auto-drop subscribers.
- The agent session continues running (the spawned tokio task is independent of the SSE connection).
- The client can reconnect later and resume receiving events from the buffer.

**Backpressure:**
- `broadcast::channel(256)` — if a subscriber is slow, the oldest unread events are dropped (`RecvError::Lagged`).
- The SSE handler detects lag and sends a `gap` event: `{"event": "gap", "missed": N}`.
- The frontend responds by refetching session state via REST to re-sync.

**Max concurrent SSE connections:** No hard limit in v1. The bottleneck is OS file descriptors (default 1024). For production, set `ulimit -n 4096` or higher. Each SSE connection holds one TCP socket and one tokio task.

---

## 9. Kanban Lane Automation

### 9.1 Flow Diagram

```
User moves card to "In Progress" column
         │
         ▼
KanbanService.move_task(task_id, column_id)
         │
         ▼
    ┌────┴────┐
    │ Update  │
    │ task in │
    │   DB    │
    └────┬────┘
         │
         ▼
    ┌────┴────────────────────┐
    │ Broadcast task_moved    │
    │ event to board stream   │
    └────┬────────────────────┘
         │
         ▼
    ┌────┴────────────────────────┐
    │ Column has auto_trigger?    │
    │ AND specialist_id set?      │
    └────┬──────────┬─────────────┘
         │          │
        Yes         No → Done
         │
         ▼
    ┌────┴────────────────────┐
    │ Load specialist by ID   │
    │ from SpecialistLoader   │
    └────┬────────────────────┘
         │
         ▼
    ┌────┴────────────────────┐
    │ SessionService.create(  │
    │   specialist_id,        │
    │   initial_prompt:       │
    │   "Process task: ..."   │
    │ )                       │
    └────┬────────────────────┘
         │
         ▼
    ┌────┴────────────────────┐
    │ Update task.session_id  │
    │ Broadcast session_started│
    └────┬────────────────────┘
         │
         ▼
    Agent processes task autonomously
    (streaming events to session SSE)
```

### 9.2 Default Board Template

When creating a board, users can specify columns. A default template:

| Position | Name | Specialist | Auto-trigger |
|----------|------|-----------|--------------|
| 0 | Backlog | backlog-refiner | true |
| 1 | To Do | todo-orchestrator | true |
| 2 | In Progress | dev-crafter | true |
| 3 | Review | review-guard | true |
| 4 | Done | done-reporter | false |

---

## 10. Session State Machine

```
                    ┌──────────────┐
                    │  connecting  │
                    └──────┬───────┘
                           │ first message sent/received
                           ▼
                    ┌──────────────┐
          ┌────────│    ready     │────────┐
          │        └──────┬───────┘        │
          │               │                │
    cancel│         done  │          error │
          │               │                │
          ▼               ▼                ▼
    ┌──────────┐  ┌──────────────┐  ┌──────────┐
    │ cancelled│  │  completed   │  │  error   │
    └──────────┘  └──────────────┘  └──────────┘
```

**Transitions:**
| From | To | Trigger |
|------|-----|---------|
| connecting | ready | First successful message exchange |
| ready | completed | Agent returns `stop_reason: end_turn` |
| ready | cancelled | User cancels or session times out |
| ready | error | Provider error (non-retryable) |
| ready | ready | New prompt sent (stays ready) |

**Timeout**: Sessions with no activity for 30 minutes are auto-completed.

---

## 11. Error Handling

### 11.1 Error Types

```rust
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("Not found: {resource} {id}")]
    NotFound { resource: String, id: String },

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Provider error: {0}")]
    Provider(#[from] ProviderError),

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    #[error("Authentication failed")]
    AuthFailed,
    #[error("Rate limited, retry after {retry_after_ms}ms")]
    RateLimited { retry_after_ms: u64 },
    #[error("Model not found: {model}")]
    ModelNotFound { model: String },
    #[error("Provider unreachable: {0}")]
    Unreachable(String),
    #[error("Stream interrupted: {0}")]
    StreamInterrupted(String),
}
```

### 11.2 Error-to-HTTP Mapping

```rust
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code, message) = match &self {
            AppError::NotFound { .. } => (404, "not_found", self.to_string()),
            AppError::Validation(_) => (400, "validation_error", self.to_string()),
            AppError::Provider(ProviderError::AuthFailed) => (401, "auth_failed", self.to_string()),
            AppError::Provider(ProviderError::RateLimited { .. }) => (429, "rate_limited", self.to_string()),
            AppError::Provider(_) => (502, "provider_error", self.to_string()),
            AppError::Database(_) => (500, "internal_error", "Internal server error".into()),
            AppError::Internal(_) => (500, "internal_error", "Internal server error".into()),
        };
        (status, Json(json!({"error": {"code": code, "message": message}}))).into_response()
    }
}
```

### 11.3 Retry Strategy for Provider Calls

```rust
async fn with_retry<F, T, E>(f: F, max_retries: u32) -> Result<T, E>
where
    F: Fn() -> Pin<Box<dyn Future<Output = Result<T, E>>>>,
    E: IsRetryable,
{
    let mut attempts = 0;
    loop {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) if e.is_retryable() && attempts < max_retries => {
                attempts += 1;
                let delay = Duration::from_millis(1000 * 2u64.pow(attempts));
                tokio::time::sleep(delay).await;
            }
            Err(e) => return Err(e),
        }
    }
}
```

Retryable errors: rate limits (429), server errors (500), overloaded (529), network timeouts.
Non-retryable: auth failures (401/403), bad requests (400), model not found (404).

---

## 12. Concurrency Model

### 12.1 Shared State

```rust
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,        // Single connection (WAL allows concurrent reads)
    pub session_service: Arc<SessionService>,
    pub kanban_service: Arc<KanbanService>,
    pub provider_registry: Arc<ProviderRegistry>,
    pub specialist_loader: Arc<SpecialistLoader>,
    pub tool_registry: Arc<ToolRegistry>,
    pub trace_collector: Arc<TraceCollector>,
    pub sse_manager: Arc<SseManager>,
    pub config: Arc<Config>,
}
```

### 12.2 Database Access

SQLite with WAL mode supports:
- **Concurrent reads** from multiple threads
- **Single writer** at a time (writes queue behind a busy timeout)

Strategy: `Arc<Mutex<Connection>>` with a 5-second busy timeout. All writes acquire the mutex. Reads also go through the mutex for simplicity (SQLite read performance is not a bottleneck for this workload).

**Alternative considered**: Connection pool (r2d2). Rejected because SQLite WAL mode doesn't benefit from multiple connections — writes are serialized anyway, and reads are already fast.

### 12.3 Async Task Spawning

When a user sends a prompt to a session, the actual agent communication happens in a spawned tokio task:

```rust
pub async fn send_prompt(&self, session_id: &str, message: &str) -> Result<String> {
    // ... validation and setup ...

    let session_service = self.clone();
    let session_id = session_id.to_string();
    let message = message.to_string();

    tokio::spawn(async move {
        if let Err(e) = session_service.run_agent_loop(&session_id, &message).await {
            tracing::error!("Agent loop failed for session {}: {}", session_id, e);
            session_service.handle_error(&session_id, e).await;
        }
    });

    Ok(message_id)
}
```

The `run_agent_loop` streams events from the provider, broadcasts them via SSE, and persists them to the database. The HTTP request returns immediately with the message ID.

### 12.4 Graceful Shutdown

```rust
tokio::select! {
    _ = signal::ctrl_c() => {
        tracing::info!("Shutdown signal received");
    }
    _ = server_handle => {
        tracing::info!("Server exited");
    }
}

// Graceful shutdown:
// 1. Stop accepting new connections
// 2. Wait for in-flight requests to complete (timeout: 30s)
// 3. Cancel all active agent sessions
// 4. Flush database WAL
// 5. Exit
```

---

## 13. Frontend Architecture

### 13.1 Routing

```tsx
const router = createBrowserRouter([
    { path: "/", element: <Home /> },
    { path: "/workspaces/:wid", element: <WorkspaceLayout />, children: [
        { index: true, element: <Overview /> },
        { path: "sessions", element: <Sessions /> },
        { path: "sessions/:sid", element: <Session /> },
        { path: "kanban", element: <Kanban /> },
        { path: "codebases", element: <Codebases /> },
    ]},
    { path: "/settings", element: <Settings /> },
]);
```

### 13.2 Key Hooks

#### useSession (Chat + SSE)
```typescript
function useSession(sessionId: string) {
    // TanStack Query for initial data
    const { data: session } = useQuery({ queryKey: ["session", sessionId], ... });
    const { data: history } = useQuery({ queryKey: ["session", sessionId, "history"], ... });

    // SSE connection for real-time updates
    const [events, setEvents] = useState<StreamEvent[]>([]);

    useEffect(() => {
        const es = new EventSource(`/api/sessions/${sessionId}/stream`);

        es.addEventListener("text_delta", (e) => {
            const data = JSON.parse(e.data);
            setEvents(prev => [...prev, { type: "text_delta", ...data }]);
        });

        es.addEventListener("done", () => {
            queryClient.invalidateQueries(["session", sessionId, "history"]);
        });

        return () => es.close();
    }, [sessionId]);

    // Prompt mutation
    const prompt = useMutation({
        mutationFn: (message: string) =>
            api.post(`/api/sessions/${sessionId}/prompt`, { message }),
    });

    return { session, history, events, prompt };
}
```

#### useKanban (Board + SSE)
```typescript
function useKanban(boardId: string) {
    const { data: board } = useQuery({ queryKey: ["board", boardId], ... });

    useEffect(() => {
        const es = new EventSource(`/api/boards/${boardId}/stream`);
        // Listen for task_moved, task_created, session_started, etc.
        // Invalidate queries on changes
        return () => es.close();
    }, [boardId]);

    const moveTask = useMutation({
        mutationFn: ({ taskId, columnId, position }) =>
            api.patch(`/api/tasks/${taskId}`, { column_id: columnId, position }),
    });

    return { board, moveTask };
}
```

### 13.3 Chat View Component Tree

```
SessionPage
├── SessionHeader (session status, specialist badge, cancel button)
├── MessageList
│   ├── MessageBubble (user)
│   │   └── MarkdownRenderer
│   ├── MessageBubble (assistant)
│   │   ├── MarkdownRenderer
│   │   └── ToolCallBlock (expandable)
│   │       ├── ToolCallHeader (tool name, duration)
│   │       └── ToolCallBody (input JSON, output)
│   └── StreamingIndicator (when agent is thinking)
├── JourneySidebar (collapsible)
│   ├── JourneyTimeline
│   │   └── DecisionNode (timestamp, summary, expandable)
│   └── FileChangesList
│       └── FileChangeItem (path, action badge)
└── MessageInput
    ├── Textarea (auto-resize)
    └── SendButton
```

### 13.4 Kanban View Component Tree

```
KanbanPage
├── BoardHeader (board name, add column button)
├── BoardContainer (horizontal scroll)
│   ├── Column
│   │   ├── ColumnHeader (name, specialist badge, auto-trigger toggle)
│   │   ├── CardList (drag-and-drop)
│   │   │   └── Card
│   │   │       ├── CardTitle
│   │   │       ├── CardStatus (badge: idle, agent working, done)
│   │   │       └── CardAgentIndicator (if session active)
│   │   └── AddCardButton
│   └── AddColumnButton
└── TaskDetailPanel (slide-over when card clicked)
    ├── TaskTitle (editable)
    ├── TaskDescription (editable)
    ├── TaskSession (link to session if agent is working)
    └── TaskHistory (trace summary)
```

---

## 14. Binary Embedding

### 14.1 Frontend Asset Embedding

The Rust binary embeds the built frontend assets using `build.rs`:

```rust
// build.rs
fn main() {
    // Build frontend
    let output = Command::new("npm")
        .args(["run", "build"])
        .current_dir("../web")
        .output()
        .expect("Failed to build frontend");

    if !output.status.success() {
        panic!("Frontend build failed: {}", String::from_utf8_lossy(&output.stderr));
    }

    // Tell cargo to re-run if frontend files change
    println!("cargo:rerun-if-changed=../web/src");
    println!("cargo:rerun-if-changed=../web/package.json");
}
```

At runtime, assets are served via `tower-http::services::ServeDir`:

```rust
let frontend_assets = ServeDir::new("../web/dist")
    .not_found_service(ServeFile::new("../web/dist/index.html")); // SPA fallback

let app = Router::new()
    .nest("/api", api_routes)
    .fallback_service(frontend_assets);
```

**Production embedding** (optional): Use `include_dir!` to embed the `dist/` directory into the binary for a truly single-file deployment.

---

## 15. Testing Strategy

### 15.1 Unit Tests

- **Store layer**: In-memory SQLite (`:memory:`) for each test, verify CRUD operations
- **Domain services**: Mock stores, verify business logic and state transitions
- **Agent layer**: Mock HTTP responses, verify stream event parsing

### 15.2 Integration Tests

- **API routes**: Use `axum::test` helpers, verify request/response contracts
- **SSE streaming**: Verify event ordering, reconnection, heartbeat
- **Kanban automation**: Verify column transition triggers session creation

### 15.3 End-to-End Tests

- **Session lifecycle**: Create session → send prompt → receive streamed response → complete
- **Kanban flow**: Create board → add card → move to column → verify agent session created

---

## 16. Dependencies

### Rust Crate Dependencies

| Crate | Purpose |
|-------|---------|
| `axum` | HTTP framework |
| `tokio` | Async runtime |
| `rusqlite` | SQLite driver (with `bundled` feature) |
| `serde` / `serde_json` | Serialization |
| `uuid` | ID generation |
| `chrono` | Timestamps |
| `tracing` / `tracing-subscriber` | Logging |
| `reqwest` | HTTP client for provider APIs |
| `tokio-stream` | SSE streaming utilities |
| `tower` / `tower-http` | Middleware, static file serving |
| `anyhow` / `thiserror` | Error handling |
| `clap` | CLI argument parsing |
| `serde_yaml` | Specialist frontmatter parsing |
| `notify` | Filesystem watching (Phase 6) |

### Frontend Dependencies

| Package | Purpose |
|---------|---------|
| `react` / `react-dom` | UI framework |
| `react-router` | Client-side routing |
| `@tanstack/react-query` | Data fetching + caching |
| `tailwindcss` | Styling |
| `@dnd-kit/core` | Drag-and-drop for kanban |
| `marked` / `react-markdown` | Markdown rendering |
| `zod` | Client-side validation |

---

## 17. Security Considerations

### 17.1 API Key Storage

Provider API keys are stored in SQLite in the `config_json` column. For v1, this is plaintext. Future: encrypt at rest with a master key derived from a user-provided passphrase.

API keys are **never** returned in API responses. The `GET /api/providers` endpoint strips the `api_key` field from the config.

### 17.2 Input Validation

All user input is validated before processing:
- Workspace names: 1-100 characters, no control characters
- Session prompts: max 100KB
- Task titles: 1-500 characters
- File paths: must be absolute, no `..` traversal

### 17.3 CORS

CORS is configured to allow only same-origin requests (the frontend is served from the same binary). No cross-origin API access.

### 17.4 Rate Limiting

Provider API rate limits are handled with exponential backoff. The platform itself does not rate-limit user requests for v1 (single-user assumption).

### 17.5 Threat Model (v1)

**Assumptions:**
- Single user on a local machine
- Binds to `127.0.0.1` by default (localhost only)
- No authentication layer

**Risks and mitigations:**

| Risk | Severity | Mitigation |
|------|----------|------------|
| Binding to `0.0.0.0` exposes API to network | HIGH | Log a `WARN` on startup if host is not `127.0.0.1`. Require `--allow-remote` flag to bind to non-localhost. |
| API key stored in plaintext in SQLite | MEDIUM | File permissions on `weave.db` should be `600`. Document this. Future: encrypt at rest. |
| Agent executes arbitrary shell commands | MEDIUM | No sandboxing in v1. The agent has same OS user permissions as the Weave process. Document this clearly. |
| No CSRF protection | LOW | Same-origin only (CORS). SPA + same-origin = no CSRF vector. |
| No rate limiting on Weave API | LOW | Single-user assumption. If exposed to network, add rate limiting middleware. |

**Future auth options** (out of scope for v1):
- API key in `Authorization` header
- Session-based auth with login page
- Mutual TLS for network exposure

---

## 18. Performance Considerations

### 18.1 Database

- WAL mode allows concurrent reads during writes
- Indexes on foreign keys and frequently queried columns
- Messages are stored as JSON text (no normalization) — acceptable for expected data volumes
- Trace events are append-heavy, read-light — optimized for write throughput

### 18.2 SSE

- `broadcast::channel(256)` per entity — backpressure drops old events if subscriber is slow
- Event buffer limited to 100 events per entity — prevents unbounded memory growth
- Heartbeat every 15 seconds — keeps connections alive through proxies

### 18.3 Frontend

- TanStack Query caches API responses — avoids redundant fetches
- SSE events update cache incrementally — no full refetch on every event
- Code splitting per route — initial load only loads home page bundle

---

## 19. Database Operations

### 19.1 Backup

SQLite is a single file, but backing up while the server is running requires care:

```
# Safe backup (WAL-aware):
sqlite3 weave.db "PRAGMA wal_checkpoint(TRUNCATE);"
cp weave.db weave.db.backup

# Or use the SQLite Online Backup API (preferred):
# rusqlite supports this via backup::Backup
```

**Recommendation**: Add a `POST /api/admin/backup` endpoint (Phase 6) that triggers a WAL checkpoint and copies the database file to a user-specified path. For v1, document the manual `sqlite3` + `cp` approach in the README.

### 19.2 Migration to New Machine

1. Stop the Weave server
2. Copy `weave.db` (and `weave.db-wal`, `weave.db-shm` if they exist) to the new machine
3. Start Weave on the new machine — it runs migrations automatically

**Caveat**: Provider `config_json` may contain absolute paths or host-specific config. Document that providers may need reconfiguration after migration.

---

## 20. Logging and Observability

### 20.1 Structured Logging

Weave uses the `tracing` crate for structured logging. The tracing subscriber is configured at startup:

```rust
tracing_subscriber::fmt()
    .with_env_filter(
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("weave=info,tower_http=info"))
    )
    .with_target(true)
    .with_thread_ids(true)
    .json()  // JSON format for production
    .init();
```

**Log levels:**
| Level | What |
|-------|------|
| `ERROR` | Provider failures, database errors, unrecoverable state |
| `WARN` | Skipped specialist files, slow queries (>100ms), SSE lag |
| `INFO` | Server startup/shutdown, session created/completed, provider added |
| `DEBUG` | Tool executions, message saves, SSE events |
| `TRACE` | Raw HTTP requests/responses, SQL queries |

**Configuration**: Set `RUST_LOG=weave=debug` for development. Default is `info`.

### 20.2 Health Check Details

`GET /api/health` returns:
```json
{
    "status": "ok",
    "version": "0.1.0",
    "uptime_seconds": 3600,
    "providers": {
        "total": 2,
        "healthy": 1,
        "unhealthy": 1
    },
    "active_sessions": 3,
    "database": {
        "size_bytes": 1048576,
        "wal_checkpoint_pending": false
    }
}
```

---

## 21. Frontend State Management

### 21.1 SSE → Cache Sync Strategy

SSE events do **not** trigger full refetches. Instead, they update the TanStack Query cache incrementally:

```typescript
// On text_delta: append to the optimistic message buffer (local state)
// On done: invalidate ["session", id, "history"] to refetch final state
// On task_moved: update the board query cache in-place
// On session_started: invalidate ["board", id] to pick up new session_id on task

es.addEventListener("task_moved", (e) => {
    const { task_id, from_column, to_column } = JSON.parse(e.data);
    queryClient.setQueryData(["board", boardId], (old) => {
        // Move task in cached board data without refetching
        return moveTaskInCache(old, task_id, to_column);
    });
});
```

**When to invalidate vs patch:**
| Event | Strategy | Reason |
|-------|----------|--------|
| `text_delta` | Local state only | High frequency, no cache update needed |
| `done` | Invalidate history | Final state needs full message with metadata |
| `task_moved` | Patch cache | Single field change, cheap to apply |
| `task_created` | Patch cache | Add item to column's card list |
| `session_started` | Invalidate board | Task now has session_id, may need specialist info |

### 21.2 Error States

Each page/hook handles errors at the query level:

```typescript
const { data, error, isLoading } = useQuery({ ... });

if (isLoading) return <Skeleton />;
if (error) return <ErrorBanner message={error.message} retry={() => refetch()} />;
```

**SSE error handling:**
```typescript
es.onerror = () => {
    // EventSource auto-reconnects by default
    // After 3 failed reconnects, show connection-lost banner
    setConnectionStatus("disconnected");
};
```

### 21.3 Loading States

| Component | Loading State |
|-----------|--------------|
| Session page | Skeleton message bubbles (3 placeholders) |
| Kanban board | Skeleton columns with card placeholders |
| Message history | Spinner at top (loading older messages) |
| Tool call block | Collapsed by default, expandable spinner while streaming |

---

## 22. What Was Intentionally Dropped

| Feature | Reason |
|---------|--------|
| WebSocket | SSE is simpler, sufficient for unidirectional streaming |
| JSON-RPC | REST is simpler, well-understood |
| ACP protocol | Direct HTTP to provider APIs is simpler |
| Process spawning | No child process management needed |
| Docker isolation | Out of scope for v1 |
| Postgres support | SQLite is sufficient, zero configuration |
| Desktop mode (Tauri) | Web-only eliminates dual-origin complexity |
| macOS/Windows | Linux-first, no platform conditionals |
| GitHub integration | Out of scope |
| MCP server mode | Out of scope |
| A2A / AG-UI protocols | Out of scope |
| Webhooks / schedules | Out of scope |
| Shared sessions | Too complex for v1 |
| Office document rendering | Out of scope |
| i18n | English only for v1 |
| Multiple concurrent providers | One active provider per session is sufficient |
