# Weave — System Design

Weave is a web-based multi-agent coordination platform. A single Rust binary serves a REST+SSE API and a bundled React SPA. Agent sessions are managed through a trait-based provider abstraction with Anthropic as the first implementation. Kanban boards serve as coordination buses where column transitions can trigger specialist agent sessions.

**Key design decisions:**
- Single binary, no external runtime dependencies
- SQLite (WAL mode) for all persistence — zero configuration
- SSE for all real-time streaming — no WebSocket, no polling
- Trait-based provider abstraction — add new agents by implementing `CodingAgent`
- Workspace-scoped resources — everything lives under a workspace (except providers, which are global)
- Single-user, localhost-first — no auth layer in v1
- Agent tools execute server-side — no sandboxing in v1

## Architecture Layers

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

## Topic Docs

Load only the one relevant to your task — don't read the whole set.

| Doc | When to load |
|-----|-------------|
| [`docs/data-model.md`](data-model.md) | Adding/modifying database tables, understanding schema |
| [`docs/api-contracts.md`](api-contracts.md) | Adding/modifying API endpoints, SSE event format |
| [`docs/domain-services.md`](domain-services.md) | Understanding service orchestration, session lifecycle, specialist loading |
| [`docs/provider-abstraction.md`](provider-abstraction.md) | Adding tools, new provider, tool profiles, security constraints |
| [`docs/sse-design.md`](sse-design.md) | SSE infrastructure, reconnection, backpressure |
| [`docs/kanban-automation.md`](kanban-automation.md) | Lane automation flow, board templates |
| [`docs/error-handling.md`](error-handling.md) | Error types, HTTP mapping, retry strategy |
| [`docs/concurrency-model.md`](concurrency-model.md) | Async task spawning, shared state, graceful shutdown |
| [`docs/frontend-architecture.md`](frontend-architecture.md) | Component trees, hooks, SSE→cache sync strategy |
| [`docs/operations.md`](operations.md) | Dependencies, security, logging, backup, performance |

## Session State Machine

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

| From | To | Trigger |
|------|-----|---------|
| connecting | ready | First successful message exchange |
| ready | completed | Agent returns `stop_reason: end_turn` |
| ready | cancelled | User cancels or session times out |
| ready | error | Provider error (non-retryable) |
| ready | ready | New prompt sent (stays ready) |

**Timeout**: Sessions with no activity for 30 minutes are auto-completed.

## What Was Intentionally Dropped

| Feature | Reason |
|---------|--------|
| WebSocket | SSE is simpler, sufficient for unidirectional streaming |
| JSON-RPC | REST is simpler, well-understood |
| Postgres support | SQLite is sufficient, zero configuration |
| Docker isolation | Out of scope for v1 |
| macOS/Windows | Linux-first, no platform conditionals |
| GitHub integration | Out of scope |
| MCP server mode | Out of scope |
| Webhooks / schedules | Out of scope |
| i18n | English only for v1 |
| Multiple concurrent providers | One active provider per session is sufficient |
