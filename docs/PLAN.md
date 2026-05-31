# Weave — Implementation Plan

## Phase 1: Foundation (Rust Backend)

**Goal**: Binary starts, serves health check, SQLite works.

### 1.1 Project Setup
- [ ] Cargo workspace with `weave-server` crate
- [ ] Dependencies: axum, tokio, rusqlite, serde, serde_json, uuid, chrono, tracing
- [ ] CLI args: `--host`, `--port`, `--db-path`
- [ ] SQLite database initialization with WAL mode
- [ ] Schema migrations (workspaces, sessions, messages, providers tables)

### 1.2 Core Stores
- [ ] `WorkspaceStore` — CRUD for workspaces
- [ ] `SessionStore` — CRUD for sessions + messages
- [ ] `ProviderStore` — CRUD for providers
- [ ] `TraceStore` — CRUD for trace events + file changes
- [ ] All stores use `rusqlite` directly (no ORM)

### 1.3 API Routes (Basic)
- [ ] `GET /api/health` — returns `{"status": "ok"}`
- [ ] `GET /api/workspaces` — list
- [ ] `POST /api/workspaces` — create
- [ ] `GET /api/workspaces/:id` — get
- [ ] `DELETE /api/workspaces/:id` — delete

### 1.4 Default Workspace
- [ ] On first startup, create a `"default"` workspace
- [ ] All resources default to this workspace

---

## Phase 2: Anthropic Provider

**Goal**: Can send messages to Anthropic API and stream responses.

### 2.1 CodingAgent Trait
- [ ] Define `CodingAgent` trait with `send_message`, `list_models`, `health_check`
- [ ] Define `StreamEvent` enum (TextDelta, ToolUseStart, ToolUseDelta, ToolResult, Done, Error)
- [ ] Define `MessageRequest` struct

### 2.2 AnthropicAgent Implementation
- [ ] HTTP client using `reqwest`
- [ ] Anthropic Messages API with streaming (`stream: true`)
- [ ] Parse SSE stream from Anthropic into `StreamEvent` items
- [ ] Handle Anthropic-specific events: `message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`
- [ ] Error handling for API errors, rate limits, auth failures

### 2.3 Provider Registry
- [ ] `ProviderRegistry` holds `HashMap<String, Arc<dyn CodingAgent>>`
- [ ] Load from SQLite on startup
- [ ] Add/remove providers at runtime
- [ ] Health check endpoint

### 2.4 Provider API Routes
- [ ] `GET /api/providers` — list
- [ ] `POST /api/providers` — add (type + base_url + api_key + default_model)
- [ ] `DELETE /api/providers/:id` — remove
- [ ] `GET /api/providers/:id/models` — list models

---

## Phase 3: Sessions + Streaming

**Goal**: Create sessions, send prompts, see streamed responses in the browser.

### 3.1 Session Service
- [ ] `SessionService` manages session lifecycle
- [ ] Create session: pick provider + model + specialist, insert into DB
- [ ] Send prompt: save user message, call provider, stream response
- [ ] Save assistant message chunks to DB as they arrive
- [ ] Cancel: abort the HTTP request to the provider
- [ ] Resume: create new session that continues from a previous session's message history (accept `parent_session_id`)

### 3.2 SSE Streaming
- [ ] `GET /api/sessions/:sid/stream` — SSE endpoint
- [ ] Buffer events in memory until client connects
- [ ] Heartbeat every 15 seconds
- [ ] Cleanup on client disconnect
- [ ] Support `Last-Event-ID` header for reconnection — replay buffered events

### 3.2b Trace Collection (Session Observability)
- [ ] Auto-record trace events during agent execution:
  - Tool calls (name, input, output, duration)
  - File changes (path, action: read/write/create/delete)
  - Decisions (extracted from thinking/reasoning blocks)
  - Errors (message, recovery status)
  - Reviews (criteria checked, verdict, optional evidence)
- [ ] `TraceStore` — persist trace events to SQLite
- [ ] Journey summary endpoint — ordered list of decisions + key actions
- [ ] File change summary — deduplicated list of files touched in session

