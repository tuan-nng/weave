# PROGRESS.md

<!--
The amnesiac craftsman's journal.
Updated at session start (read it) and session end (rewrite it).
A fresh session should be able to reach an executable state in under 3 minutes by reading this file.
-->

## Current State

- **Last updated:** 2026-06-04
- **Latest commit:** 026ac45 (feat-034 graceful shutdown)
- **Active feature:** none — feat-037 (native Anthropic tool-execution loop) is now `passing`; multi-runtime foundation phase-6 prerequisite complete; phase-7 (feat-038..042) is now unblocked
- **Build status:** green — `./init.sh` all 3 layers pass
- **Test status:** green — 611 Rust tests + 83 frontend tests pass
- **Lint status:** green — clippy clean, fmt clean, prettier clean, ESLint clean
- **Uncommitted:** feat-037 implementation. New `StopReason::LoopLimit { iterations }` variant + `is_error` field on `ContentBlock::ToolResult`. New `agent_loop` async fn in `service/sessions.rs` driving the model ↔ tool-execution loop (MAX_TOOL_ITERATIONS=8, TOOL_EXECUTION_TIMEOUT=30s). New `ToolOutcome` enum handling unknown tool / validation failure / tool error / cancel-mid-loop / loop-cap. New `execute_tool_call` free fn with JSON Schema validation (Draft7) and per-tool timeout. New `sanitize_tool_input` helper trimming string leaves. `TraceCollector` made `Clone`. `EventConverter` now defers `ToolUseStart` emission until `ContentBlockStop` so streamed `input_json_delta` is assembled into a `serde_json::Value` before the tool is invoked. `ContentBlock::ToolResult` carries `is_error` to the wire. A2A `map_sse_to_a2a` maps `LoopLimit` → `TaskStatus::Failed`. `build_message_metadata` includes `tool_calls` summary in metadata JSON whenever a tool was called. 7 new spec tests + `ScriptedTool` + `ScriptedAgent` + 2 helpers.

## Completed Since Project Start

- [x] System design docs (`docs/SYSTEM_DESIGN.md`, `docs/ARCHITECTURE.md`, `docs/road-map/PLAN.md`)
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
- [x] **feat-037**: Native Anthropic tool-execution loop (agent_loop, ToolOutcome, JSON Schema validation, sanitize_tool_input, EventConverter deferred-emit, LoopLimit stop_reason). 7 spec tests cover basic happy path, unknown tool, validation error, exec error, loop limit, cancel mid-loop, and no-tool passthrough.

## In Progress

(none — all features in phases 1-5 are passing)

## Blocked

(none)

## Remaining Features

