# PROGRESS.md

<!--
The amnesiac craftsman's journal.
Updated at session start (read it) and session end (rewrite it).
A fresh session should be able to reach an executable state in under 3 minutes by reading this file.
-->

## Current State

- **Last updated:** 2026-05-31
- **Latest commit:** (pending — feat-008)
- **Active feature:** none
- **Build status:** green — `cargo build -p weave-server` succeeds
- **Test status:** green — 103 tests pass (18 new for feat-008 + 85 existing)
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

## In Progress

(none)

## Blocked

(none)

## Known Issues

- `web/` directory does not exist yet (expected — Phase 3)

## Next Steps

1. Start feat-009: SessionService (depends on feat-008 — now passing)
2. Continue Phase 1 (Core Foundation): feat-010

## Notes for Next Session

- feat-008 created: `src/store/sessions.rs`, `src/api/sessions.rs`
- `SessionStore` is a unit struct with CRUD + state machine enforcement
- State machine: `connecting` -> `ready` -> `completed`/`cancelled`/`error`; terminal states are final
- State machine enforcement is atomic via SQL `WHERE status NOT IN (...)` — no TOCTOU race
- `VALID_STATUSES` constant validates target status against known values
- `MessageStore` is immutable (create + list only, no update/delete)
- Message pagination uses `id` cursor (consistent with session pagination), not `created_at`
- FK violation for `provider_id` caught via `map_fk_violation` (extended code 787)
- `seed_deps` test helper is `pub(crate)` in `store::sessions::tests` for reuse by API tests
- `ListParams::effective_limit()` deduplicates pagination limit logic
- API routes: `POST /api/workspaces/{wid}/sessions`, `GET /api/workspaces/{wid}/sessions`, `GET /api/sessions/{id}`, `PATCH /api/sessions/{id}`, `DELETE /api/sessions/{id}`, `GET /api/sessions/{sid}/history`
- feat-009 (SessionService) depends on feat-008 — now passing, can start immediately

## Out-of-Scope Items Noticed

(none yet)