### 3.3 Session API Routes
- [ ] `GET /api/workspaces/:wid/sessions` — list
- [ ] `POST /api/workspaces/:wid/sessions` — create
- [ ] `GET /api/sessions/:sid` — get
- [ ] `DELETE /api/sessions/:sid` — delete
- [ ] `POST /api/sessions/:sid/prompt` — send message
- [ ] `POST /api/sessions/:sid/cancel` — cancel
- [ ] `GET /api/sessions/:sid/history` — full history
- [ ] `GET /api/sessions/:sid/trace` — full trace
- [ ] `GET /api/sessions/:sid/trace/journey` — journey summary
- [ ] `GET /api/sessions/:sid/trace/files` — file change summary

### 3.4 Specialist Loading
- [ ] Load markdown + YAML frontmatter from `resources/specialists/`
- [ ] Inject system prompt into first message
- [ ] Support custom specialists via filesystem
- [ ] Document specialist YAML frontmatter schema:
  ```yaml
  ---
  name: Dev Crafter
  model: sonnet
  description: Implements changes within task scope
  ---
  You are a dev crafter. Stay within task scope...
  ```

---

## Phase 4: Frontend

**Goal**: Browser UI for creating sessions and chatting with agents.

### 4.1 Project Setup
- [ ] Vite + React 19 + TypeScript
- [ ] Tailwind CSS
- [ ] TanStack Query for data fetching
- [ ] React Router for navigation

### 4.2 Pages
- [ ] Home — workspace selector, session list
- [ ] Session — chat view with message list + input
- [ ] Settings — provider management

### 4.3 Chat Components
- [ ] `MessageList` — renders user + assistant messages
- [ ] `MessageInput` — text input + send button
- [ ] `ToolCall` — renders tool use blocks
- [ ] `StreamingIndicator` — shows agent is thinking

### 4.4 SSE Integration
- [ ] `useSession` hook — fetches session data + connects to SSE stream
- [ ] Real-time message updates as chunks arrive
- [ ] Auto-scroll to latest message

### 4.4b Journey View (Session Observability)
- [ ] Journey timeline — shows decisions and key actions in order
- [ ] File change sidebar — shows which files were touched
- [ ] Tool call expandable blocks — shows inputs/outputs
- [ ] Timing indicators — how long each step took

### 4.5 Static Serving
- [ ] Rust binary serves built frontend from embedded assets or build directory
- [ ] SPA fallback for client-side routing

---

## Phase 5: Kanban

**Goal**: Kanban boards with task management and lane automation.

### 5.1 Kanban Store
- [ ] `KanbanStore` — CRUD for boards, columns, tasks
- [ ] Board ↔ Workspace relationship
- [ ] Column ordering (position)
- [ ] Column ↔ Specialist binding (`specialist_id` column, nullable)
- [ ] Task ordering within columns

### 5.2 Kanban Service
- [ ] Create board with default columns (e.g. "To Do", "In Progress", "Done")
- [ ] Move task between columns
- [ ] Lane-specialist binding: each column can optionally bind a specialist
- [ ] Column transition triggers: when card moves to a column with a bound specialist and auto-trigger enabled, create a session with that specialist's system prompt

### 5.3 Kanban API Routes
- [ ] `GET /api/workspaces/:wid/boards` — list
- [ ] `POST /api/workspaces/:wid/boards` — create
- [ ] `GET /api/boards/:bid` — get with columns + cards
- [ ] `PATCH /api/boards/:bid` — update
- [ ] `POST /api/boards/:bid/columns` — add column
- [ ] `PATCH /api/columns/:cid` — update column
- [ ] `POST /api/boards/:bid/cards` — add card
- [ ] `PATCH /api/tasks/:tid` — update task
- [ ] `DELETE /api/tasks/:tid` — delete task
- [ ] `GET /api/boards/:bid/stream` — SSE for kanban events