| ID | Description | Dependencies |
|----|-------------|-------------|
| feat-035 | Configuration (env vars, CLI, TOML) | feat-001 |
| feat-037 | Native Anthropic tool-execution loop (prerequisite) | feat-005, 006, 009, 012, 013 |
| feat-038 | Session table migration for runtime/mode/cli_resume_id | feat-008 |
| feat-039 | Provider table config discriminated union (HTTP vs CLI) | feat-007 |
| feat-040 | Runtime Tool × mode compatibility validator | feat-005, 038, 039 |
| feat-041 | CodingAgent trait extension for CLI turn context (`TurnContext`) | feat-005, 009, 038 |
| feat-042 | ProviderRegistry model cache (per-Runtime-Tool, 5min TTL) | feat-005, 007, 039 |
| feat-043 | Per-turn CLI subprocess runner | feat-009, 041 |
| feat-044 | Fake CLI test harness (conformance fixture) | — |
| feat-045 | Claude Code `stream-json` parser | feat-005 |
| feat-046 | `PermissionMapper` trait + Claude Code implementation | feat-005, 012, 040, 041 |
| feat-047 | CLI resume metadata persistence + replay fallback | feat-005, 008, 038, 041, 043, 045 |
| feat-048 | `JourneyTranslator` for CLI streams (no re-execution) | feat-005, 017, 043, 045 |
| feat-049 | Child-process reaping on startup + per-session tracking | feat-009, 034, 043 |
| feat-050 | Workspace-scoped CLI session validation (cwd inside codebase) | feat-008, 032, 040 |
| feat-051 | `ClaudeCodeCodingAgent` end-to-end (fake harness) | feat-037…050 |
| feat-052 | Settings page Runtime Tool-aware form | feat-020, 039, 042 |
| feat-053 | 4-step session creation sheet (Runtime Tool → Role → Model → What it works on) | feat-021, 040, 041, 042 |
| feat-054 | Session page layout switcher (native / wrapped / attended) | feat-021, 040, 051 |
| feat-055 | Kanban column `(runtime_kind, specialist_id)` binding | feat-024, 025, 040 |
| feat-056 | A2A explicit Runtime Tool selection (no first-provider fallback) | feat-029, 040 |
| feat-057 | Shared CLI adapter conformance test suite | feat-043, 044, 045, 046, 047, 048, 050 |
| feat-058 | `CodexCodingAgent` adapter | feat-051, 057 |
| feat-059 | `OpenCodeCodingAgent` adapter | feat-051, 057 |
| feat-060 | Attended mode `Terminal` abstraction (deferred) | feat-051 |

Detailed task descriptions (per-feature engineering handoff) live at `docs/road-map/multi-runtime-tasks.md`.

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
- **Tool-containment gap** (security audit, feat-037 review): `ToolContext.codebase_root` is hardcoded to server CWD (`service/sessions.rs:436`). `fs_read` (`tools/fs/read.rs:34-60`), `fs_list` (`tools/fs/list.rs:47`), and `fs_search` (`tools/fs/search.rs:55-59`) only call `PathValidator::require_absolute` — they do NOT call `validate_write_path`, so a model can read or list any absolute path the server can reach. `shell_exec` (`tools/shell.rs:63-77`) does not validate `cwd` against `codebase_root` either. Fix in a future feature: add `root_path` to `workspaces` table; require every tool path arg to be contained under `codebase_root`.
- **Tool `input_schema` compile failure silently allows the call** (`service/sessions.rs:692-702`). Should return `ValidationFailed` instead of proceeding.
- **`tracing::debug!(... command = %command ...)` in `shell_exec`** (`tools/shell.rs:82-88`) logs the full shell command including any embedded secrets. Drop the `command` field, keep only binary name + arg count.
- **`agent_loop` clones `history` and `tool_defs` per iteration** (O(n²)). Switch `MessageRequest` to borrow `&[Message]` + `&[ToolDefinition]`.

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

### 2026-06-04 — User-facing docs under `docs/user/`
- Created `docs/user/` mirroring routa's `use-routa/` style: short, scannable, second-person, UX-focused (not internals).
- 11 files: `index.md` (landing), `quickstart.md` (5-min path), then one per feature (workspaces, providers, sessions, journey, kanban, codebases, specialists), plus `common-workflows.md` and `best-practices.md`.
- Internal `docs/` (ARCHITECTURE, data-model, etc.) stays the engineer-facing source of truth; `docs/user/` is the user-facing counterpart and the right handoff for new Weave users.
- No code changes, all 605 Rust + 83 frontend tests still green, `./init.sh` still passes.

