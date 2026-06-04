# PROGRESS.md

<!--
The amnesiac craftsman's journal.
Updated at session start (read it) and session end (rewrite it).
A fresh session should be able to reach an executable state in under 3 minutes by reading this file.
-->

## Current State

- **Last updated:** 2026-06-04
- **Latest commit:** 8f07550 (feat-034 graceful shutdown)
- **Active feature:** none — all features through phase-5 are passing
- **Build status:** green — `./init.sh` all 3 layers pass
- **Test status:** green — 605 Rust tests + 83 frontend tests pass
- **Lint status:** green — clippy clean, fmt clean, prettier clean, ESLint clean

## Completed Since Project Start

- [x] System design docs (`docs/SYSTEM_DESIGN.md`, `docs/ARCHITECTURE.md`, `docs/PLAN.md`)
- [x] **feat-001**: Binary skeleton (CLI, tracing, health check, graceful shutdown)
- [x] **feat-002**: SQLite with WAL mode, migrations (11 tables)
- [x] **feat-003**: Shared error types (AppError, ProviderError)
- [x] **feat-004**: Workspace CRUD (store + API + default seed)
- [x] **feat-005**: CodingAgent trait (StreamEvent, StopReason, Send+Sync)
- [x] **feat-006**: AnthropicAgent (SSE streaming, error mapping, retry)
- [x] **feat-007**: ProviderStore + ProviderRegistry (CRUD, api_key stripping)
- [x] **feat-008**: SessionStore + MessageStore (state machine, pagination)
- [x] **feat-009**: SessionService (prompt lifecycle, streaming, cancellation)
- [x] **feat-010**: SSE infrastructure (SseManager, EventBuffer, reconnection)
- [x] **feat-011**: SpecialistLoader (YAML frontmatter, system prompt injection)
- [x] **feat-012**: ToolRegistry (ToolExecutor trait, 5 profiles)
- [x] **feat-013**: Filesystem tools (fs_read/write/edit/search/list, PathValidator)
- [x] **feat-014**: Shell tool (shell_exec, timeout, 100KB truncation)
- [x] **feat-015**: Git tools (status, diff, log, commit, identity validation)
- [x] **feat-016**: Task context tools (get/list/update, workspace-scoped)
- [x] **feat-017**: TraceCollector (channel-based, background flush, 3 API endpoints)
- [x] **feat-018**: Session resume (parent chain, depth limit 5, cycle detection)
- [x] **feat-019**: React frontend scaffolding (Vite + React 19 + TS + Tailwind + TanStack Query)
- [x] **feat-020**: Frontend pages (Home, Workspace, Settings, shared components)
- [x] **feat-021**: Session chat view (useSession hook, SSE streaming, Markdown)
- [x] **feat-022**: Journey sidebar (Decision timeline, FileChangesList, collapsible)
- [x] **feat-023**: Frontend served from Rust binary (build.rs + ServeDir fallback)
- [x] **feat-024**: KanbanStore CRUD + 8-endpoint HTTP API (boards, columns, tasks)
- [x] **feat-025**: KanbanService lane automation + board-scoped SSE stream
- [x] **feat-026**: Kanban frontend (@dnd-kit, real-time SSE, TaskDetailPanel)
- [x] **feat-027**: Default board template + 5 built-in specialists
- [x] **feat-028**: Kanban tools for agents (get_board, move_card, create_card, search_cards) + transition gates
- [x] **feat-029**: A2A protocol server endpoints (Agent Card, SendMessage, GetTask, CancelTask, SubscribeToTask SSE)
- [x] **feat-030**: Note tools for agents (create, read, list, set_content, append)
- [x] **feat-031**: Artifact tools (request, provide, list) + kanban transition gate-3
- [x] **feat-032**: CodebaseStore + API + frontend pages
- [x] **feat-033**: Enhanced health check (version, uptime, provider total/healthy/unhealthy, per-workspace active_sessions, db size_bytes, wal_checkpoint_pending; 10s provider-health TTL cache; always 200 with status="ok"|"degraded")
- [x] **feat-034**: Graceful shutdown — SIGTERM/SIGINT/drain-cap race, parent CancellationToken in AppState, ActiveSessions::cancel_all, SseWireEvent::Shutdown + SseManager::broadcast_shutdown, Db::checkpoint (TRUNCATE), service::startup::reap_orphans (transactional mark-as-error), spawn cleanup task, run() extracted from main(). 12 new tests.
- [x] **feat-036**: Session chat re-implementation (message_persisted SSE, useReducer, id-based handoff)

## In Progress

(none — all features in phases 1-5 are passing)

## Blocked

(none)

## Remaining Features

| ID | Description | Dependencies |
|----|-------------|-------------|
| feat-035 | Configuration (env vars, CLI, TOML) | feat-001 |

## Key Architectural Decisions

See `DECISIONS.md` for full rationale. Quick reference:
- Single Rust binary with embedded frontend (build.rs)
- SQLite with WAL mode, no ORM (raw rusqlite)
- SSE for all real-time (no WebSocket)
- Workspace-scoped resources (every query includes workspace_id)
- `feature_list.json` is single source of truth for task scope

