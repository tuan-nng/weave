# PROGRESS.md

<!--
The amnesiac craftsman's journal.
Updated at session start (read it) and session end (rewrite it).
A fresh session should be able to reach an executable state in under 3 minutes by reading this file.
-->

## Current State

- **Last updated:** 2026-05-31
- **Latest commit:** 25ea940
- **Active feature:** none
- **Build status:** green — `cargo build -p weave-server` succeeds
- **Test status:** green — 135 tests pass (7 new for feat-011 + 128 existing)
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

## In Progress

(none)

## Blocked

(none)

## Known Issues

- `web/` directory does not exist yet (expected — Phase 3)

## Next Steps

1. Start feat-012: ToolRegistry (depends on feat-005 — passing)
2. Continue Phase 2 (Agent Tools & Observability)

## Session Notes

### 2026-05-31 — Linter/formatter config added
- Added `rustfmt.toml` — stable options only (edition 2021, max_width 100, import reordering)
- Added `clippy.toml` — complexity thresholds (cognitive 25, lines 150, args 8), test allowances
- Added `.cargo/config.toml` — placeholder for future lint additions
- All 128 tests pass, clippy clean, fmt clean, smoke test passes

## Notes for Next Session

- feat-011 created: `src/specialist/mod.rs`
- `SpecialistRegistry` wraps `HashMap<String, Specialist>` with `load_from_dir`, `get_by_name`, `count`, `all`
- Frontmatter parsing uses `serde_yaml::Value` (no duplicate struct) — extracts `name`, `description`, `model`, `tool_profile`, `tags`
- Closing `---` delimiter must start at line boundary (`\n---`) to avoid false matches inside YAML values
- `resources/specialists/` directory created (empty) — loading from missing dir returns `(0, 0)` without error
- `AppState` now has `specialists: Arc<SpecialistRegistry>` field
- `SessionService::send_prompt` and `run_prompt_task` accept `&Arc<SpecialistRegistry>` / `Arc<SpecialistRegistry>`
- Model resolution priority: session.model → specialist.model → provider.default_model → hardcoded fallback
- System prompt injection: `session.specialist_id` → `specialists.get_by_name()` → `s.system_prompt` → `MessageRequest.system`
- Warning logged when `specialist_id` references a nonexistent specialist (graceful degradation, no error)
- `tool_profile` field parsed but not yet consumed — will be used by ToolRegistry (feat-012)
- Phase 2 (Agent Tools & Observability) started — feat-011 is the first feature in this phase

## Out-of-Scope Items Noticed

(none yet)