### 2026-06-04 — Multi-runtime strategic plan
- Wrote `docs/road-map/multi-runtime-strategy.md` (committed strategic direction). Commits the direction: sessions gain a Runtime Tool axis (`claude-code` / `codex` / `opencode` / `anthropic-api` / `openai-api` / `openai-compatible`) and a `mode` (`native` / `wrapped` / `attended`) axis. The first implementation prerequisite is the native Anthropic tool-execution loop; Claude Code CLI wrapped mode is the first CLI target. The `Provider` table widens to a discriminated union; `CliCodingAgent` is added alongside `AnthropicAgent` with request/context shape to revisit; attended mode is a separate `Terminal` abstraction.
- Records the non-obvious calls: Claude Code CLI wrapped mode is the first implementation target, specialists stay prompt-only, models come from the tool not Weave, journey is the unifying artifact, per-turn subprocess for wrapped mode, the `Multiple concurrent providers` drop in `SYSTEM_DESIGN.md` is amended.
- Registered in `docs/SYSTEM_DESIGN.md` routing map. Pointer in `DECISIONS.md` (2026-06-04 entry). Doc-only change — no code, no schema migration, no API surface change yet.
- Implementation plan is the next deliverable; the strategic plan explicitly defers schema, API, and frontend decisions to it.

### 2026-06-04 — Multi-runtime task breakdown
- Broke the strategy into 24 implementation features across 6 new phases in `feature_list.json` (feat-037…feat-060). All new entries `state: "not_started"`. WIP=1 invariant preserved (no feature in `active` state). Existing 35 passing features and feat-035 (not_started) untouched.
- Phases: phase-6 (native tool loop), phase-7 (multi-runtime foundation: schema + trait + cache), phase-8 (Claude Code wrapped mode — 9 features), phase-9 (multi-runtime user surface), phase-10 (Codex/OpenCode adapters), phase-11 (attended mode, deferred).
- Key commitments baked into the breakdown: `TurnContext` extends the `CodingAgent` trait (not `MessageRequest`); `cli_resume_id` lives inside `runtime_metadata_json` (generic per-runtime column, not CLI-specific); `attended` mode is rejected at session creation until Phase 11; adapter conformance suite (feat-057) is a hard gate for Codex/OpenCode.
- Detailed per-feature task descriptions (engineering handoff format) live at `docs/road-map/multi-runtime-tasks.md` (created in this session).
- `feature_list.json` validated: 11 phases, 60 features, all phase refs resolve, all dependency targets exist, states preserved. JSON load test passed.

### 2026-06-04 — Doc reorganization into `docs/road-map/`
- Moved `docs/PLAN.md` and `docs/multi-runtime-strategy.md` into `docs/road-map/`. PLAN moved via `git mv` (rename preserved in history); strategy moved via plain `mv` (was untracked).
- `docs/SYSTEM_DESIGN.md` — added the new doc to the topic-docs routing map; amended the "Multiple concurrent providers" drop to point at the new path. Link targets (relative `(...)`) fixed for both occurrences.
- `CLAUDE.md` — Topic Docs list split into **Road-map** (forward-looking plans) and **Current state** (reference material for the system as it exists). Two new entries in the Road-map subsection.
- `README.md` — Plan link updated to the new path.
- `DECISIONS.md` — multi-runtime strategy link updated (full path retained since DECISIONS.md is at the repo root).
- `PROGRESS.md` — historical journal entries updated to the new paths so future readers can click through.
- Verification: `grep` for the old paths returns empty; `grep` for stale relative link targets returns empty. Doc-only — `./init.sh` is not affected.

## Notes for Next Session

- Package manager is **Bun** (not npm). Use `bun run test`, `bunx vite build`, etc.
- Tailwind CSS v4 uses `@tailwindcss/vite` plugin + `@import "tailwindcss"` (no config file).
- `build.rs` runs `bunx vite build` at compile time. `WEAVE_SKIP_FRONTEND_BUILD=1` to skip.
- Dev: `just dev` (backend) + `just dev-web` (frontend). Production: single binary.
- `./init.sh` is the one-command full verification gate. Run it before and after any change.
- `feature_list.json` is the single source of truth for task scope — do not track work in comments or TODOs.
- The 1 remaining feature is feat-035 (config).
- `docs/user/` is the user-facing documentation set (created 2026-06-04). When a feature ships, consider whether its user-facing guide needs an update — but do not change internal `docs/*.md` from a user-doc session.
