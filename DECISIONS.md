# DECISIONS.md

<!--
Non-obvious architectural and design choices, with the reason.
Records the "why" that gets lost when context is compacted or sessions reset.
NOT a code-review log. Only decisions that would otherwise force re-derivation.

Format: dated, scoped, with rejected alternatives.
Newest at the top.
-->

## 2026-05-31: Single Rust binary with embedded frontend

- **Decision:** Build a single Rust binary that embeds the compiled React frontend at build time via `build.rs`. No separate Node.js runtime in production.
- **Reason:** Simplifies deployment to a single executable. Eliminates CORS, reverse proxy config, and Node.js runtime dependency. Linux-first target.
- **Rejected alternatives:**
  - Separate frontend server (Vite/Nginx) — rejected: adds operational complexity, CORS config, two processes to manage
  - Serve static files from filesystem at runtime — rejected: single binary is cleaner, no path configuration needed
- **Constraints introduced:** `build.rs` must run frontend build before Rust compilation. Dev mode uses Vite dev server with proxy to backend.
- **Revisit when:** Frontend build time exceeds 30s and blocks iteration speed.

## 2026-05-31: SQLite with WAL mode, no ORM

- **Decision:** Use `rusqlite` directly with WAL mode. No ORM (Diesel, SeaORM, etc.).
- **Reason:** Single-user/small-team tool. SQLite is embedded, zero-config, and WAL handles concurrent reads well. Raw SQL gives full control over the 11-table schema without ORM abstraction overhead.
- **Rejected alternatives:**
  - Diesel — rejected: macro-heavy, compile-time schema doesn't match rapid iteration phase
  - SeaORM — rejected: async wrapper around SQLite adds complexity for minimal gain
  - Postgres — rejected: requires separate process, overkill for workspace-scoped tool
- **Constraints introduced:** All queries must be parameterized (no string interpolation). Migrations are manual SQL files in `resources/migrations/`.
- **Revisit when:** Multi-user concurrent write workload exceeds SQLite's WAL capacity (~100 concurrent writers).

## 2026-05-31: SSE for all real-time communication

- **Decision:** Server-Sent Events (SSE) as the sole real-time transport. No WebSocket.
- **Reason:** SSE is simpler, works over standard HTTP, auto-reconnects, and is sufficient for server-to-client streaming (agent responses, trace events, kanban updates). Client-to-server communication uses regular HTTP POST.
- **Rejected alternatives:**
  - WebSocket — rejected: bidirectional not needed, adds protocol complexity, harder to proxy/load-balance
  - Long polling — rejected: higher latency, more resource waste
- **Constraints introduced:** All streaming endpoints follow `/api/{resource}/stream` pattern. Event buffer must handle client disconnect/reconnect with `Last-Event-ID`.
- **Revisit when:** True bidirectional streaming is needed (unlikely for this architecture).

## 2026-05-31: Provider abstraction via CodingAgent trait

- **Decision:** Define a `CodingAgent` trait that abstracts provider capabilities. Anthropic is the first implementation.
- **Reason:** Enables adding OpenAI, local models, or custom providers without changing session/kanban logic. Trait-based design is idiomatic Rust.
- **Rejected alternatives:**
  - Hardcode Anthropic — rejected: limits extensibility, contradicts platform positioning
  - Plugin system with dynamic loading — rejected: premature complexity for v1
- **Constraints introduced:** `StreamEvent` enum is the universal streaming contract. Providers must implement `send_message`, `list_models`, `health_check`.
- **Revisit when:** Second provider is added — validate the trait shape still fits.

## 2026-05-31: Workspace-scoped resources

- **Decision:** Every resource (sessions, boards, providers) belongs to a workspace. Default workspace created on first startup.
- **Reason:** Multi-workspace support is a core design goal. Scoping from the start prevents painful migration later.
- **Rejected alternatives:**
  - Global resources with optional workspace — rejected: leads to inconsistent scoping, "orphan" resources
  - Multi-tenant with tenant_id — rejected: workspaces are the tenant concept, no need for a separate layer
- **Constraints introduced:** Every DB query must include `workspace_id`. API routes are nested under `/api/workspaces/:wid/`.
- **Revisit when:** Never — this is a permanent architectural choice.
