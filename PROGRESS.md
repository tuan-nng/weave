# PROGRESS.md

<!--
The amnesiac craftsman's journal.
Updated at session start (read it) and session end (rewrite it).
A fresh session should be able to reach an executable state in under 3 minutes by reading this file.
-->

## Current State

- **Last updated:** 2026-05-31
- **Latest commit:** (pending — feat-012)
- **Active feature:** none
- **Build status:** green — `cargo build -p weave-server` succeeds
- **Test status:** green — 149 tests pass (14 new for feat-012 + 135 existing)
- **Lint status:** green — clippy clean, fmt clean

## Completed Since Project Start

- [x] System design documentation (`docs/SYSTEM_DESIGN.md`, `docs/ARCHITECTURE.md`)
- [x] Implementation plan (`docs/PLAN.md`)
- [x] Workspace `Cargo.toml` created (members: `crates/weave-server`)
- [x] **feat-001**: Binary skeleton with CLI, tracing, health check, graceful shutdown
- [x] **feat-002**: SQLite database with WAL mode and migrations (11 tables, user_version tracking)
- [x] **feat-003**: Shared error types (AppError, ProviderError) with thiserror, IntoResponse, JSON envelope
- [x] **feat-004**: Workspace CRUD (WorkspaceStore, REST API, default workspace seed, cursor pagination)
- [x] **feat-005**: CodingAgent trait (provider abstraction, StreamEvent, StopReason, message types, all Send+Sync)
- [x] **feat-006**: AnthropicAgent (SSE streaming, error mapping, retry logic, message conversion)
- [x] **feat-007**: ProviderStore + ProviderRegistry (CRUD, agent lifecycle, API with api_key stripping)
- [x] **feat-008**: SessionStore + MessageStore (session state machine, message pagination, session API)
- [x] **feat-009**: SessionService (prompt lifecycle, async streaming, cancellation, message history)
- [x] **feat-010**: SSE infrastructure (SseManager, EventBuffer, SSE endpoint, reconnection, backpressure)
- [x] **feat-011**: SpecialistLoader (YAML frontmatter parsing, SpecialistRegistry, system prompt + model override injection)
- [x] **feat-012**: ToolRegistry (ToolExecutor trait, 5 profiles, profile-based filtering, SessionService integration)

## In Progress

(none)

## Blocked

(none)

## Known Issues

- `web/` directory does not exist yet (expected — Phase 3)

## Next Steps

1. Start feat-013: Filesystem tools (depends on feat-012, feat-011 — both passing)
2. Continue Phase 2 (Agent Tools & Observability)

## Session Notes

### 2026-05-31 — Linter/formatter config added
- Added `rustfmt.toml` — stable options only (edition 2021, max_width 100, import reordering)
- Added `clippy.toml` — complexity thresholds (cognitive 25, lines 150, args 8), test allowances
- Added `.cargo/config.toml` — placeholder for future lint additions
- All 128 tests pass, clippy clean, fmt clean, smoke test passes

### 2026-05-31 — feat-012: ToolRegistry
- Created `src/tools/mod.rs` — new module for tool infrastructure
- `ToolExecutor` trait: `name()`, `description()`, `input_schema()`, `execute(input, context) -> ToolResult` (async_trait, Send+Sync)
- `ToolContext`: `session_id`, `cwd`, `codebase_root`, `trace_collector: Arc<TraceCollector>`
- `TraceCollector` is a stub (empty struct) — will be fleshed out in feat-017
- `ToolResult`: `success`, `data`, `error` — serde roundtrip verified
- `ToolRegistry`: `HashMap<String, Arc<dyn ToolExecutor>>` + `HashMap<String, Vec<String>>` for profiles
- Five profiles: `full` (dynamic=all registered), `implementation`, `review`, `planning`, `reporting`
- `validate_profile_name()` for early fail-fast in `send_prompt`
- `resolve_profile()` returns `Vec<ToolDefinition>` — empty vec converted to None by caller
- `all_definitions()` sorts by name for deterministic output
- `AppState` now has `tools: Arc<ToolRegistry>` field
- `SessionService::send_prompt` validates tool profile early, `run_prompt_task` resolves tools from specialist's profile
- Invalid profile name → `AppError::Validation` with dynamic error message
- `test_support` module exports `MockTool` for shared use across test modules
- 149 tests pass (14 new: 12 tools + 2 service integration)

## Notes for Next Session

- feat-012 created: `src/tools/mod.rs`
- `ToolRegistry` is immutable after startup — no Mutex needed
- Profile resolution in `run_prompt_task` happens after specialist resolution but before `system_prompt` extraction
- `validate_profile_name` in `send_prompt` prevents spawning a task that will immediately fail
- Empty tool list from `resolve_profile` is converted to `None` in `run_prompt_task` (not sent as `tools: []`)
- `test_support::MockTool` is `pub(crate)` — reusable from other test modules
- `CapturingAgent` is shared at the top of `service/sessions::tests` module
- Next feature: feat-013 (filesystem tools) will register concrete `ToolExecutor` implementations

## Out-of-Scope Items Noticed

(none yet)