### 5.4 Kanban Lane Specialists
- [ ] `backlog-refiner.md` — turns rough cards into canonical stories with acceptance criteria
- [ ] `todo-orchestrator.md` — validates stories, adds execution plans, key files, risk notes
- [ ] `dev-crafter.md` — implements changes, runs verification, commits work
- [ ] `review-guard.md` — independently verifies acceptance criteria, rejects/approves
- [ ] `done-reporter.md` — writes completion summaries

### 5.5 Kanban Frontend
- [ ] Board view with drag-and-drop columns
- [ ] Card component with title, status badge
- [ ] Move card between columns
- [ ] Column specialist indicator (shows bound specialist, auto-trigger toggle)
- [ ] Real-time updates via SSE

---

## Phase 6: Polish + Extras

### 6.1 Codebases
- [ ] Register git repos as codebases
- [ ] Associate sessions with codebases (cwd)
- [ ] Git status integration: current branch, dirty files, last 5 commits
- [ ] Codebase view in frontend showing repo context

### 6.2 Traces
- [ ] Record tool calls, file changes, timing
- [ ] Trace viewer in frontend

### 6.3 Error Handling
- [ ] Graceful error messages for provider failures
- [ ] Retry logic for transient errors
- [ ] Rate limit handling

### 6.4 Configuration
- [ ] Environment variables for default provider, database path
- [ ] Config file support (TOML or YAML)

---

## File Creation Order

### Step 1: Rust skeleton
```
Cargo.toml
crates/weave-server/Cargo.toml
crates/weave-server/src/main.rs
crates/weave-server/src/config.rs
crates/weave-server/src/db.rs
crates/weave-server/src/api/mod.rs
crates/weave-server/src/api/health.rs
crates/weave-server/src/store/mod.rs
crates/weave-server/src/store/workspaces.rs
crates/weave-server/src/store/traces.rs
crates/weave-server/src/domain/mod.rs
```

### Step 2: Provider + Agent
```
crates/weave-server/src/agent/mod.rs
crates/weave-server/src/agent/registry.rs
crates/weave-server/src/agent/events.rs
crates/weave-server/src/agent/anthropic/mod.rs
crates/weave-server/src/agent/anthropic/client.rs
crates/weave-server/src/agent/anthropic/streaming.rs
crates/weave-server/src/agent/anthropic/types.rs
crates/weave-server/src/api/providers.rs
crates/weave-server/src/store/providers.rs
```

### Step 3: Sessions + Traces
```
crates/weave-server/src/domain/session.rs
crates/weave-server/src/domain/trace.rs
crates/weave-server/src/store/sessions.rs
crates/weave-server/src/api/sessions.rs
crates/weave-server/src/api/traces.rs
crates/weave-server/src/domain/specialist.rs
resources/specialists/routa.md
resources/specialists/crafter.md
resources/specialists/gate.md
resources/specialists/backlog-refiner.md
resources/specialists/todo-orchestrator.md
resources/specialists/dev-crafter.md
resources/specialists/review-guard.md
resources/specialists/done-reporter.md
```

### Step 4: Frontend
```
web/package.json
web/vite.config.ts
web/tailwind.config.ts
web/index.html
web/src/main.tsx
web/src/app/router.tsx
web/src/app/layout.tsx
web/src/app/pages/home.tsx
web/src/app/pages/session.tsx
web/src/app/pages/settings.tsx
web/src/components/chat/message-list.tsx
web/src/components/chat/message-input.tsx
web/src/components/journey/timeline.tsx
web/src/components/journey/decision-node.tsx
web/src/components/journey/file-changes.tsx
web/src/hooks/use-session.ts
web/src/lib/api.ts
web/src/lib/types.ts
```

### Step 5: Kanban
```
crates/weave-server/src/domain/kanban.rs
crates/weave-server/src/store/kanban.rs
crates/weave-server/src/api/kanban.rs
web/src/app/pages/kanban.tsx
web/src/components/kanban/board.tsx
web/src/components/kanban/column.tsx
web/src/components/kanban/card.tsx
web/src/hooks/use-kanban.ts
```

### Step 6: Polish
```
crates/weave-server/src/store/codebases.rs
crates/weave-server/src/api/codebases.rs
crates/weave-server/build.rs (embed frontend)
README.md
```
