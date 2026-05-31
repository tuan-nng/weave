# Weave — Architecture

## What Is Weave?

Weave is a **web-based multi-agent coordination platform**. It orchestrates AI coding agents to perform tasks like code generation, review, and kanban-driven automation. It is not a chatbot — it is an execution platform where agent sessions are managed, streamed, and observed through structured workflows.

## Design Principles

1. **Single backend** — Rust/Axum binary serves both the API and bundled web UI. No Node.js runtime dependency.
2. **Trait-based provider abstraction** — `CodingAgent` trait with Anthropic as first implementation. Adding new agents = implement trait + register.
3. **Web-only** — no Tauri, no desktop shell, no dual-origin navigation.
4. **Linux-first** — no macOS/Windows platform conditionals.
5. **SQLite-only** — one file, one path, zero configuration.
6. **Streaming-native** — SSE for all real-time updates. No polling, no WebSocket, no JSON-RPC gateway.
7. **Workspace-scoped** — every resource lives under a workspace.
8. **Small API surface** — ~25 endpoints, not 170+.

## Runtime Topology

```
┌─────────────────────────────────────────────────┐
│                   Browser                        │
│  React SPA (served from Rust binary)             │
│  ┌───────────┐ ┌───────────┐ ┌───────────┐     │
│  │ Sessions  │ │  Kanban   │ │ Settings  │     │
│  │  (chat)   │ │  (board)  │ │(providers)│     │
│  └─────┬─────┘ └─────┬─────┘ └─────┬─────┘     │
│        │              │              │           │
│        └──────────────┼──────────────┘           │
│                       │ REST + SSE               │
└───────────────────────┼──────────────────────────┘
                        │
┌───────────────────────┼──────────────────────────┐
│                Weave Binary                       │
│                       │                           │
│  ┌────────────────────┴────────────────────┐     │
│  │            Axum HTTP Server              │     │
│  │  /api/workspaces  /api/sessions          │     │
│  │  /api/providers   /api/boards            │     │
│  │  /api/health      /api/tasks             │     │
│  └────────────────────┬────────────────────┘     │
│                       │                           │
│  ┌────────────────────┴────────────────────┐     │
│  │           Domain Services                │     │
│  │  SessionService  KanbanService           │     │
│  │  ProviderRegistry  SpecialistLoader      │     │
│  └─────────┬──────────────┬────────────────┘     │
│            │              │                       │
│  ┌─────────┴──────┐ ┌────┴──────────────────┐   │
│  │  SQLite Store  │ │  CodingAgent Trait     │   │
│  │  (rusqlite)    │ │  ┌──────────────────┐ │   │
│  │                │ │  │  AnthropicAgent  │ │   │
│  │                │ │  │  (HTTP + SSE)    │ │   │
│  │                │ │  └────────┬─────────┘ │   │
│  │                │ │           │            │   │
│  │                │ │  ┌────────┴─────────┐ │   │
│  │                │ │  │  Future Agents   │ │   │
│  │                │ │  │  (OpenAI, local) │ │   │
│  │                │ │  └──────────────────┘ │   │
│  └────────────────┘ └───────────────────────┘   │
│                                                  │
└──────────────────────────────────────────────────┘
                        │
                        │ HTTPS
                        ▼
               ┌─────────────────┐
               │  Anthropic API  │
               │  (or proxy)     │
               └─────────────────┘
```

## Domain Model

```
Workspace
├── Sessions (agent execution threads)
│   ├── Messages (user + assistant + tool_use + tool_result)
│   └── Traces (tool calls, file changes, timing)
├── Codebases (git repo identity: path, branch, label)
│   └── Worktrees (ephemeral execution copies)
├── Tasks (work units)
│   └── Kanban Boards
│       ├── Columns (status lanes)
│       └── Cards (task instances in columns)
├── Providers (CodingAgent instances with config)
└── Specialists (agent role definitions — markdown + YAML)
```

### Workspace

