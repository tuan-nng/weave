# PROGRESS.md

<!--
The amnesiac craftsman's journal.
Updated at session start (read it) and session end (rewrite it).
A fresh session should be able to reach an executable state in under 3 minutes by reading this file.
-->

## Current State

- **Last updated:** 2026-05-31
- **Latest commit:** (pending — feat-007)
- **Active feature:** none
- **Build status:** green — `cargo build -p weave-server` succeeds
- **Test status:** green — 85 tests pass (27 new for feat-007 + 58 existing)
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

## In Progress

(none)

## Blocked

(none)

## Known Issues

- `web/` directory does not exist yet (expected — Phase 3)

## Next Steps

1. Start feat-008: SessionStore (depends on feat-007 — now passing)
2. Continue Phase 1 (Core Foundation): feat-009 through feat-010

## Notes for Next Session

- feat-007 created: `src/store/providers.rs`, `src/agent/registry.rs`, `src/api/providers.rs`
- `ProviderStore` is a unit struct (same pattern as WorkspaceStore) with CRUD + `has_sessions` check
- `Provider` struct uses `#[serde(skip_serializing)]` on `config_json` to strip api_key from API responses
- `ProviderRegistry` holds `Mutex<HashMap<String, Arc<dyn CodingAgent>>>` — std::sync::Mutex, not tokio
- Registry `create_agent` is `pub(crate)` so API can use it to validate config before DB insert
- `AppError::Conflict(String)` added for 409 responses (provider delete with sessions)
- `AppState` now has two fields: `db: Arc<Db>` and `registry: Arc<ProviderRegistry>`
- Startup loads providers from DB into registry; failures logged as warnings, don't abort
- API routes: `GET /api/providers`, `POST /api/providers`, `DELETE /api/providers/{id}`, `GET /api/providers/{id}/models`
- Only "anthropic" provider type supported for v1; type validated in handler
- `list_models()` returns empty vec from AnthropicAgent — will be populated when more providers are added
- feat-008 (SessionStore) depends on feat-007 — now passing, can start immediately

## Out-of-Scope Items Noticed

(none yet)