## Out-of-Scope Items Noticed

Items deferred from past sessions. Address when a feature touches the relevant area.

- **`verify_task_in_workspace` duplicated** across `store/artifacts.rs`, `service/kanban.rs`, `api/kanban.rs` — 3 copies of "look up task's workspace via board". Fix: add `TaskStore::workspace_id_for_task`.
- **`seed_task` helper duplicated** across 5+ tool test files. Fix: add to `kanban_test_helpers.rs`.
- **Unmatched `/api/*` paths return index.html** instead of 404 JSON (feat-023 fallback catches them). Fix: nest API router under `/api` with JSON 404 handler.
- **`SseManager` channel GC**: no cleanup for stale board/session channels on long-running servers.
- **Transition gates bypassed on HTTP PATCH**: `api/kanban.rs::update_task` calls `move_to_column` without `check_transition_gates`. Frontend drag-and-drop bypasses the gate.
- **TOCTOU between gate check and move**: gate runs in a read tx, move in a write tx. Window is tight (SQLite WAL serializes) but exists.
- **`MAX_TASK_TITLE_LEN` defined in two places**: `tools/fs/mod.rs` and `api/kanban.rs`. Fix: hoist to `store::tasks`.
- **Cancel button always visible** in session header even when no stream is active. UX wart.

## Session Notes

### 2026-06-03 — feat-029, feat-030, feat-031, feat-032
- feat-029: A2A protocol implemented (6 files in `src/a2a/`, migration 009 adds `context_id` to sessions). 582 Rust tests.
- feat-030: Note tools (5 tool executors, `notes` table via migration 008). `map_insert_error` hoisted to `db.rs` (3rd caller). 569 Rust tests.
- feat-031 Phase 6 reconciliation: all 8 critical+important review fixes confirmed already-applied. PROGRESS.md updated.
- feat-032: CodebaseStore + API + frontend (4 new backend files, 4 new frontend files). 518 Rust tests + 83 frontend tests.

### 2026-06-04 — feat-033
- Enhanced health check (`GET /api/health`): added `providers` (total/healthy/unhealthy), `active_sessions` (per-workspace `BTreeMap`), `database` (size_bytes, wal_checkpoint_pending, reachable). Raw JSON shape preserved (liveness-probe contract). Provider health probed in parallel via `futures_util::future::join_all` with a 10s TTL cache; `add_agent`/`remove_agent` invalidate the cache. `degraded` rule: `healthy == 0 || !database.reachable`. 593 Rust tests pass (11 new). 4 files touched: `db.rs` (+ `path: PathBuf`, `size_bytes`, `wal_checkpoint_pending`), `store/sessions.rs` (+ `count_active_by_workspace` using the `TERMINAL` const), `agent/registry.rs` (+ `health_cache`, `cached_health_summary`, `agents_snapshot`, `invalidate_health_cache`), `api/health.rs` (rewrote `HealthResponse`, added `ProviderSummary`/`DatabaseInfo` and 4 integration tests including a cache-hit + healthy-status pair).

### 2026-06-02 — feat-022, feat-026, feat-023
- feat-022: Journey sidebar. Backend SQL filter tightened to Decision+Error only. Frontend: 5 components, 14 new tests.
- feat-026: Kanban frontend. @dnd-kit drag-and-drop, SSE real-time updates, TaskDetailPanel slide-over. 17 new tests.
- feat-023: Frontend served from Rust binary. First `build.rs`, `static_assets.rs` with SPA fallback. 5 new tests.
- Bug fix: Journey sidebar decision fragmentation (177 rows → ~5 per turn). Thinking deltas coalesced into single Decision per reasoning pass.

### 2026-06-01 — feat-019, feat-020, feat-021, feat-036, bug fixes
- Frontend scaffolding + pages + session chat view implemented.
- feat-036: Session chat re-implementation (message_persisted SSE, useReducer, id-based handoff).
- Multiple bug fixes: session terminated after first turn, message ordering by UUID, user message invisible, page flash on completion, stale "Thinking..." badge.

### 2026-05-31 — Initial harness + feats 001-018
- Core foundation: binary, database, providers, sessions, SSE.
- Agent tools: filesystem, shell, git, task context, TraceCollector.
- Session resume with parent chain validation.

## Notes for Next Session

- Package manager is **Bun** (not npm). Use `bun run test`, `bunx vite build`, etc.
- Tailwind CSS v4 uses `@tailwindcss/vite` plugin + `@import "tailwindcss"` (no config file).
- `build.rs` runs `bunx vite build` at compile time. `WEAVE_SKIP_FRONTEND_BUILD=1` to skip.
- Dev: `just dev` (backend) + `just dev-web` (frontend). Production: single binary.
- `./init.sh` is the one-command full verification gate. Run it before and after any change.
- `feature_list.json` is the single source of truth for task scope — do not track work in comments or TODOs.
- The 1 remaining feature is feat-035 (config).