Top-level coordination boundary. All resources are workspace-scoped. A `"default"` workspace exists for quick start.

### Session

A live or historical agent execution thread. Contains messages (user prompts, assistant responses, tool calls) and can be resumed (via `parent_session_id`), or cancelled.

**Lifecycle**: `connecting` → `ready` → (user prompts + agent responses) → `completed` | `cancelled` | `error`

### Trace (Session Observability)

Every session automatically produces a **trace** — a structured record of what the agent did and why. Traces are first-class, not bolted on.

**Trace events capture:**
- **Decisions**: Why the agent chose a particular approach (extracted from thinking/reasoning blocks)
- **Tool calls**: What tools were invoked, with what inputs, and what outputs
- **File changes**: Which files were read, modified, created, or deleted
- **Timing**: How long each step took
- **Errors**: What failed and how the agent recovered (or didn't)

**Journey view**: The trace is presented as a **journey** — a timeline of decisions and actions that shows the agent's reasoning path, not just the final output. This replaces the v1 "Harness Monitor" which was bolted on after the fact.

```rust
struct TraceEvent {
    id: String,
    session_id: String,
    event_type: TraceEventType,
    timestamp: DateTime<Utc>,
    data: serde_json::Value,
}

enum TraceEventType {
    Decision { summary: String },
    ToolCall { name: String, input: serde_json::Value, output: Option<String>, duration_ms: u64 },
    FileChange { path: String, action: FileAction },
    Error { message: String, recovered: bool },
    Milestone { label: String },
    Review { criteria: String, verdict: String, evidence: Option<String> },
}
```

### Provider

A configured `CodingAgent` instance. Each provider has:
- A type (e.g. "anthropic")
- Connection config (base URL, API key, default model)
- Available models
- Health status

### Specialist

Agent role definitions externalized as **Markdown files with YAML frontmatter**. Define system prompts, model preferences, and behavior instructions. Loaded from filesystem (`resources/specialists/`).

**Frontmatter schema:**
```yaml
---
name: Dev Crafter
model: sonnet
description: Implements changes within task scope
---
You are a dev crafter. Stay within task scope...
```

**Bundled specialists:**
- `routa.md` — Coordinator (plans, delegates, verifies)
- `crafter.md` — Implementor (writes code within scope)
- `gate.md` — Verifier (evidence-driven verification)
- `backlog-refiner.md` — Turns rough cards into stories with acceptance criteria
- `todo-orchestrator.md` — Validates stories, adds execution plans
- `dev-crafter.md` — Implements changes, runs verification, commits
- `review-guard.md` — Verifies acceptance criteria, rejects/approves
- `done-reporter.md` — Writes completion summaries

### Kanban Board

A board with ordered columns. Tasks are cards that move between columns. Each column can optionally bind a **specialist** — when a card moves to a column with a bound specialist and `auto_trigger` enabled, an agent session is created using that specialist's system prompt. This turns the kanban into a coordination bus, not just a task board.

## Provider Abstraction

### The `CodingAgent` Trait

```rust
/// A coding agent that can hold conversations and execute tools.
#[async_trait]
trait CodingAgent: Send + Sync {
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
```

### Implementations

| Provider | Status | Notes |
|----------|--------|-------|
| `AnthropicAgent` | v1 | Direct Anthropic API + custom base URLs for proxies |
| `OpenAIAgent` | future | OpenAI-compatible endpoints |
| `LocalAgent` | future | Ollama, vLLM, any local model server |
| `CliAgent` | future | CLI-wrapped agents (Claude Code, OpenCode) |

### Stream Events

All providers emit the same event types:

```rust
enum StreamEvent {
    TextDelta { text: String },
    ToolUseStart { id: String, name: String, input: serde_json::Value },
    ToolUseDelta { id: String, delta: String },
    ToolResult { id: String, result: String },
    Thinking { text: String },
    Done { stop_reason: StopReason },
    Error { message: String },
}
```

## API Surface

### System
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/health` | Health check |

### Workspaces
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/workspaces` | List all workspaces |
| POST | `/api/workspaces` | Create workspace |
| GET | `/api/workspaces/:id` | Get workspace |
| PATCH | `/api/workspaces/:id` | Update workspace |
| DELETE | `/api/workspaces/:id` | Delete workspace |

### Sessions
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/workspaces/:wid/sessions` | List sessions in workspace |
| POST | `/api/workspaces/:wid/sessions` | Create session (starts agent). Accepts optional `parent_session_id` to resume from a previous session's history. |
| GET | `/api/sessions/:sid` | Get session |
| DELETE | `/api/sessions/:sid` | Delete session |
| POST | `/api/sessions/:sid/prompt` | Send message to agent |
| POST | `/api/sessions/:sid/cancel` | Cancel running generation |
| GET | `/api/sessions/:sid/history` | Get full message history |
| GET | `/api/sessions/:sid/stream` | SSE stream for real-time updates. Supports `Last-Event-ID` header for reconnection replay. |

### Providers
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/providers` | List configured providers |
| POST | `/api/providers` | Add provider |
| DELETE | `/api/providers/:id` | Remove provider |
| GET | `/api/providers/:id/models` | List available models |

### Kanban
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/workspaces/:wid/boards` | List boards |
| POST | `/api/workspaces/:wid/boards` | Create board |
| GET | `/api/boards/:bid` | Get board with columns + cards |
| PATCH | `/api/boards/:bid` | Update board |
| POST | `/api/boards/:bid/columns` | Add column |
| PATCH | `/api/columns/:cid` | Update column (name, position, specialist binding, auto-trigger) |
| POST | `/api/boards/:bid/cards` | Add card |
| PATCH | `/api/tasks/:tid` | Update task (move, rename, etc.) |
| DELETE | `/api/tasks/:tid` | Delete task |
| GET | `/api/boards/:bid/stream` | SSE stream for kanban events |

### Codebases
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/workspaces/:wid/codebases` | List codebases |
| POST | `/api/workspaces/:wid/codebases` | Register codebase |
| DELETE | `/api/codebases/:cid` | Remove codebase |

### Traces (Session Observability)
| Method | Path | Purpose |
|--------|------|---------|
| GET | `/api/sessions/:sid/trace` | Get full trace for a session |
| GET | `/api/sessions/:sid/trace/journey` | Get journey summary (decisions + key actions) |
| GET | `/api/sessions/:sid/trace/files` | Get file change summary |

**Total: ~28 endpoints**

## Storage

Single SQLite database with WAL mode:

```sql
-- Core tables
workspaces (id, name, status, created_at, updated_at)
sessions (id, workspace_id, provider_id, specialist_id, parent_session_id, status, model, cwd, created_at, updated_at)
messages (id, session_id, role, content, metadata, created_at)
providers (id, type, name, config_json, created_at)
codebases (id, workspace_id, path, branch, label, created_at)

-- Kanban tables
boards (id, workspace_id, name, created_at)
columns (id, board_id, name, position, specialist_id, auto_trigger, created_at)
tasks (id, board_id, column_id, title, description, position, status, session_id, created_at, updated_at)

-- Traces (session observability)
traces (id, session_id, event_type, summary, data_json, timestamp)
file_changes (id, trace_id, session_id, path, action, timestamp)
```

## Frontend

React SPA served by the Rust binary:

```
web/
├── package.json
├── vite.config.ts
├── index.html
├── src/
│   ├── main.tsx
│   ├── app/
│   │   ├── router.tsx
│   │   ├── layout.tsx
│   │   └── pages/
│   │       ├── home.tsx
│   │       ├── workspace/
│   │       │   ├── overview.tsx
│   │       │   ├── sessions.tsx
│   │       │   ├── session.tsx      # Chat view
│   │       │   └── kanban.tsx
│   │       └── settings.tsx
│   ├── components/
│   │   ├── chat/
│   │   │   ├── message-list.tsx
│   │   │   ├── message-input.tsx
│   │   │   ├── tool-call.tsx
│   │   │   └── streaming-indicator.tsx
│   │   ├── journey/
│   │   │   ├── timeline.tsx
│   │   │   ├── decision-node.tsx
│   │   │   └── file-changes.tsx
│   │   ├── kanban/
│   │   │   ├── board.tsx
│   │   │   ├── column.tsx
│   │   │   └── card.tsx
│   │   └── shared/
│   │       ├── sidebar.tsx
│   │       ├── workspace-switcher.tsx
│   │       └── provider-badge.tsx
│   ├── hooks/
│   │   ├── use-session.ts       # Session data + SSE
│   │   ├── use-kanban.ts        # Kanban data + SSE
│   │   └── use-providers.ts     # Provider CRUD
│   └── lib/
│       ├── api.ts               # Fetch wrapper
│       └── types.ts             # Shared types
```

**Stack**: React 19, Vite, Tailwind CSS, TanStack Query, React Router

## Binary Structure

```
weave/
├── Cargo.toml                    # Workspace root
├── crates/
│   └── weave-server/             # Single binary crate
│       ├── Cargo.toml
│       ├── build.rs              # Embed frontend assets
│       └── src/
│           ├── main.rs           # Entry point + CLI args
│           ├── config.rs         # Configuration
│           ├── db.rs             # SQLite setup + migrations
│           ├── api/
│           │   ├── mod.rs        # Router assembly
│           │   ├── health.rs
│           │   ├── workspaces.rs
│           │   ├── sessions.rs
│           │   ├── providers.rs
│           │   ├── kanban.rs
│           │   ├── codebases.rs
│           │   └── traces.rs
│           ├── domain/
│           │   ├── mod.rs
│           │   ├── session.rs    # Session service
│           │   ├── kanban.rs     # Kanban service
│           │   ├── specialist.rs # Specialist loader
│           │   └── trace.rs      # Trace collection + journey
│           ├── agent/
│           │   ├── mod.rs        # CodingAgent trait
│           │   ├── registry.rs   # Provider registry
│           │   ├── events.rs     # StreamEvent types
│           │   └── anthropic/
│           │       ├── mod.rs
│           │       ├── client.rs
│           │       ├── streaming.rs
│           │       └── types.rs
│           └── store/
│               ├── mod.rs
│               ├── workspaces.rs
│               ├── sessions.rs
│               ├── providers.rs
│               ├── kanban.rs
│               └── traces.rs
├── web/                          # Frontend
│   ├── package.json
│   ├── vite.config.ts
│   ├── tailwind.config.ts
│   ├── index.html
│   └── src/
├── resources/
│   └── specialists/              # Bundled specialist definitions
│       ├── routa.md
│       ├── crafter.md
│       ├── gate.md
│       ├── backlog-refiner.md
│       ├── todo-orchestrator.md
│       ├── dev-crafter.md
│       ├── review-guard.md
│       └── done-reporter.md
├── docs/
│   ├── ARCHITECTURE.md           # This file
│   └── PLAN.md                   # Implementation plan
└── README.md
```

## What Was Dropped from Routa v1

| Dropped | Reason |
|---------|--------|
| Next.js backend | Single Rust binary eliminates dual-backend parity bugs |
| 11 hardcoded providers | Trait-based abstraction, Anthropic first, extensible |
| Tauri desktop | Web-only, no dual-origin hacks |
| macOS/Windows | Linux-first, no platform conditionals |
| JSON-RPC gateway | Plain REST + SSE |
| ACP protocol | Direct HTTP to provider APIs |
| Process spawning | Direct API calls, no child process management |
| Office WASM | Out of scope |
| Shared sessions | Too complex for v1 |
| A2A / AG-UI protocols | Out of scope |
| GitHub integration | Out of scope |
| Webhooks / schedules / workflows | Out of scope |
| MCP server mode | Out of scope |
| Docker agent execution | Out of scope |
| Drizzle ORM | rusqlite directly |
| 8GB build memory | <100MB |
