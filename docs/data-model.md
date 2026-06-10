# Data Model

## Entity-Relationship Diagram

```
Workspace (1) ─────┬── (N) Session
                    ├── (N) Codebase
                    └── (N) Board

Provider (*)        Global — not workspace-scoped
Specialist (*)      Filesystem-loaded — not in DB

Session (1) ────────┬── (N) Message
                    ├── (N) TraceEvent
                    └── (N) FileChange

Session (N) ──────── (0..1) Session  (parent_session_id for resume)

Board (1) ──────────┬── (N) Column
                    └── (N) Task

Column (1) ───────── (N) Task
Column (N) ───────── (0..1) Specialist (specialist_id binding, filesystem reference)

Task (N) ─────────── (0..1) Session (session_id when agent is working on it)

Codebase (1) ─────── (N) Worktree  (reserved for v2)
```

**Scope rules:**
- **Providers** are global — a single provider can serve sessions across all workspaces. The `/api/providers` endpoints are not workspace-scoped.
- **Specialists** are filesystem resources (markdown files), not DB entities. Columns reference them by string ID.
- **Worktrees** are defined in the schema but have no API surface in v1. The table is reserved for future git worktree isolation.

## Schema Definitions

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

-- Messages (immutable — no updated_at column)
CREATE TABLE messages (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role        TEXT NOT NULL,             -- user | assistant | tool_use | tool_result
    content     TEXT NOT NULL,             -- JSON-encoded content
    metadata    TEXT,                      -- JSON-encoded extra data
    created_at  TEXT NOT NULL
);
CREATE INDEX idx_messages_session ON messages(session_id);

-- Providers
CREATE TABLE providers (
    id              TEXT PRIMARY KEY,
    type            TEXT NOT NULL,         -- vendor: anthropic | openai | local | cli
    kind            TEXT NOT NULL DEFAULT 'http',  -- transport: http | cli (feat-039)
    name            TEXT NOT NULL,
    default_model   TEXT,                  -- canonical wire field, both kinds (feat-039)
    binary_path     TEXT,                  -- cli rows only (feat-039)
    args_json       TEXT,                  -- JSON Vec<String>, cli rows only (feat-039)
    env_json        TEXT,                  -- JSON BTreeMap<String,String>, cli rows only (feat-039)
    permission_mode TEXT,                  -- cli rows only (feat-039)
    config_json     TEXT NOT NULL,         -- {base_url, api_key, default_model, ...}
    created_at      TEXT NOT NULL
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

-- Worktrees (reserved for v2 — no API/domain code in v1)
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

-- Traces (immutable append-only — no updated_at)
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

## ID Generation

All IDs are UUID v4 strings (`uuid::Uuid::new_v4().to_string()`). No auto-increment, no sequential IDs. This avoids:
- ID enumeration attacks
- Cross-database conflicts if SQLite files are copied
- Coordination overhead in concurrent inserts

## Timestamps

All timestamps are ISO 8601 strings stored as TEXT (`chrono::Utc::now().to_rfc3339()`). SQLite has no native datetime type; TEXT with ISO format is sortable and unambiguous.

## Message Content Format

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

**Storage rule**: Content is stored exactly as received from the provider (normalized to a JSON array of blocks). The `metadata` column holds session-level annotations (e.g., `{"model": "claude-sonnet-4-5", "tokens_used": 1234}`).

## Pagination

All list endpoints accept optional query parameters:

```
GET /api/workspaces?page=1&limit=50
GET /api/workspaces/:wid/sessions?page=1&limit=20
GET /api/sessions/:sid/history?cursor=msg_abc123&limit=50
```

**Two strategies:**
- **Offset-based** (`page` + `limit`): Used for small, bounded collections (workspaces, sessions, boards). Default `limit=50`, max `100`.
- **Cursor-based** (`cursor` + `limit`): Used for append-only, potentially large collections (messages, traces). The cursor is the `id` of the last item from the previous page.

**Response envelope:**
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
